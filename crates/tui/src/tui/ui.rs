//! TUI event loop and rendering logic for `DeepSeek` CLI.

use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::future::Future;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{
    Arc, LazyLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::resource_telemetry::{TokenThroughput, estimate_output_tokens_from_text};
use anyhow::{Context, Result};
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
    buffer::Buffer,
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
    CacheWarmupKey, DeepSeekClient, PromptInspection, build_cache_warmup_request,
    inspect_prompt_for_request,
};
use crate::commands;
use crate::compaction::CompactionConfig;
use crate::compaction::estimate_input_tokens_conservative;
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
use crate::tui::color_compat::ColorCompatBackend;
use crate::tui::command_palette::{
    CommandPaletteView, build_entries_with_plugins as build_command_palette_entries,
};
use crate::tui::composer_ui::*;
use crate::tui::context_inspector::ContextInspectorView;
use crate::tui::event_broker::EventBroker;
use crate::tui::file_mention::ContextReference;
use crate::tui::file_picker_relevance;
use crate::tui::footer_ui::{
    friendly_subagent_progress, is_noisy_subagent_progress, render_footer,
};
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
use crate::tui::plan_prompt::PlanPromptView;
use crate::tui::plan_todo_bridge::{PlanAcceptance, project_accepted_plan};
use crate::tui::scrolling::TranscriptScroll;
use crate::work_graph::task_owner_snapshot;
// SelectionAutoscroll unused
use crate::tui::motion::{FrameRequester, MotionMode};
use crate::tui::session_picker::SessionPickerView;
use crate::tui::shell_job_routing::{
    add_shell_job_message, format_shell_job_list, format_shell_poll, open_shell_job_pager,
};
use crate::tui::streaming::StreamDisplayClock;
use crate::tui::streaming_thinking;
#[cfg(test)]
use crate::tui::subagent_routing::reconcile_subagent_activity_state_at;
use crate::tui::subagent_routing::{
    apply_subagent_terminal_projection, format_task_list, handle_subagent_mailbox, open_task_pager,
    parent_stop_status, reconcile_subagent_activity_state, running_agent_count,
    sort_subagents_in_place, subagent_message_refreshes_workspace_context, task_mode_label,
    task_summary_to_panel_entry,
};
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
    ActiveTurnMetadata, AgentCurrentActivity, AgentCurrentActivityStatus, App, AppAction, AppMode,
    HuntVerdict, OnboardingState, PendingProviderSwitch, QueuedMessage, ReasoningEffort,
    SidebarFocus, StatusToastLevel, SubmitDisposition, TaskPanelEntry, TaskPanelEntryKind,
    ToolEvidence, TuiOptions, bound_agent_activity_text, looks_like_slash_command_input,
    shell_command_from_bang_input,
};
use super::approval::{
    ApprovalMode, ApprovalRequest, ApprovalView, ElevationRequest, ElevationView, ReviewDecision,
};
use super::history::{
    HistoryCell, ToolCell, ToolStatus, history_cells_from_message, summarize_tool_output,
};
use super::slash_menu::{
    apply_slash_menu_selection, partial_inline_skill_mention_at_cursor,
    try_autocomplete_slash_command, visible_slash_menu_entries,
};
use super::views::{ConfigView, ContextMenuAction, HelpView, ModalKind, ViewEvent};
use super::widgets::pending_input_preview::{ContextPreviewItem, PendingInputPreview};
use super::widgets::{ChatWidget, ComposerWidget, HeaderData, HeaderWidget, Renderable};

// Activity Detail / raw-detail / pager-text helpers extracted into `activity_detail`
// (issue #4103). Re-export the cross-module entry points so existing
// `crate::tui::ui::{...}` importers (mouse_ui, footer_ui) keep resolving, and
// import the ui-internal entry points used from this file's own body.
pub(crate) use self::activity_detail::{
    copy_cell_to_clipboard, detail_target_label, open_details_pager_for_cell,
    selected_detail_footer_label, turn_handoff_markdown,
};
use self::activity_detail::{
    copy_focused_cell, detail_target_cell_index, extract_reasoning_header, open_tool_details_pager,
    open_turn_inspector_pager,
};
// Ctrl+O now opens the whole-turn Turn Inspector (#4104); the single-cell
// Activity Detail pager is no longer bound to a key, so it is only referenced
// from tests. (`v` raw leaf detail keeps using `open_tool_details_pager`.)
#[cfg(test)]
use self::activity_detail::open_activity_detail_pager;

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
/// ~12.5fps clock. Active markers run at 8fps; keeping the atmosphere on the
/// faster clock makes diagonal color travel continuous without forcing the
/// whole TUI onto a 30fps repaint loop.
pub(crate) const UI_UNDERWATER_ANIMATION_MS: u64 = 80;
// At an 80-column terminal the file tree owns 20 columns, leaving a 60-column
// chat host. Keep a compact 20-column sidebar plus a 40-column transcript.
pub(crate) const SIDEBAR_VISIBLE_MIN_WIDTH: u16 = 60;
const DEFAULT_TERMINAL_PROBE_TIMEOUT_MS: u64 = 500;
const TURN_META_PREFIX: &str = "<turn_meta>";
const SESSION_TITLE_MAX_CHARS: usize = 32;
const VERSION_HINT_TOAST_TTL_MS: u64 = 12_000;

const REQUIRED_RELEASE_ASSETS: &[&str] = &[
    "codewhale-artifacts-sha256.txt",
    "codew-android-arm64",
    "codewhale-android-arm64",
    "codewhale-android-arm64.tar.gz",
    "codewhale-tui-android-arm64",
    "codewhale-linux-arm64",
    "codewhale-linux-arm64.tar.gz",
    "codewhale-linux-x64",
    "codewhale-linux-x64.tar.gz",
    "codewhale-macos-arm64",
    "codewhale-macos-arm64.tar.gz",
    "codewhale-macos-x64",
    "codewhale-macos-x64.tar.gz",
    "codewhale-tui-linux-arm64",
    "codewhale-tui-linux-x64",
    "codewhale-tui-macos-arm64",
    "codewhale-tui-macos-x64",
    "codewhale-tui-windows-x64.exe",
    "codewhale-windows-x64.exe",
    "codewhale-windows-x64-portable.zip",
    "codewhale-windows-x64.zip",
    "codew-windows-arm64.exe",
    "codewhale-tui-windows-arm64.exe",
    "codewhale-windows-arm64.exe",
    "codewhale-windows-arm64-portable.zip",
    "codewhale-windows-arm64.zip",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarRenderState {
    Hidden,
    SuppressedByWidth {
        available_width: u16,
        min_width: u16,
    },
    AutoCollapsed,
    Visible,
}

pub(crate) fn sidebar_render_state(app: &mut App) -> SidebarRenderState {
    if app.sidebar_focus == SidebarFocus::Hidden {
        return SidebarRenderState::Hidden;
    }

    if let Some(available_width) = sidebar_host_width_hint(app)
        && available_width < SIDEBAR_VISIBLE_MIN_WIDTH
    {
        return SidebarRenderState::SuppressedByWidth {
            available_width,
            min_width: SIDEBAR_VISIBLE_MIN_WIDTH,
        };
    }

    if crate::tui::sidebar::sidebar_auto_idle(app) {
        return SidebarRenderState::AutoCollapsed;
    }

    SidebarRenderState::Visible
}

fn sidebar_host_width_hint(app: &App) -> Option<u16> {
    app.last_sidebar_host_width.or_else(|| {
        let transcript_width = app.viewport.last_transcript_area.map(|area| area.width)?;
        let sidebar_width = app
            .viewport
            .last_sidebar_area
            .or(app.last_sidebar_area)
            .map(|area| area.width)
            .unwrap_or(0);
        Some(transcript_width.saturating_add(sidebar_width))
    })
}

fn sidebar_width_for_chat_area(app: &App, chat_width: u16) -> Option<u16> {
    if app.sidebar_focus == SidebarFocus::Hidden || chat_width < SIDEBAR_VISIBLE_MIN_WIDTH {
        return None;
    }

    let preferred_sidebar =
        (u32::from(chat_width) * u32::from(app.sidebar_width_percent.clamp(10, 50)) / 100) as u16;
    let sidebar_width = preferred_sidebar.max(24).min(chat_width.saturating_sub(40));

    (sidebar_width >= 20).then_some(sidebar_width)
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
// Xterm alternate-scroll mode keeps wheel events inside the alternate-screen
// viewport when mouse capture is requested but unavailable or temporarily
// dropped. Leave it off with `--no-mouse-capture` so the host terminal owns
// raw mouse selection behavior end-to-end.
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

struct TerminalInputPump {
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

fn next_terminal_event(
    input: &TerminalInputPump,
    pending: &mut VecDeque<Event>,
    timeout: Duration,
) -> io::Result<Option<Event>> {
    if let Some(event) = pending.pop_front() {
        return Ok(Some(event));
    }
    input.recv_timeout(timeout)
}

fn try_next_terminal_event(
    input: &TerminalInputPump,
    pending: &mut VecDeque<Event>,
) -> io::Result<Option<Event>> {
    if let Some(event) = pending.pop_front() {
        return Ok(Some(event));
    }
    input.try_recv()
}

fn drain_terminal_input_queue(
    input: &TerminalInputPump,
    pending: &mut VecDeque<Event>,
) -> io::Result<()> {
    pending.clear();
    while input.try_recv()?.is_some() {}
    Ok(())
}

fn collect_pending_terminal_events(
    input: &TerminalInputPump,
    pending: &mut VecDeque<Event>,
) -> io::Result<()> {
    while let Some(event) = input.try_recv()? {
        pending.push_back(event);
    }
    Ok(())
}

fn engine_drain_budget_exhausted(events_drained: usize, started: Instant, now: Instant) -> bool {
    events_drained >= MAX_ENGINE_EVENTS_PER_DRAIN
        || now.saturating_duration_since(started) >= ENGINE_DRAIN_TIME_BUDGET
}

fn open_setup_checkpoint_if_due(app: &mut App, config: &Config, skip_onboarding: bool) -> bool {
    if skip_onboarding {
        if crate::tui::setup::should_open_update_checkpoint(app, config)
            && let Err(err) = crate::tui::setup::defer_update_checkpoint_for_app(app, config)
        {
            tracing::warn!(
                target: "tui::setup",
                "failed to record deferred setup checkpoint: {err}"
            );
        }
        return false;
    }
    if app.onboarding != crate::tui::app::OnboardingState::None
        || app.view_stack.top_kind() == Some(ModalKind::SetupWizard)
        || !crate::tui::setup::should_open_update_checkpoint(app, config)
    {
        return false;
    }

    // A fresh wizard invalidates any in-flight model draft from a prior one.
    let _ = app.next_draft_gen();
    app.view_stack
        .push(crate::tui::setup::SetupWizardView::new_for_app(app, config));
    true
}

fn complete_trust_directory_onboarding(app: &mut App, config: &Config) -> Result<(), String> {
    onboarding::mark_trusted(&app.workspace).map_err(|err| err.to_string())?;
    app.trust_mode = true;
    app.hooks = HookExecutor::new(
        crate::hooks::HooksConfig::load_with_project(config.hooks_config(), &app.workspace),
        app.workspace.clone(),
    );
    app.runtime_services.hook_executor = Some(std::sync::Arc::new(app.hooks.clone()));
    app.status_message = None;
    if app.onboarding_workspace_trust_gate {
        app.onboarding_workspace_trust_gate = false;
        app.onboarding = OnboardingState::None;
    } else if app.onboarding_missing_key_recovery {
        app.onboarding = OnboardingState::Tips;
    } else {
        app.onboarding = OnboardingState::MentalModels;
    }
    Ok(())
}

fn back_from_api_key_onboarding(app: &mut App) {
    app.onboarding = OnboardingState::Provider;
    app.api_key_input.clear();
    app.api_key_cursor = 0;
    app.status_message = None;
}

fn back_from_provider_onboarding(app: &mut App) {
    if app.onboarding_missing_key_recovery {
        // A returning user declined missing-key recovery: leave onboarding
        // for the offline composer without mutating the saved route.
        app.onboarding = OnboardingState::None;
        app.status_message = None;
        app.needs_redraw = true;
        return;
    }
    app.onboarding = OnboardingState::Language;
    app.status_message = None;
}

fn complete_provider_picker_onboarding(app: &mut App, provider: ApiProvider) {
    app.onboarding_provider = provider;
    app.onboarding_needs_api_key = false;
    app.api_key_env_only = false;
    app.offline_mode = false;
    onboarding::advance_onboarding_after_api_key(app);
}

async fn submit_keyless_onboarding_provider(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
) -> bool {
    let provider = app.onboarding_provider;
    if !onboarding::onboarding_provider_allows_empty_api_key(config, provider) {
        return false;
    }
    if !switch_provider(app, engine_handle, config, provider, None).await {
        return false;
    }

    app.api_key_input.clear();
    app.api_key_cursor = 0;
    app.onboarding_needs_api_key = false;
    app.api_key_env_only = false;
    app.offline_mode = false;
    onboarding::advance_onboarding_after_api_key(app);
    true
}

fn surface_prompt_override_notices(app: &mut App) {
    for notice in prompts::take_prompt_override_notices() {
        app.add_message(HistoryCell::System {
            content: format!("Warning: {notice}"),
        });
        app.push_status_toast(notice, StatusToastLevel::Warning, Some(12_000));
    }
}

/// Run the interactive TUI event loop.
///
/// # Examples
///
/// ```ignore
/// # use crate::config::Config;
/// # use crate::tui::TuiOptions;
/// # async fn example(config: &Config, options: TuiOptions) -> anyhow::Result<()> {
/// crate::tui::run_tui(config, options).await
/// # }
/// ```
pub async fn run_tui(
    config: &Config,
    options: TuiOptions,
    plugin_registry: std::sync::Arc<crate::plugins::PluginRegistry>,
) -> Result<()> {
    let use_alt_screen = options.use_alt_screen;
    let use_mouse_capture = options.use_mouse_capture;
    let use_bracketed_paste = options.use_bracketed_paste;

    // Apply OSC 8 hyperlink toggle from config.
    //
    // #3029: OSC 8 hyperlinks are emitted out-of-band. Markdown wrapping keeps
    // visible spans and per-line targets in separate structures; each render
    // seam translates those targets into absolute `LinkRegion`s without ever
    // placing an escape byte in a ratatui buffer cell. `ColorCompatBackend`
    // then emits the OSC 8 escapes through its `Write` impl around the matching
    // cell runs. Hyperlinks are on by default for terminals that handle the OSC
    // terminator (`ESC \`) cleanly. Windows legacy consoles (conhost) still
    // mishandle the terminator, so the default stays off there; opt in via
    // `[tui] osc8_links = true` on any platform.
    let osc8_default_on = !cfg!(target_os = "windows");
    crate::tui::osc8::set_enabled(
        config
            .tui
            .as_ref()
            .and_then(|tui| tui.osc8_links)
            .unwrap_or(osc8_default_on),
    );

    // Terminal probe with timeout to prevent hanging on unresponsive terminals.
    //
    // The blocking task cannot be cancelled once the timeout fires, so a slow
    // `enable_raw_mode` may still succeed *after* we've bailed out, leaking
    // raw mode. Both sides run `raw_mode_probe_handshake`; whichever observes
    // the other's flag disables raw mode again.
    let probe_timeout = terminal_probe_timeout(config);
    let probe_abandoned = Arc::new(AtomicBool::new(false));
    let probe_enabled = Arc::new(AtomicBool::new(false));
    let task_abandoned = Arc::clone(&probe_abandoned);
    let task_enabled = Arc::clone(&probe_enabled);
    let enable_raw = tokio::task::spawn_blocking(move || {
        let result =
            enable_raw_mode().map_err(|e| anyhow::anyhow!("Failed to enable raw mode: {e}"));
        if result.is_ok() && raw_mode_probe_handshake(&task_enabled, &task_abandoned) {
            // The probe timed out while we were blocked; the caller already
            // gave up, so undo the late enable instead of leaking raw mode.
            let _ = disable_raw_mode();
        }
        result
    });

    match tokio::time::timeout(probe_timeout, enable_raw).await {
        Ok(inner_result) => {
            inner_result??; // propagate both join and raw-mode errors
        }
        Err(_) => {
            if raw_mode_probe_handshake(&probe_abandoned, &probe_enabled) {
                // The blocking task finished enabling raw mode right as the
                // timeout fired and may have missed the abandoned flag.
                let _ = disable_raw_mode();
            }
            tracing::warn!(
                "Terminal probe timed out after {}ms - terminal may be unresponsive",
                probe_timeout.as_millis()
            );
            return Err(anyhow::anyhow!(
                "Terminal probe timed out after {}ms",
                probe_timeout.as_millis()
            ));
        }
    }

    #[cfg(target_os = "windows")]
    enable_windows_ime_console_mode();

    let mut stdout = io::stdout();
    // Initialize the file-backed TUI log and redirect raw stderr away from
    // the alt-screen for the lifetime of this guard. MUST run BEFORE
    // EnterAlternateScreen; otherwise logging between alt-screen entry and
    // redirect init leaks raw bytes into the TUI buffer, causing the "scroll
    // demon" on Windows (#1909) and garbled output on all platforms (#1085).
    // The guard is held until the function returns; dropping it after
    // LeaveAlternateScreen restores the original stderr handle/fd so shutdown
    // messages reach the user's terminal. We accept the init failing (e.g.,
    // read-only $HOME) and continue without the redirect rather than refusing
    // to start the TUI.
    let _tui_log_guard = match crate::runtime_log::init() {
        Ok(guard) => Some(guard),
        Err(err) => {
            tracing::warn!(target: "runtime_log", ?err, "TUI log init failed; stderr leaks may render as scroll-demon");
            None
        }
    };
    if use_alt_screen {
        execute!(stdout, EnterAlternateScreen)?;
        // Windows also suppresses Codewhale's own verbose CLI logger while
        // the alt-screen is active. The stderr redirect above catches raw
        // writes; this prevents the known verbose source at the origin.
        #[cfg(windows)]
        crate::logging::snapshot_verbose_state();
        #[cfg(windows)]
        crate::logging::set_verbose(false);
    }
    // Mouse capture, bracketed paste, focus events, and the Kitty
    // keyboard-protocol escape-disambiguation flag (#442). Single source
    // of truth shared with the FocusGained recovery path and
    // resume_terminal — see recover_terminal_modes.
    //
    // Focus events are necessary for IME compositor re-activation on
    // macOS when the user switches away (Cmd+Tab) and returns. The Kitty
    // keyboard protocol opt-in is best-effort: terminals that don't
    // support it (iTerm2, Terminal.app, Windows 10 conhost) silently
    // discard the escape, while supporting terminals (Kitty, Ghostty,
    // Alacritty 0.13+, WezTerm, recent Konsole, recent xterm) report
    // unambiguous events for Option/Alt-modified keys and plain Esc.
    //
    // Only `DISAMBIGUATE_ESCAPE_CODES` is pushed — the higher tiers
    // (`REPORT_EVENT_TYPES`, `REPORT_ALL_KEYS_AS_ESCAPE_CODES`) emit
    // release events that the existing key handlers would mis-route
    // as duplicate presses.
    //
    // On Windows, crossterm's `PushKeyboardEnhancementFlags` command always
    // reports the terminal as unsupported (`is_ansi_code_supported` returns
    // false), so the escape is written directly instead. VSCode's integrated
    // terminal and Windows Terminal ≥1.17 honour the kitty keyboard protocol
    // and will correctly disambiguate Shift+Enter from plain Enter once this
    // sequence is received. Terminals that do not understand it silently
    // ignore it.
    recover_terminal_modes(&mut stdout, use_mouse_capture, use_bracketed_paste);
    let mut cleanup_guard = TerminalCleanupGuard {
        use_alt_screen,
        use_mouse_capture,
        use_bracketed_paste,
        defused: false,
    };
    let color_depth = palette::ColorDepth::detect();
    let palette_mode = palette::PaletteMode::detect();
    tracing::debug!(
        ?color_depth,
        ?palette_mode,
        "terminal color profile detected"
    );
    let backend = ColorCompatBackend::new(stdout, color_depth, palette_mode);
    let mut terminal = Terminal::new(backend)?;
    // At this point Settings hasn't loaded yet, so we can't read the
    // user's `synchronized_output` knob. Use the same env-based terminal
    // quirk detection that `Settings::apply_env_overrides` uses, so the
    // startup viewport reset matches what every later draw will do on
    // flicker-sensitive hosts. A user who has explicitly set
    // `synchronized_output = "on"` to override detection will get sync wrap
    // from the main draw loop onward; the one-time startup viewport reset
    // stays opt-out for them, which is the safe default because the cost is
    // at most brief tearing on the first frame.
    let sync_output_at_init = !crate::settings::detected_ptyxis_terminal()
        && !crate::settings::detected_legacy_windows_console_host();
    reset_terminal_viewport(&mut terminal, sync_output_at_init)?;
    let event_broker = EventBroker::new();

    // Local mutable copy so runtime config flips (e.g. `/provider` switch)
    // can rebuild the API client without restarting the process.
    let mut config = config.clone();
    let config = &mut config;
    let mut app = App::new_with_plugin_registry(options.clone(), config, plugin_registry);
    crate::startup_trace::mark("app_constructed");
    sync_config_provider_from_app(config, &app);
    surface_prompt_override_notices(&mut app);

    if options.resume_session_id.is_none() && !app.launch.visible {
        let opened_setup = open_setup_checkpoint_if_due(&mut app, config, options.skip_onboarding);
        // One-time Fleet + Hotbar intro for returning (non-resuming) users.
        // First-time users see it when they finish onboarding. Gated by a
        // persisted flag, so it shows exactly once and never inside a resumed
        // session transcript or behind the constitution checkpoint.
        if !opened_setup {
            app.maybe_show_feature_intro();
        }
    }

    // Load existing session if resuming.
    if let Some(ref session_id) = options.resume_session_id
        && let Ok(manager) = SessionManager::default_location()
    {
        // Try to load by prefix or full ID
        let load_result: std::io::Result<Option<crate::session_manager::SavedSession>> =
            if session_id == "latest" {
                // Special case: resume the most recent session in this workspace.
                match manager.get_latest_session_for_workspace(&options.workspace) {
                    Ok(Some(meta)) => manager.load_session(&meta.id).map(Some),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            } else {
                manager.load_session_by_prefix(session_id).map(Some)
            };

        match load_result {
            Ok(Some(saved)) => match apply_loaded_session(&mut app, config, &saved) {
                Ok(false) => {
                    app.status_message = Some(format!(
                        "Resumed session: {}",
                        crate::session_manager::truncate_id(&saved.metadata.id)
                    ));
                }
                Ok(true) => {}
                Err(err) => {
                    app.status_message = Some(format!("Failed to restore session: {err}"));
                }
            },
            Ok(None) => {
                app.status_message = Some("No sessions found to resume".to_string());
            }
            Err(e) => {
                app.status_message = Some(format!("Failed to load session: {e}"));
            }
        }
    }

    if let Ok(manager) = SessionManager::default_location() {
        match manager.load_offline_queue_state() {
            Ok(Some(state)) => {
                // Only restore queue if session_id matches (or if we're resuming the same session)
                let should_restore = match (&state.session_id, &app.current_session_id) {
                    (Some(saved_id), Some(current_id)) => saved_id == current_id,
                    (None, _) => false, // Legacy unscoped queues are stale-risky; fail closed.
                    (_, None) => false, // No current session - don't restore
                };

                if should_restore {
                    app.queued_messages = state
                        .messages
                        .into_iter()
                        .map(queued_session_to_ui)
                        .collect();
                    let restored_draft = state.draft.map(queued_session_to_ui);
                    if restored_draft.is_some() || app.queued_draft.is_none() {
                        app.queued_draft = restored_draft;
                    }
                    if app.status_message.is_none() && app.queued_message_count() > 0 {
                        app.status_message = Some(format!(
                            "Restored {} queued message(s) from previous session — ↑ to edit, Ctrl+X to discard",
                            app.queued_message_count()
                        ));
                    }
                } else {
                    // Session mismatch - clear the stale queue
                    let _ = manager.clear_offline_queue_state();
                }
            }
            Ok(None) => {}
            Err(err) => {
                if app.status_message.is_none() {
                    app.status_message = Some(format!("Failed to restore offline queue: {err}"));
                }
            }
        }
    }

    let task_manager = TaskManager::start(
        TaskManagerConfig::from_runtime(
            config,
            app.workspace.clone(),
            Some(app.model.clone()),
            Some(app.max_subagents.clamp(1, 4)),
        ),
        config.clone(),
        std::sync::Arc::clone(&app.plugin_registry),
    )
    .await?;
    let automations = std::sync::Arc::new(tokio::sync::Mutex::new(
        AutomationManager::default_location()?,
    ));
    let automation_cancel = tokio_util::sync::CancellationToken::new();
    let automation_scheduler = spawn_scheduler(
        automations.clone(),
        task_manager.clone(),
        automation_cancel.clone(),
        AutomationSchedulerConfig::default(),
    );
    let shell_manager = app
        .runtime_services
        .shell_manager
        .clone()
        .unwrap_or_else(|| crate::tools::shell::new_shared_shell_manager(app.workspace.clone()));
    // #2511: ensure hook_executor is initialized for fresh sessions — it is
    // only set by apply_workspace_runtime_state (session resume / workspace
    // switch), so a brand-new session would otherwise leave it None and both
    // exec_shell shell_env hooks and ToolCallBefore gate would silently no-op.
    if app.runtime_services.hook_executor.is_none() {
        app.runtime_services.hook_executor = Some(std::sync::Arc::new(app.hooks.clone()));
    }
    app.runtime_services = RuntimeToolServices {
        shell_manager: Some(shell_manager),
        task_manager: Some(task_manager.clone()),
        automations: Some(automations),
        task_data_dir: Some(task_manager.data_dir()),
        active_task_id: None,
        active_thread_id: None,
        dynamic_tool_executor: None,
        work: app.runtime_services.work.clone(),
        // #456: plumb the App's HookExecutor so `exec_shell` can surface
        // the configured `shell_env` hooks. Clone the shared Arc.
        hook_executor: app.runtime_services.hook_executor.clone(),
        handle_store: app.runtime_services.handle_store.clone(),
        rlm_sessions: app.runtime_services.rlm_sessions.clone(),
    };
    crate::startup_trace::mark("task_manager_ready");
    refresh_active_task_panel(&mut app, &task_manager).await;

    let engine_config = build_engine_config(&app, config);

    // Spawn the Engine - it will handle all API communication
    let engine_handle = spawn_tui_engine(engine_config, config);
    crate::startup_trace::mark("engine_spawned");
    // The translation client is optional: it never crashes the TUI on
    // startup, even when the API key is missing, the base URL is malformed,
    // or the network is unavailable.
    // Translations are skipped with a logged warning until a key is saved.
    let translation_client = match DeepSeekClient::new(config) {
        Ok(client) => Some(Arc::new(client)),
        Err(err) => {
            if app.onboarding == OnboardingState::None {
                tracing::warn!("Translation client initialization failed: {err}");
            }
            None
        }
    };

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
                mode: app.mode,
            })
            .await;
    }

    // The engine owns the canonical model-facing prompt from startup. Mirror
    // that exact value before the first draw so `/context` never reports an
    // empty system prompt merely because no user turn has been submitted yet.
    match engine_handle.get_session_snapshot().await {
        Ok(snapshot) => app.system_prompt = snapshot.system_prompt,
        Err(err) => tracing::warn!("could not mirror initial engine system prompt: {err:#}"),
    }

    // Fire session start hook
    {
        let context = app.base_hook_context();
        let _ = app.execute_hooks(HookEvent::SessionStart, &context);
    }

    // Spawn the persistence actor so checkpoint/session-save I/O stays off
    // the UI thread.  The actor serialises + writes to disk in a dedicated
    // task; the UI just `try_send`s a request and returns immediately.
    let persistence_runtime = SessionManager::default_location()
        .ok()
        .map(|persist_manager| {
            let (handle, task) = persistence_actor::spawn_persistence_actor(persist_manager);
            persistence_actor::init_actor(handle.clone());
            (handle, task)
        });

    // Returning users with a missing key begin directly in the same canonical
    // provider setup picker as first-run. Focus their persisted route so
    // recovery does not silently replace a Kimi Code bare-K3 endpoint.
    if app.onboarding == OnboardingState::Provider {
        open_onboarding_provider_picker(&mut app, config, &engine_handle, true).await;
    }

    // #4605: create the dispatch completion channel before any submit path so
    // initial input and queued follow-ups can dispatch without blocking the
    // startup sequence.
    let (dispatch_completion_tx, dispatch_completion_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::tui::app::DispatchApplyFn>();
    app.dispatch_completion_tx = Some(dispatch_completion_tx);

    submit_initial_input_if_ready(&mut app, config, &engine_handle).await?;

    crate::startup_trace::log_summary();
    let result = run_event_loop(
        &mut terminal,
        &mut app,
        config,
        engine_handle,
        task_manager,
        &event_broker,
        translation_client,
        dispatch_completion_rx,
    )
    .await;
    automation_cancel.cancel();
    automation_scheduler.abort();

    // Fire session end hook
    {
        let context = app.base_hook_context();
        let _ = app.execute_hooks(HookEvent::SessionEnd, &context);
    }

    // Flush the persistence actor: clear this session's checkpoint, collect
    // the durability report (write failures are surfaced, not discarded),
    // then shut down gracefully.
    if let Some((handle, task)) = persistence_runtime {
        if let Some(session_id) = app.current_session_id.clone() {
            handle.try_send(PersistRequest::ClearCheckpoint { session_id });
        }
        let (report_tx, report_rx) = tokio::sync::oneshot::channel();
        handle.try_send(PersistRequest::FlushAndReport { reply: report_tx });
        if let Ok(report) = report_rx.await
            && !report.failures.is_empty()
        {
            tracing::warn!(
                target: "persistence",
                failures = ?report.failures,
                "session persistence reported write failures during shutdown",
            );
        }
        handle.try_send(PersistRequest::Shutdown);
        let _ = task.await;
    }

    cleanup_guard.defused = true;
    pop_keyboard_enhancement_flags(terminal.backend_mut());
    disable_alternate_scroll_mode(terminal.backend_mut());
    execute!(terminal.backend_mut(), DisableFocusChange)?;
    disable_raw_mode()?;
    if use_alt_screen {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        #[cfg(windows)]
        crate::logging::restore_verbose_state();
    }
    if use_mouse_capture {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    if use_bracketed_paste {
        disable_bracketed_paste_mode(terminal.backend_mut());
    }
    terminal.show_cursor()?;
    drop(terminal);

    if result.is_ok() && should_show_resume_hint(app.current_session_id.as_deref()) {
        // Printed AFTER `LeaveAlternateScreen` / `drop(terminal)` above,
        // so we're back on the primary screen — this is the one
        // legitimate stdout write in the TUI module tree. The
        // module-level `#![deny(clippy::print_stdout)]` would otherwise
        // refuse it.
        #[allow(clippy::print_stdout)]
        {
            println!("{}", resume_hint_text());
        }
    }

    result
}

fn should_show_resume_hint(session_id: Option<&str>) -> bool {
    session_id.is_some_and(|id| !id.trim().is_empty())
}

fn resume_hint_text() -> &'static str {
    "To continue this session, execute codewhale run --continue"
}

/// One side of the raw-mode probe abandonment handshake between the startup
/// probe timeout and the blocking `enable_raw_mode` task finishing late.
///
/// Each side publishes its own flag (`publish`), then checks whether the
/// other side's flag (`check`) is already up; a `true` return means this
/// side must disable raw mode again. `SeqCst` ordering guarantees that when
/// both sides run, at least one observes the other's flag, so a raw-mode
/// enable landing after the probe timeout is always undone. Both sides
/// observing each other is fine — a duplicate `disable_raw_mode` is a no-op.
fn raw_mode_probe_handshake(publish: &AtomicBool, check: &AtomicBool) -> bool {
    publish.store(true, Ordering::SeqCst);
    check.load(Ordering::SeqCst)
}

fn terminal_probe_timeout(config: &Config) -> Duration {
    let timeout_ms = config
        .tui
        .as_ref()
        .and_then(|tui| tui.terminal_probe_timeout_ms)
        .unwrap_or(DEFAULT_TERMINAL_PROBE_TIMEOUT_MS)
        .clamp(100, 5_000);
    Duration::from_millis(timeout_ms)
}

fn execute_subagent_observer_hook(
    app: &App,
    event: HookEvent,
    agent_id: &str,
    text_field: &str,
    text: &str,
) {
    if !app.hooks.has_hooks_for_event(event) {
        return;
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

    let hooks = app.hooks.clone();
    let _ = std::thread::Builder::new()
        .name(format!("{}-observer-hook", event.as_str()))
        .spawn(move || {
            let _ = hooks.execute_json_observer(event, &context, &payload);
        });
}

fn execute_turn_end_observer_hook(
    app: &App,
    turn: Option<&ActiveTurnMetadata>,
    usage: &Usage,
    billing_surface: Option<&str>,
    duration: Duration,
    error: Option<&str>,
) {
    if !app.hooks.has_hooks_for_event(HookEvent::TurnEnd) {
        return;
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
    let hooks = app.hooks.clone();
    let _ = std::thread::Builder::new()
        .name("turn_end-observer-hook".to_string())
        .spawn(move || {
            let _ = hooks.execute_json_observer(HookEvent::TurnEnd, &context, &payload);
        });
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

fn subagent_terminal_verb(status: &SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Completed => "completed",
        SubAgentStatus::Interrupted(_) => "interrupted",
        SubAgentStatus::Failed(_) => "failed",
        SubAgentStatus::Cancelled => "cancelled",
        SubAgentStatus::BudgetExhausted => "exhausted its budget",
        SubAgentStatus::Running => "finished",
    }
}

fn subagent_terminal_projection_from_mailbox(
    message: &MailboxMessage,
) -> Option<(&str, SubAgentStatus, Option<String>)> {
    match message {
        MailboxMessage::Completed { agent_id, summary } => Some((
            agent_id.as_str(),
            SubAgentStatus::Completed,
            Some(summary.clone()),
        )),
        MailboxMessage::Failed { agent_id, error } => Some((
            agent_id.as_str(),
            SubAgentStatus::Failed(error.clone()),
            Some(error.clone()),
        )),
        MailboxMessage::Interrupted { agent_id, reason } => Some((
            agent_id.as_str(),
            SubAgentStatus::Interrupted(reason.clone()),
            Some(reason.clone()),
        )),
        MailboxMessage::Cancelled { agent_id } => Some((
            agent_id.as_str(),
            SubAgentStatus::Cancelled,
            Some("cancelled".to_string()),
        )),
        _ => None,
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
    config.memory_enabled() && !config.moraine_fallback() && is_memory_quick_add(input)
}

#[cfg(test)]
mod memory_quick_add_tests {
    use super::should_intercept_memory_quick_add;
    use crate::config::Config;

    #[test]
    fn memory_quick_add_interception_respects_moraine_fallback() {
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

        let moraine: Config = toml::from_str(
            r#"
            [memory]
            enabled = true
            moraine_fallback = true
            "#,
        )
        .expect("parse moraine memory config");
        assert!(!should_intercept_memory_quick_add(
            &moraine,
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

/// Persist a `# foo` quick-add to the memory file and surface a status
/// note to the user. Errors land in the same status channel so a missing
/// memory directory becomes visible without crashing the composer.
fn handle_memory_quick_add(app: &mut App, input: &str, config: &Config) {
    let path = config.memory_path();
    match crate::memory::append_entry(&path, input) {
        Ok(()) => {
            app.status_message = Some(format!("memory: appended to {}", path.display()));
        }
        Err(err) => {
            app.status_message = Some(format!(
                "memory: failed to write {}: {}",
                path.display(),
                err
            ));
        }
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

fn build_engine_config(app: &App, config: &Config) -> EngineConfig {
    let provider = app.api_provider;
    let max_subagents = app.max_subagents.clamp(1, crate::config::MAX_SUBAGENTS);
    EngineConfig {
        model: app.model.clone(),
        active_route_limits: app.active_route_limits,
        workspace: app.workspace.clone(),
        allow_shell: app.allow_shell,
        trust_mode: app.trust_mode,
        notes_path: config.notes_path(),
        mcp_config_path: config.mcp_config_path(),
        skills_dir: app.skills_dir.clone(),
        skills_scan_codewhale_only: app.skills_scan_codewhale_only,
        plugin_registry: Some(std::sync::Arc::clone(&app.plugin_registry)),
        instructions: configured_instruction_sources(config),
        project_context_pack_enabled: config.project_context_pack_enabled(),
        translation_enabled: app.translation_enabled,
        show_thinking: app.show_thinking,
        verbosity: app.verbosity.clone(),
        // Effectively unlimited. V4 has a 1M context window and the user
        // wants the model running until it's actually done. The previous cap
        // of 100 hit the ceiling on long multi-step plans (wide refactors,
        // sub-agent orchestration) and presented as the agent "giving up
        // mid-task". `u32::MAX` is the type ceiling; users can still
        // interrupt with Ctrl+C / Esc, and a turn naturally ends when the
        // model stops emitting tool calls. A real runaway is rare and
        // human-noticeable; we trust the operator over a hard step cap.
        max_steps: u32::MAX,
        max_subagents,
        max_admitted_subagents: config
            .max_admitted_subagents_for_provider(provider)
            .max(max_subagents),
        launch_concurrency: config
            .launch_concurrency_for_provider(provider)
            .max(app.mode.mode_delegation_launch_floor()),
        subagents_enabled: config.subagents_enabled_for_provider(provider),
        features: config.features(),
        auto_review_policy: config.auto_review_policy(),
        compaction: app.compaction_config(),
        todos: app.todos.clone(),
        plan_state: app.plan_state.clone(),
        goal_state: crate::tools::goal::new_shared_goal_state_from_host_status(
            app.hunt.quarry.clone(),
            app.hunt.token_budget,
            app.hunt.verdict.goal_status(),
        ),
        max_spawn_depth: config.subagent_max_spawn_depth_for_provider(provider),
        subagent_token_budget: config.subagent_token_budget_for_provider(provider),
        allowed_tools: app.active_allowed_tools.clone(),
        disallowed_tools: None,
        hook_executor: app.runtime_services.hook_executor.clone(),
        network_policy: config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        }),
        snapshots_enabled: config.snapshots_config().enabled,
        snapshots_max_workspace_bytes: config
            .snapshots_config()
            .max_workspace_gb
            .saturating_mul(1024 * 1024 * 1024),
        lsp_config: config
            .lsp
            .clone()
            .map(crate::config::LspConfigToml::into_runtime),
        runtime_services: app.runtime_services.clone(),
        subagent_model_overrides: config.subagent_model_overrides(),
        fleet_roster: std::sync::Arc::new(crate::fleet::roster::FleetRoster::load(
            &config.fleet_config(),
            &app.workspace,
        )),
        subagent_api_timeout: Duration::from_secs(
            config.subagent_api_timeout_secs_for_provider(provider),
        ),
        stream_chunk_timeout: Duration::from_secs(app.stream_chunk_timeout_secs),
        subagent_heartbeat_timeout: Duration::from_secs(
            config.subagent_heartbeat_timeout_secs_for_provider(provider),
        ),
        prefer_bwrap: config.prefer_bwrap.unwrap_or(false),
        memory_enabled: config.memory_enabled(),
        moraine_fallback: config.moraine_fallback(),
        memory_path: config.memory_path(),
        speech_output_dir: config.speech_output_dir(),
        vision_config: config.vision_model_config(),
        strict_tool_mode: config.strict_tool_mode.unwrap_or(false),
        goal_objective: app.hunt.quarry.clone(),
        goal_token_budget: app.hunt.token_budget,
        goal_status: app.hunt.verdict.goal_status(),
        locale_tag: app.ui_locale.tag().to_string(),
        workshop: config.workshop.clone(),
        search_provider: config.search_provider(),
        search_api_key: config.search.as_ref().and_then(|s| s.api_key.clone()),
        search_base_url: config.search.as_ref().and_then(|s| s.base_url.clone()),
        tools_always_load: config.tools_always_load(),
        tools: config.tools.clone(),
        workspace_follow_symlinks: app.workspace_follow_symlinks,
        exec_policy_engine: config.exec_policy_engine.clone(),
        terminal_chrome_enabled: true,
    }
}

fn configured_instruction_sources(config: &Config) -> Vec<prompts::InstructionSource> {
    config
        .instructions_paths()
        .into_iter()
        .map(Into::into)
        .collect()
}

#[cfg(test)]
fn build_app_system_prompt(app: &App, config: &Config) -> SystemPrompt {
    build_app_system_prompt_with_goal(app, config, app.hunt.quarry.as_deref())
}

fn build_app_system_prompt_with_goal(
    app: &App,
    config: &Config,
    goal_objective: Option<&str>,
) -> SystemPrompt {
    let instructions = configured_instruction_sources(config);
    let memory_path = config.memory_path();
    let user_memory_block = crate::memory::compose_block(
        config.memory_enabled() && !config.moraine_fallback(),
        &memory_path,
    );
    prompts::system_prompt_for_mode_with_context_skills_and_session(
        &app.workspace,
        None,
        Some(&app.skills_dir),
        Some(&instructions),
        prompts::PromptSessionContext {
            user_memory_block: user_memory_block.as_deref(),
            goal_objective,
            project_context_pack_enabled: config.project_context_pack_enabled(),
            locale_tag: app.ui_locale.tag(),
            translation_enabled: app.translation_enabled,
            model_id: &app.model,
            context_window_override: Some(crate::route_budget::route_context_window_tokens(
                app.api_provider,
                &app.model,
                app.active_route_limits,
            )),
            show_thinking: app.show_thinking,
            verbosity: app.verbosity.as_deref(),
            skills_scan_codewhale_only: app.skills_scan_codewhale_only,
            plugin_registry: Some(app.plugin_registry.as_ref()),
        },
    )
}

/// Choose which durable-task summaries should appear in the Work
/// sidebar's Tasks panel.
///
/// Active tasks (`Queued`/`Running`) are always included. Terminal
/// tasks (`Completed`/`Failed`/`Canceled`) are session-local receipts: both
/// their creation and completion must fall within this TUI session. Durable
/// tasks are stored per user rather than per TUI process, and startup recovery
/// can stamp an old running record with a fresh `ended_at`. Treating that as a
/// current receipt makes a new same-workspace instance look failed (#4416).
/// Shared and older task history remains available explicitly through
/// `/tasks`; it does not belong on the fresh live-work surface by default.
///
/// A terminal task missing `ended_at` is treated as not current and
/// dropped: durable tasks always stamp `ended_at` when they reach a
/// terminal state, so absence of it indicates a record from a much
/// older schema and isn't worth surfacing.
pub(crate) fn select_work_sidebar_tasks(
    tasks: Vec<TaskSummary>,
    session_started_at: chrono::DateTime<chrono::Utc>,
) -> Vec<TaskSummary> {
    tasks
        .into_iter()
        .filter(|task| match task.status {
            TaskStatus::Queued | TaskStatus::Running => true,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled => {
                task.created_at >= session_started_at
                    && task
                        .ended_at
                        .is_some_and(|ended_at| ended_at >= session_started_at)
            }
        })
        .collect()
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
    let mut entries: Vec<TaskPanelEntry> = select_work_sidebar_tasks(tasks, session_started_at)
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
    if jobs.is_empty() {
        return false;
    }

    let mut changed = false;
    for index in 0..app.virtual_cell_count() {
        let Some((task_id, next_status, next_live, next_duration, finalized)) =
            shell_exec_live_update(app, index, &jobs)
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
        if finalized {
            exec.output = next_live;
            exec.output_summary = exec
                .output
                .as_deref()
                .map(super::history::summarize_tool_output);
            exec.live_output = None;
        } else {
            exec.live_output = next_live;
        }
        changed = true;
    }
    changed
}

fn shell_exec_live_update(
    app: &App,
    index: usize,
    jobs: &std::collections::HashMap<String, ShellJobSnapshot>,
) -> Option<(String, ToolStatus, Option<String>, u64, bool)> {
    let HistoryCell::Tool(ToolCell::Exec(exec)) = app.cell_at_virtual_index(index)? else {
        return None;
    };
    if exec.output.is_some() {
        return None;
    }
    let task_id = exec.shell_task_id.as_deref()?;
    let job = jobs.get(task_id)?;
    let next_status = shell_job_tool_status(&job.status);
    let next_live = shell_job_live_output(job).or_else(|| exec.live_output.clone());
    let finalized = !matches!(job.status, ShellStatus::Running);
    if exec.status == next_status
        && exec.live_output == next_live
        && exec.duration_ms == Some(job.elapsed_ms)
    {
        return None;
    }
    Some((
        task_id.to_string(),
        next_status,
        next_live,
        job.elapsed_ms,
        finalized,
    ))
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

/// Fetch the DeepSeek account balance from the balance API.
///
/// Returns `None` on any error (network, auth, parse) — callers should treat
/// a `None` return as "balance unknown" and keep the previous value.
async fn fetch_deepseek_balance(
    api_key: &str,
    base_url: &str,
) -> Option<crate::pricing::BalanceInfo> {
    let url = format!("{}/user/balance", base_url.trim_end_matches('/'));
    let client = &*BALANCE_CLIENT;
    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        tracing::debug!(
            "balance API returned {}: {}",
            response.status().as_u16(),
            response.text().await.unwrap_or_default()
        );
        return None;
    }
    let body: crate::pricing::BalanceResponse = response.json().await.ok()?;
    // Return the first balance entry (typically the user's primary currency).
    body.balance_infos.into_iter().next()
}

fn should_fetch_deepseek_balance(app: &App) -> bool {
    app.status_items.contains(&StatusItem::Balance)
        && matches!(
            app.api_provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN
        )
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn run_event_loop(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    mut engine_handle: EngineHandle,
    task_manager: SharedTaskManager,
    event_broker: &EventBroker,
    translation_client: Option<Arc<DeepSeekClient>>,
    mut dispatch_completion_rx: tokio::sync::mpsc::UnboundedReceiver<
        crate::tui::app::DispatchApplyFn,
    >,
) -> Result<()> {
    // Track streaming state
    let mut current_streaming_text = String::new();
    let mut stream_display_clock = StreamDisplayClock::default();
    let (translation_tx, mut translation_rx) =
        tokio::sync::mpsc::unbounded_channel::<TranslationEvent>();
    let mut pending_translations = 0usize;
    let mut pending_thinking_translations = 0usize;
    let mut last_queue_state = (app.queued_messages.clone(), app.queued_draft.clone());
    let mut last_queue_was_empty = app.queued_messages.is_empty() && app.queued_draft.is_none();
    let mut last_task_refresh = Instant::now()
        .checked_sub(Duration::from_secs(2))
        .unwrap_or_else(Instant::now);
    let mut last_status_frame = Instant::now()
        .checked_sub(Duration::from_millis(UI_STATUS_ANIMATION_MS))
        .unwrap_or_else(Instant::now);
    // 120 FPS draw cap. Without this we redraw on every SSE chunk during a
    // long stream — wasted work the user can't perceive. See
    // `tui::frame_rate_limiter` for the rationale; ports the small piece of
    // codex's frame coalescing that maps cleanly onto our poll-based loop.
    let mut frame_rate_limiter = crate::tui::frame_rate_limiter::FrameRateLimiter::default();
    // Widgets request future animation frames here; the poll loop remains the
    // sole `terminal.draw` emitter (no competing animation loop).
    let mut frame_requester = FrameRequester::new();
    let mut web_config_session: Option<WebConfigSession> = None;
    let mut prev_input_snapshot = String::new();
    let mut terminal_paused_at: Option<Instant> = None;
    let mut force_terminal_repaint = false;
    // FocusGained debounce: some terminal emulators (e.g. Tabby) re-trigger
    // FocusGained when we re-arm focus-change reporting inside
    // recover_terminal_modes, creating a tight repaint loop. Skip
    // mode recovery (but still mark a repaint) within the debounce window.
    const FOCUS_RECOVERY_DEBOUNCE: Duration = Duration::from_millis(200);
    let mut last_focus_recovery = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);
    let mut terminal_input = TerminalInputPump::spawn()?;
    let mut pending_terminal_events: VecDeque<Event> = VecDeque::new();
    let mut last_terminal_input_recovery = Instant::now()
        .checked_sub(TERMINAL_INPUT_RECOVERY_COOLDOWN)
        .unwrap_or_else(Instant::now);
    let mut last_recovery_snapshot_at: Option<Instant> = None;

    // Fire-and-forget version check — runs once per session in the
    // background. On success, a short status toast advertises the update
    // without replacing the user's configured footer/status-line chips.
    let mut version_check: Option<tokio::task::JoinHandle<Option<String>>> =
        spawn_startup_version_check(config.update_config());

    // Fire a one-shot initial balance fetch for DeepSeek providers
    // so the footer chip shows balance on the first frame without
    // waiting for a turn to complete.
    if !app.balance_initiated && should_fetch_deepseek_balance(app) {
        let cell = app.balance_cell.clone();
        let api_key = config.deepseek_api_key().unwrap_or_default();
        let base_url = config.deepseek_base_url();
        if !api_key.is_empty() {
            app.last_balance_fetch = Some(Instant::now());
            tokio::spawn(async move {
                if let Some(info) = fetch_deepseek_balance(&api_key, &base_url).await
                    && let Ok(mut guard) = cell.lock()
                {
                    *guard = Some(info);
                }
            });
        }
        app.balance_initiated = true;
    }

    let mut pending_subagent_list_refresh = false;

    loop {
        // Drain dispatch completions from spawned send tasks (#4605). The
        // closure receives `&mut App` and applies success state or rollback.
        while let Ok(apply) = dispatch_completion_rx.try_recv() {
            let _ = apply(app, &engine_handle, &*config);
        }

        // Drain the version-check handle once; re-assign None so we
        // don't poll it again.
        let mut done = false;
        if let Some(ref handle) = version_check {
            done = handle.is_finished();
        }
        if done && let Ok(Some(hint)) = version_check.take().unwrap().await {
            app.push_status_toast(
                hint,
                StatusToastLevel::Info,
                Some(VERSION_HINT_TOAST_TTL_MS),
            );
        }

        if !drain_web_config_events(&mut web_config_session, app, config, &engine_handle).await {
            web_config_session = None;
        }

        while let Ok(event) = translation_rx.try_recv() {
            match event {
                TranslationEvent::AssistantMessage {
                    history_index,
                    original_text,
                    translated,
                    thinking,
                    tool_uses,
                } => {
                    pending_translations = pending_translations.saturating_sub(1);
                    pending_thinking_translations = pending_thinking_translations.saturating_sub(1);
                    let text = match translated {
                        Ok(text) => {
                            app.status_message = Some(
                                crate::localization::tr(
                                    app.ui_locale,
                                    crate::localization::MessageId::TranslationComplete,
                                )
                                .to_string(),
                            );
                            text
                        }
                        Err(err) => {
                            tracing::warn!("assistant translation failed: {err}");
                            app.status_message = Some(format!(
                                "{}: {err}",
                                crate::localization::tr(
                                    app.ui_locale,
                                    crate::localization::MessageId::TranslationFailed,
                                )
                            ));
                            crate::localization::hidden_translation_failed(app.ui_locale)
                                .to_string()
                        }
                    };

                    if let Some(index) = history_index
                        && let Some(HistoryCell::Assistant { content, .. }) =
                            app.history.get_mut(index)
                    {
                        *content = text.clone();
                        app.bump_history_cell(index);
                    }
                    if !replace_matching_assistant_text(app, &original_text, text.clone()) {
                        push_assistant_message(app, text, thinking, tool_uses);
                    }
                    if pending_translations == 0
                        && !matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
                    {
                        app.is_loading = pending_translations > 0;
                    }
                    app.needs_redraw = true;
                }
                TranslationEvent::Thinking {
                    placeholder,
                    translated,
                } => {
                    pending_translations = pending_translations.saturating_sub(1);
                    let text = match translated {
                        Ok(text) => {
                            app.status_message = Some(
                                crate::localization::thinking_translation_complete(app.ui_locale)
                                    .to_string(),
                            );
                            text
                        }
                        Err(err) => {
                            tracing::warn!("thinking translation failed: {err}");
                            app.status_message = Some(format!(
                                "{}: {err}",
                                crate::localization::thinking_translation_failed(app.ui_locale)
                            ));
                            crate::localization::hidden_translation_failed(app.ui_locale)
                                .to_string()
                        }
                    };
                    streaming_thinking::replace_pending_translation(app, &placeholder, text);
                    if pending_translations == 0
                        && !matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
                    {
                        app.is_loading = false;
                    }
                    app.needs_redraw = true;
                }
            }
        }

        if last_task_refresh.elapsed() >= Duration::from_millis(2500) {
            if refresh_active_task_panel(app, &task_manager).await {
                app.needs_redraw = true;
            }
            if refresh_shell_exec_live_output(app) {
                app.needs_redraw = true;
            }
            if app
                .runtime_services
                .work
                .as_ref()
                .is_some_and(|work| work.has_pending_publish())
                && let Err(err) = persist_pending_work_checkpoint(app).await
            {
                tracing::warn!(error = %err, "background Work lifecycle checkpoint remains pending");
            }
            last_task_refresh = Instant::now();
        }

        // Clear suggestion when the user modifies the input.
        if app.input != prev_input_snapshot {
            app.prompt_suggestion = None;
            prev_input_snapshot = app.input.clone();
        }

        // Poll prompt suggestion cell from background generation task.
        // Discard stale results whose generation token no longer matches.
        if let Ok(mut guard) = app.prompt_suggestion_cell.try_lock()
            && let Some((gen_token, suggestion)) = guard.take()
            && gen_token
                == app
                    .prompt_suggestion_gen
                    .load(std::sync::atomic::Ordering::Relaxed)
        {
            app.prompt_suggestion = Some(suggestion);
        }

        // Poll the fleet-profile model-draft cell filled by the background
        // drafting task (#3757 review: the draft must not park the loop).
        let fleet_draft_delivery = app
            .fleet_draft_cell
            .try_lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some((draft_gen, model_label, picked_route, reasoning_effort, outcome)) =
            fleet_draft_delivery
            && draft_gen == app.current_draft_gen()
        {
            deliver_fleet_draft_result(
                app,
                model_label,
                picked_route,
                reasoning_effort,
                outcome,
                app.ui_locale,
            );
        }

        // Poll the constitution model-draft cell (same background pattern).
        let constitution_draft_delivery = app
            .constitution_draft_cell
            .try_lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some((draft_gen, model_label, draft_locale, outcome)) = constitution_draft_delivery
            && draft_gen == app.current_draft_gen()
        {
            deliver_constitution_draft_result(app, model_label, draft_locale, outcome);
        }

        // #1830/#2317: service any already-arrived terminal keys before a
        // potentially long engine batch so composer/modal input stays live.
        collect_pending_terminal_events(&terminal_input, &mut pending_terminal_events)?;

        // First, poll for engine events (non-blocking)
        let mut received_engine_event = false;
        let mut transcript_batch_updated = false;
        // #freeze: coalesce per-event `Op::ListSubAgents` sends into a single
        // trailing-edge refresh per drain. At high fanout, many spawn/complete/
        // mailbox events in one drain otherwise each take the manager write
        // lock and trigger a full O(N) list reconcile.
        let mut subagent_list_refresh_requested = false;
        let mut queued_to_send: Option<QueuedMessage> = None;
        let mut respawn_after_provider_rollback: Option<String> = None;
        let mut fallback_after_engine_error: Option<ProviderFallbackRollback> = None;
        {
            let mut rx = engine_handle.rx_event.write().await;
            let mut progress_redraw_agents: HashSet<String> = HashSet::new();
            let drain_started = Instant::now();
            let mut events_drained = 0usize;
            loop {
                if events_drained > 0
                    && engine_drain_budget_exhausted(events_drained, drain_started, Instant::now())
                {
                    break;
                }
                let event = match rx.try_recv() {
                    Ok(event) => event,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        if recover_engine_event_disconnect(app) {
                            received_engine_event = true;
                            transcript_batch_updated = true;
                        }
                        break;
                    }
                };
                // #3033: remember whether an EARLIER event in this drain batch
                // already requested a redraw. The AgentProgress throttle below
                // may opt the current event out of repainting, but it must not
                // cancel redraws owed to other events in the same batch.
                let redraw_requested_before_event = received_engine_event;
                received_engine_event = true;
                capture_turn_started_metadata(app, &event);
                if app.suppress_stream_events_until_turn_complete {
                    if matches!(event, EngineEvent::TurnStarted { .. }) {
                        // Ctrl+C can race with the engine's per-turn token
                        // reset: the first cancel may hit the previous token
                        // if SendMessage is queued but TurnStarted has not
                        // arrived yet. Reassert cancellation once the real
                        // turn starts, then keep hiding its queued deltas.
                        engine_handle.cancel();
                        continue;
                    }
                    if suppress_engine_event_after_local_cancel(&event) {
                        continue;
                    }
                } else if !app.is_loading && ignore_stale_stream_event_while_idle(&event) {
                    continue;
                }
                record_turn_activity(app, &event, Instant::now());
                match event {
                    EngineEvent::MessageStarted { .. } => {
                        // Assistant text starting after parallel tool work
                        // means the tool group is done. Flush the active
                        // cell first so the message lands BELOW the
                        // committed tool group (Codex pattern: streamed
                        // assistant content always flows after work).
                        app.flush_active_cell();
                        current_streaming_text.clear();
                        app.streaming_output_token_estimate = 0;
                        app.streaming_state.reset();
                        app.streaming_state.start_text(0, None);
                        app.streaming_message_index = None;
                        stream_display_clock.reset();
                    }
                    EngineEvent::MessageDelta { content, .. } => {
                        let sanitized = sanitize_stream_chunk(&content);
                        if sanitized.is_empty() {
                            continue;
                        }
                        // First delta of a fresh stream has no streaming
                        // cell yet; flush active so the tool group settles
                        // before the assistant prose appears below it.
                        if app.streaming_message_index.is_none() {
                            app.flush_active_cell();
                        }
                        current_streaming_text.push_str(&sanitized);
                        ensure_streaming_assistant_history_cell(app);
                        app.streaming_state.push_content(0, &sanitized);
                        stream_display_clock.note_delta(Instant::now());
                        received_engine_event = redraw_requested_before_event;
                    }
                    EngineEvent::MessageComplete { .. } => {
                        // #861 RC3: defensive drain of a still-active thinking
                        // entry. Normally `ThinkingComplete` arrives first and
                        // populates `last_reasoning` before we get here, but
                        // when the engine bursts events the channel can
                        // deliver `MessageComplete` first, in which case
                        // `last_reasoning.take()` below would be `None` and
                        // the thinking block would be dropped from
                        // `api_messages` — causing a DeepSeek HTTP 400 on the
                        // next turn (V4 thinking-mode requires
                        // `reasoning_content` replay). Inline-finalize the
                        // thinking entry here so this branch is order-
                        // independent.
                        if app.streaming_thinking_active_entry.is_some() {
                            if streaming_thinking::finalize_current(app) {
                                transcript_batch_updated = true;
                            }
                            streaming_thinking::stash_reasoning_buffer_into_last_reasoning(app);
                        }
                        let mut completed_message_index = None;
                        if let Some(index) = app.streaming_message_index.take() {
                            completed_message_index = Some(index);
                            stream_display_clock.flush_now(Instant::now());
                            let remaining = app.streaming_state.finalize_block_text(0);
                            if !remaining.is_empty() {
                                append_streaming_text(app, index, &remaining);
                                accrue_streaming_token_estimate(app, &remaining);
                            }
                            if let Some(HistoryCell::Assistant { streaming, .. }) =
                                app.history.get_mut(index)
                            {
                                *streaming = false;
                            }
                            // Streaming flag flipped — the cell's compact /
                            // transcript variants render slightly
                            // differently, so bump its revision so the cache
                            // refreshes this row only.
                            app.bump_history_cell(index);
                            transcript_batch_updated = true;
                            stream_display_clock.reset();
                        }

                        let thinking = app.last_reasoning.take();
                        let tool_uses = app.pending_tool_uses.drain(..).collect::<Vec<_>>();
                        let history_index = completed_message_index;

                        if app.translation_enabled
                            && !current_streaming_text.is_empty()
                            && crate::tui::translation::needs_translation(&current_streaming_text)
                            && let Some(translation_client) = translation_client.as_ref()
                        {
                            app.status_message = Some(
                                crate::localization::tr(
                                    app.ui_locale,
                                    crate::localization::MessageId::TranslationInProgress,
                                )
                                .to_string(),
                            );
                            app.is_loading = true;
                            pending_translations = pending_translations.saturating_add(1);
                            let tx = translation_tx.clone();
                            let client = translation_client.clone();
                            let original_text = current_streaming_text.clone();
                            let translation_model = app
                                .last_effective_model
                                .clone()
                                .unwrap_or_else(|| app.model.clone());
                            let target_language =
                                app.ui_locale.translation_target_name().to_string();
                            tokio::spawn(async move {
                                let translated = crate::tui::translation::translate_text(
                                    &original_text,
                                    &client,
                                    &translation_model,
                                    &target_language,
                                )
                                .await;
                                let _ = tx.send(TranslationEvent::AssistantMessage {
                                    history_index,
                                    original_text,
                                    translated,
                                    thinking,
                                    tool_uses,
                                });
                            });
                        } else {
                            push_assistant_message(
                                app,
                                current_streaming_text.clone(),
                                thinking,
                                tool_uses,
                            );
                        }
                    }
                    EngineEvent::ThinkingStarted { .. } => {
                        stream_display_clock.reset();
                        // P2.3: thinking lives in the active cell so it groups
                        // visually with the tool calls that follow until the
                        // next assistant prose chunk flushes the group.
                        if streaming_thinking::start_block(app) {
                            transcript_batch_updated = true;
                        }
                        if app.translation_enabled {
                            let entry_idx = streaming_thinking::ensure_active_entry(app);
                            streaming_thinking::set_placeholder(app, entry_idx);
                            transcript_batch_updated = true;
                        }
                    }
                    EngineEvent::ThinkingDelta { content, .. } => {
                        let sanitized = sanitize_stream_chunk(&content);
                        if sanitized.is_empty() {
                            continue;
                        }
                        app.reasoning_buffer.push_str(&sanitized);
                        if app.reasoning_header.is_none() {
                            app.reasoning_header = extract_reasoning_header(&app.reasoning_buffer);
                        }

                        streaming_thinking::ensure_active_entry(app);
                        app.streaming_state.push_content(0, &sanitized);
                        stream_display_clock.note_delta(Instant::now());
                        received_engine_event = redraw_requested_before_event;
                    }
                    EngineEvent::ThinkingComplete { .. } => {
                        stream_display_clock.flush_now(Instant::now());
                        if app.translation_enabled {
                            let original_thinking = app.reasoning_buffer.clone();
                            let _ = app.streaming_state.finalize_block_text(0);
                            let duration = app
                                .thinking_started_at
                                .take()
                                .map(|t| t.elapsed().as_secs_f32());
                            if streaming_thinking::finalize_active_entry(app, duration, "") {
                                transcript_batch_updated = true;
                            }
                            if !original_thinking.is_empty()
                                && crate::tui::translation::needs_translation(&original_thinking)
                                && let Some(translation_client) = translation_client.as_ref()
                            {
                                app.status_message = Some(
                                    crate::localization::thinking_translation_in_progress(
                                        app.ui_locale,
                                    )
                                    .to_string(),
                                );
                                app.is_loading = true;
                                pending_translations = pending_translations.saturating_add(1);
                                pending_thinking_translations =
                                    pending_thinking_translations.saturating_add(1);
                                let tx = translation_tx.clone();
                                let client = translation_client.clone();
                                let translation_model = app
                                    .last_effective_model
                                    .clone()
                                    .unwrap_or_else(|| app.model.clone());
                                let placeholder =
                                    crate::localization::thinking_translation_placeholder(
                                        app.ui_locale,
                                    )
                                    .to_string();
                                let target_language =
                                    app.ui_locale.translation_target_name().to_string();
                                tokio::spawn(async move {
                                    let translated = crate::tui::translation::translate_text(
                                        &original_thinking,
                                        &client,
                                        &translation_model,
                                        &target_language,
                                    )
                                    .await;
                                    let _ = tx.send(TranslationEvent::Thinking {
                                        placeholder,
                                        translated,
                                    });
                                });
                            } else {
                                let placeholder =
                                    crate::localization::thinking_translation_placeholder(
                                        app.ui_locale,
                                    );
                                streaming_thinking::replace_pending_translation(
                                    app,
                                    placeholder,
                                    original_thinking,
                                );
                            }
                        } else if streaming_thinking::finalize_current(app) {
                            transcript_batch_updated = true;
                        }
                        streaming_thinking::stash_reasoning_buffer_into_last_reasoning(app);
                        stream_display_clock.reset();
                    }
                    EngineEvent::ToolCallStarted { id, name, input } => {
                        app.pending_tool_uses
                            .push((id.clone(), name.clone(), input.clone()));
                        // Note this dispatch so the next sub-agent `Started`
                        // mailbox envelope routes into the right card kind
                        // (delegate vs fanout).
                        if matches!(
                            name.as_str(),
                            "agent" | "rlm_open" | "rlm_eval" | "rlm" | "delegate"
                        ) {
                            app.pending_subagent_dispatch = Some(name.clone());
                            if matches!(name.as_str(), "rlm_open" | "rlm_eval" | "rlm") {
                                // New fanout invocation — children should
                                // group under a fresh card, not the
                                // previous fanout's leftover.
                                app.last_fanout_card_index = None;
                            }
                        }
                        handle_tool_call_started(app, &id, &name, &input);
                    }
                    // Liveness only. `record_turn_activity` above consumes the
                    // pulse; it must not alter transcript or status copy.
                    EngineEvent::ToolCallHeartbeat => {}
                    EngineEvent::ToolCallComplete { id, name, result } => {
                        if crate::tui::tool_routing::evidence_completion_should_be_ignored(
                            app, &id, &result,
                        ) {
                            tracing::debug!(tool_id = %id, tool_name = %name, "ignored foreign or replayed evidence completion");
                            continue;
                        }
                        if name == "update_plan" {
                            app.plan_tool_used_in_turn = true;
                        }
                        if is_model_visible_tool_call(&id) {
                            let tool_content = match &result {
                                Ok(output) => sanitize_stream_chunk(
                                    &tool_result_content_for_api_message(app, &id, &name, output)
                                        .await,
                                ),
                                Err(err) => sanitize_stream_chunk(&format!("Error: {err}")),
                            };
                            app.api_messages.push(Message {
                                role: "user".to_string(),
                                content: vec![ContentBlock::ToolResult {
                                    tool_use_id: id.clone(),
                                    content: tool_content,
                                    is_error: None,
                                    content_blocks: None,
                                }],
                            });
                        } else {
                            app.pending_tool_uses
                                .retain(|(tool_id, _, _)| tool_id != &id);
                        }
                        handle_tool_call_complete(app, &id, &name, &result);
                        if crate::mcp::McpPool::is_mcp_tool(&name)
                            && match &result {
                                Ok(output) => !output.success,
                                Err(_) => true,
                            }
                        {
                            let _ = app.maybe_show_behavioral_tip(
                                crate::tui::behavioral_tips::BehavioralTip::McpValidation,
                            );
                        }

                        if result.is_ok()
                            && is_work_graph_mutation_tool(&name)
                            && let Err(err) = persist_pending_work_checkpoint(app).await
                        {
                            tracing::warn!(
                                tool = %name,
                                error = %err,
                                "Work Graph checkpoint was not enqueued; projections remain unpublished"
                            );
                            app.status_message = Some(format!(
                                "Work update is pending: checkpoint could not be queued ({err})"
                            ));
                        }

                        // Immediately refresh the task panel sidebar when a
                        // tool that changes task state completes, so the
                        // Tasks panel stays in sync with tool execution
                        // rather than waiting up to 2.5 s for the periodic
                        // poll. Also merge shell jobs (#373).
                        // Only tools that actually change durable tasks or
                        // background shell jobs force a jobs-panel refresh.
                        // Checklist/todo/plan tools drive the To-do panel,
                        // which reads `app.todos` directly and repaints on the
                        // normal redraw — no forced refresh needed (avoids the
                        // old per-checklist Tasks-panel churn).
                        if matches!(
                            name.as_str(),
                            "agent"
                                | "task_shell_start"
                                | "exec_shell"
                                | "exec_shell_cancel"
                                | "exec_shell_wait"
                                | "task_cancel"
                                // Unified durable-task tool (piagent phase B):
                                // create/cancel actions mutate task state, so
                                // any `tasks` completion refreshes the panel.
                                | "tasks"
                        ) {
                            refresh_active_task_panel(app, &task_manager).await;
                            last_task_refresh = Instant::now();
                        }
                        if matches!(name.as_str(), "agent") {
                            subagent_list_refresh_requested = true;
                        }
                    }
                    EngineEvent::TurnStarted { turn_id, .. } => {
                        app.ocean_completion_started_at = None;
                        app.ocean_receipt_settle_start = None;
                        app.ocean_turn_history_start = app.history.len();
                        app.suppress_stream_events_until_turn_complete = false;
                        app.is_loading = true;
                        app.offline_mode = false;
                        app.turn_error_posted = false;
                        app.lsp_repair = crate::tui::app::LspRepairState::default();
                        app.prompt_suggestion = None;
                        app.prompt_suggestion_gen
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        app.dispatch_started_at = None;
                        current_streaming_text.clear();
                        app.streaming_output_token_estimate = 0;
                        app.streaming_state.reset();
                        app.streaming_message_index = None;
                        app.streaming_thinking_active_entry = None;
                        stream_display_clock.reset();
                        let now = Instant::now();
                        app.turn_started_at = Some(now);
                        app.turn_last_activity_at = Some(now);
                        app.session.last_output_throughput = None;
                        app.streaming_output_token_estimate = 0;
                        app.provider_wait_incident_logged = false;
                        // Discoverability hint for users who don't know how
                        // to interrupt a long-running turn (#1367). Only
                        // surface when the status_message slot is empty so
                        // we don't trample over a real transient message
                        // (e.g. "/queue saved", "Selection copied"); the
                        // hint then auto-clears as soon as anything else
                        // updates the slot.
                        if app.status_message.is_none() {
                            app.status_message = Some("Press Esc or Ctrl+C to cancel".to_string());
                        }
                        app.runtime_turn_id = Some(turn_id);
                        app.runtime_turn_status = Some("in_progress".to_string());
                        app.turn_counter = app.turn_counter.saturating_add(1);
                        app.reasoning_buffer.clear();
                        app.reasoning_header = None;
                        app.last_reasoning = None;
                        app.pending_tool_uses.clear();
                        app.plan_tool_used_in_turn = false;
                        last_status_frame = Instant::now();
                    }
                    EngineEvent::TurnComplete {
                        usage,
                        status,
                        error,
                        tool_catalog,
                        base_url,
                    } => {
                        let completed_turn = app.active_turn.take();
                        let billing_surface = completed_turn
                            .as_ref()
                            .and_then(|turn| turn.route.as_ref())
                            .and_then(|route| {
                                crate::pricing::billing_surface_for_route(
                                    route.provider,
                                    base_url.as_deref(),
                                )
                            });
                        app.session.last_tool_catalog = tool_catalog;
                        app.session.last_base_url = base_url;
                        let was_locally_cancelled = app.suppress_stream_events_until_turn_complete;
                        app.suppress_stream_events_until_turn_complete = false;
                        app.active_allowed_tools = None;
                        if app.paused_quarry.is_none() {
                            app.pausable = false;
                            app.paused = false;
                        }
                        // Turn completion is an ordinary state transition.
                        // Clearing all 7,900 cells after a long stream was the
                        // visible end-of-turn flash in the rejected build.
                        // Ratatui's diff is sufficient here; full repaints stay
                        // reserved for real terminal boundary changes (resize,
                        // focus recovery, theme, child-terminal return).
                        // Finalize any in-flight tool group. Cancellation
                        // marks still-running entries as Failed so the user
                        // sees they were interrupted rather than the spinner
                        // hanging forever.
                        if matches!(
                            status,
                            crate::core::events::TurnOutcomeStatus::Interrupted
                                | crate::core::events::TurnOutcomeStatus::Failed
                        ) {
                            app.finalize_active_cell_as_interrupted();
                            // Also mark the streaming Assistant cell (if any)
                            // so partial reasoning/text isn't left with a
                            // permanent spinner. Idempotent with the
                            // optimistic call in the Esc handler.
                            app.finalize_streaming_assistant_as_interrupted();
                        } else {
                            app.flush_active_cell();
                        }
                        app.is_loading = false;
                        app.dispatch_started_at = None;
                        app.pending_provider_switch = None;
                        app.offline_mode = false;
                        app.streaming_state.reset();
                        stream_display_clock.reset();
                        if was_locally_cancelled {
                            current_streaming_text.clear();
                        }
                        // Capture elapsed before clearing turn_started_at so
                        // notifications can use the real wall-clock duration.
                        let turn_elapsed =
                            app.turn_started_at.map(|t| t.elapsed()).unwrap_or_default();
                        app.turn_started_at = None;
                        app.turn_last_activity_at = None;
                        app.streaming_output_token_estimate = 0;
                        // Roll the just-finished turn's elapsed time into the
                        // cumulative session work-time (#448 follow-up). The
                        // footer's `worked Nh Mm` chip reads this so the
                        // label reflects actual model work, not idle
                        // uptime since launch.
                        app.cumulative_turn_duration =
                            app.cumulative_turn_duration.saturating_add(turn_elapsed);
                        // Stream lock applies per-turn; clear it so the next
                        // turn's chunks pull the view down again until the
                        // user opts out by scrolling up.
                        app.user_scrolled_during_stream = false;
                        app.runtime_turn_status = Some(match status {
                            crate::core::events::TurnOutcomeStatus::Completed => {
                                app.ocean_completion_started_at = Some(Instant::now());
                                app.ocean_receipt_settle_start =
                                    Some(app.ocean_turn_history_start.min(app.history.len()));
                                "completed".to_string()
                            }
                            crate::core::events::TurnOutcomeStatus::Interrupted => {
                                app.ocean_completion_started_at = None;
                                app.ocean_receipt_settle_start = None;
                                "interrupted".to_string()
                            }
                            crate::core::events::TurnOutcomeStatus::Failed => {
                                app.ocean_completion_started_at = None;
                                app.ocean_receipt_settle_start = None;
                                "failed".to_string()
                            }
                        });
                        if matches!(
                            status,
                            crate::core::events::TurnOutcomeStatus::Interrupted
                                | crate::core::events::TurnOutcomeStatus::Failed
                        ) {
                            subagent_list_refresh_requested = true;
                        }
                        crate::tui::notifications::clear_taskbar_progress();
                        if status != crate::core::events::TurnOutcomeStatus::Completed {
                            crate::retry_status::clear();
                            crate::tui::notifications::stop_title_animation_quietly();
                        }
                        let turn_tokens = usage.input_tokens + usage.output_tokens;
                        app.session.total_tokens =
                            app.session.total_tokens.saturating_add(turn_tokens);
                        app.session.total_conversation_tokens = app
                            .session
                            .total_conversation_tokens
                            .saturating_add(turn_tokens);
                        app.session.total_input_tokens = app
                            .session
                            .total_input_tokens
                            .saturating_add(usage.input_tokens);
                        app.session.total_output_tokens = app
                            .session
                            .total_output_tokens
                            .saturating_add(usage.output_tokens);
                        // Only accumulate cache telemetry when reported.
                        if let Some(hit_tokens) = usage.prompt_cache_hit_tokens {
                            app.session.total_cache_hit_tokens = app
                                .session
                                .total_cache_hit_tokens
                                .saturating_add(hit_tokens);
                            let cache_miss = usage
                                .prompt_cache_miss_tokens
                                .unwrap_or_else(|| usage.input_tokens.saturating_sub(hit_tokens));
                            app.session.total_cache_miss_tokens = app
                                .session
                                .total_cache_miss_tokens
                                .saturating_add(cache_miss);
                        }
                        app.session.last_prompt_tokens = Some(usage.input_tokens);
                        app.session.last_completion_tokens = Some(usage.output_tokens);
                        app.session.last_output_throughput =
                            TokenThroughput::new(u64::from(usage.output_tokens), turn_elapsed);
                        app.session.last_prompt_cache_hit_tokens = usage.prompt_cache_hit_tokens;
                        app.session.last_prompt_cache_miss_tokens = usage.prompt_cache_miss_tokens;
                        app.session.last_reasoning_replay_tokens = usage.reasoning_replay_tokens;
                        let (provider, provider_identity, model, auto_model) = completed_turn
                            .as_ref()
                            .and_then(|turn| turn.route.as_ref())
                            .map(|route| {
                                (
                                    Some(route.provider),
                                    Some(route.provider_identity.clone()),
                                    Some(route.model.clone()),
                                    route.auto_model,
                                )
                            })
                            .unwrap_or((None, None, None, false));
                        let effective_turn_provider = provider.unwrap_or(app.api_provider);
                        let effective_turn_model = model
                            .as_deref()
                            .filter(|model| !model.trim().is_empty())
                            .unwrap_or_else(|| {
                                app.last_effective_model.as_deref().unwrap_or(&app.model)
                            })
                            .to_string();
                        app.last_effective_provider = Some(effective_turn_provider);
                        app.last_effective_provider_identity = provider_identity.clone();
                        if completed_turn
                            .as_ref()
                            .and_then(|turn| turn.route.as_ref())
                            .is_some_and(|route| route.auto_model)
                        {
                            app.last_auto_route_receipt = completed_turn
                                .as_ref()
                                .and_then(|turn| turn.auto_route_receipt.clone());
                        } else if completed_turn
                            .as_ref()
                            .is_some_and(|turn| turn.route.is_some())
                        {
                            app.last_auto_route_receipt = None;
                        }
                        if status == crate::core::events::TurnOutcomeStatus::Completed {
                            app.provider_health.record_success(
                                config,
                                effective_turn_provider,
                                &effective_turn_model,
                            );
                        }
                        if auto_model {
                            app.last_effective_model = Some(effective_turn_model.clone());
                        }
                        app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
                            provider,
                            provider_identity,
                            model,
                            auto_model,
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            cache_hit_tokens: usage.prompt_cache_hit_tokens,
                            cache_miss_tokens: usage.prompt_cache_miss_tokens,
                            reasoning_replay_tokens: usage.reasoning_replay_tokens,
                            recorded_at: Instant::now(),
                        });
                        if let Some(error) = error.as_deref() {
                            // Only show "Turn failed:" in the composer status
                            // area when an EngineEvent::Error has NOT already
                            // posted the same message into the transcript.
                            // Otherwise the error appears twice: once in a
                            // HistoryCell and again as a redundant status line.
                            if !app.turn_error_posted {
                                app.status_message = Some(format!("Turn failed: {error}"));
                            }
                        }

                        // Update session cost
                        let turn_cost = completed_turn
                            .as_ref()
                            .and_then(|turn| {
                                turn.route.as_ref().map(|route| (turn.created_at, route))
                            })
                            .and_then(|(created_at, route)| {
                                let billing =
                                    crate::route_billing::for_route(config, route.provider);
                                if !billing.shows_money() {
                                    return None;
                                }
                                crate::pricing::calculate_turn_cost_estimate_for_route_at(
                                    route.provider,
                                    &route.model,
                                    billing_surface,
                                    &usage,
                                    created_at,
                                )
                            });
                        if let Some(cost) = turn_cost {
                            app.accrue_session_cost_estimate(cost);
                        }

                        // Emit OSC 9 / BEL desktop notification for long turns, and
                        // always stop the title animation that began on TurnStarted.
                        if status == crate::core::events::TurnOutcomeStatus::Completed {
                            if let Some((method, threshold, include_summary)) =
                                notifications::settings(config)
                            {
                                let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                                let msg = notifications::completed_turn_message(
                                    app,
                                    &current_streaming_text,
                                    include_summary,
                                    turn_elapsed,
                                    turn_cost,
                                );
                                crate::tui::notifications::notify_done(
                                    method,
                                    in_tmux,
                                    &msg,
                                    threshold,
                                    turn_elapsed,
                                );
                                crate::tui::notifications::stop_title_animation();
                            } else {
                                crate::tui::notifications::stop_title_animation_quietly();
                            }
                        }

                        // Generate ghost-text follow-up suggestion asynchronously.
                        if status == crate::core::events::TurnOutcomeStatus::Completed
                            && config.prompt_suggestion_enabled()
                            && app.api_messages.len() >= 2
                        {
                            let suggestion_cell = app.prompt_suggestion_cell.clone();
                            let api_key = config.deepseek_api_key().unwrap_or_default();
                            let base_url = config.deepseek_base_url();
                            let model = config.default_model();
                            let messages: Vec<crate::models::Message> = app.api_messages.clone();
                            let gen_token = app
                                .prompt_suggestion_gen
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if !api_key.is_empty() {
                                tokio::spawn(async move {
                                    let summary =
                                        crate::tui::prompt_suggestion::summarize_recent_messages(
                                            &messages, 8,
                                        );
                                    if let Some(suggestion) =
                                        crate::tui::prompt_suggestion::generate_suggestion(
                                            &api_key, &base_url, &model, &summary,
                                        )
                                        .await
                                        && let Ok(mut guard) = suggestion_cell.lock()
                                    {
                                        *guard = Some((gen_token, suggestion));
                                    }
                                });
                            }
                        }

                        // Generate post-turn receipt for completed turns.
                        // Also push a persistent status toast so users always
                        // see the outcome in the footer (not just the 8-second
                        // composer receipt), regardless of notification method
                        // or platform.
                        if status == crate::core::events::TurnOutcomeStatus::Completed {
                            // Debt ledger completion-gate: after every completed
                            // turn, check whether there are unresolved entries
                            // the agent should address before claiming the task is
                            // done (#2127). This runs autonomously — no tool call
                            // required — so the agent can't forget to check.
                            if let Ok(ledger) = crate::slop_ledger::SlopLedger::load()
                                && ledger.has_open_entries()
                                && let Some(gate_msg) = ledger.completion_gate_summary()
                            {
                                let short = gate_msg.lines().nth(4).unwrap_or("review before done");
                                app.push_status_toast(
                                    format!("⚠️ Debt ledger: {short}"),
                                    crate::tui::app::StatusToastLevel::Warning,
                                    Some(12_000),
                                );
                            }

                            let tool_count = app.tool_evidence.len();
                            let mut receipt = "✓ turn completed".to_string();
                            if tool_count > 0 {
                                let _ = write!(receipt, " · {tool_count} tool(s) used");
                                for evidence in &app.tool_evidence {
                                    let summary = crate::utils::truncate_with_ellipsis(
                                        &evidence.summary,
                                        60,
                                        "…",
                                    );
                                    let _ = write!(receipt, " · {}: {summary}", evidence.tool_name);
                                }
                            }
                            app.set_receipt_text(receipt.clone());
                            // Mirror as a persistent status toast (10s TTL).
                            // The footer bar visibly shows status toasts,
                            // which is more glanceable than the composer
                            // border receipt alone.
                            app.push_status_toast(
                                receipt,
                                crate::tui::app::StatusToastLevel::Info,
                                Some(10_000),
                            );
                        }

                        // Auto-save completed turn and clear crash checkpoint.
                        // Offloaded to the persistence actor so the UI
                        // stays responsive.
                        let mut completed_snapshot_id: Option<String> = None;
                        if let Ok(manager) = SessionManager::default_location()
                            && let Ok(session) = build_session_snapshot(app, &manager)
                        {
                            app.current_session_id = Some(session.metadata.id.clone());
                            completed_snapshot_id = Some(session.metadata.id.clone());
                            let queued = persistence_actor::try_persist(
                                PersistRequest::SessionSnapshot(session),
                            );
                            if queued {
                                if let Err(err) = publish_pending_work_projection(app).await {
                                    tracing::warn!(
                                        error = %err,
                                        "completed-turn Work projections remain unpublished"
                                    );
                                    app.status_message = Some(format!(
                                        "Session queued, but Work views could not publish ({err})"
                                    ));
                                }
                            } else if app
                                .runtime_services
                                .work
                                .as_ref()
                                .is_some_and(|work| work.has_pending_publish())
                            {
                                app.status_message = Some(
                                    "Work update is pending: session snapshot could not be queued"
                                        .to_string(),
                                );
                            }
                        }
                        if let Some(session_id) = completed_snapshot_id {
                            persistence_actor::persist(PersistRequest::ClearCheckpoint {
                                session_id,
                            });
                        }

                        // Refresh DeepSeek account balance after each completed
                        // turn so the footer balance chip stays current without
                        // adding latency to any request path.
                        let balance_cooldown_expired = app
                            .last_balance_fetch
                            .is_none_or(|t| t.elapsed() >= BALANCE_FETCH_COOLDOWN);
                        if balance_cooldown_expired && should_fetch_deepseek_balance(app) {
                            let cell = app.balance_cell.clone();
                            let api_key = config.deepseek_api_key().unwrap_or_default();
                            let base_url = config.deepseek_base_url();
                            if !api_key.is_empty() {
                                app.last_balance_fetch = Some(Instant::now());
                                tokio::spawn(async move {
                                    if let Some(info) =
                                        fetch_deepseek_balance(&api_key, &base_url).await
                                        && let Ok(mut guard) = cell.lock()
                                    {
                                        *guard = Some(info);
                                    }
                                });
                            }
                        }

                        if app.mode == AppMode::Plan
                            && app.plan_tool_used_in_turn
                            && !app.plan_prompt_pending
                            && app.queued_message_count() == 0
                            && app.queued_draft.is_none()
                        {
                            let review_plan = match app.runtime_services.work.as_ref().map(|work| {
                                work.plan_review_for_confirmation(app.current_session_id.as_deref())
                            }) {
                                Some(Ok(Some((proposal_id, proposed, summary)))) => {
                                    Ok((Some(proposal_id), proposed, Some(summary)))
                                }
                                None => Ok((None, app.plan_state.lock().await.snapshot(), None)),
                                Some(Ok(None)) => {
                                    Ok((None, app.plan_state.lock().await.snapshot(), None))
                                }
                                Some(Err(err)) => Err(err),
                            };
                            match review_plan {
                                Ok((proposal_id, plan, summary)) => {
                                    app.pending_plan_proposal_id = proposal_id;
                                    app.plan_prompt_pending = true;
                                    app.add_message(HistoryCell::System {
                                        content: plan_next_step_prompt(),
                                    });
                                    if app.view_stack.top_kind() != Some(ModalKind::PlanPrompt) {
                                        let todos = Some(app.todos.lock().await.snapshot());
                                        app.view_stack.push(
                                            PlanPromptView::new(Some(plan))
                                                .with_plan_diff_summary(summary)
                                                .with_todos(todos),
                                        );
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(
                                        error = %err,
                                        "validated Plan diff could not be rendered for review"
                                    );
                                    app.plan_prompt_pending = false;
                                    app.pending_plan_proposal_id = None;
                                    let message = format!(
                                        "Plan review is unavailable; no proposal was accepted ({err})"
                                    );
                                    app.status_message = Some(message.clone());
                                    app.add_message(HistoryCell::System { content: message });
                                }
                            }
                        }
                        app.plan_tool_used_in_turn = false;

                        // Legacy pending-steer recovery. Current keyboard
                        // handling keeps Esc as cancel-only, but older saved
                        // state may still carry pending steers.
                        if status == crate::core::events::TurnOutcomeStatus::Interrupted
                            && app.submit_pending_steers_after_interrupt
                        {
                            if let Some(merged) = merge_pending_steers(&mut *app) {
                                queued_to_send = Some(merged);
                            }
                        } else if status == crate::core::events::TurnOutcomeStatus::Failed
                            && !app.pending_steers.is_empty()
                        {
                            // Hard-fail recovery: if the engine failed before
                            // a clean Interrupted landed, demote pending
                            // steers to the visible queue so they're not
                            // silently lost. User can /queue to inspect.
                            for msg in app.drain_pending_steers() {
                                app.queue_message(msg);
                            }
                        }

                        execute_turn_end_observer_hook(
                            app,
                            completed_turn.as_ref(),
                            &usage,
                            billing_surface,
                            turn_elapsed,
                            error.as_deref(),
                        );

                        if queued_to_send.is_none() {
                            queued_to_send = app.pop_queued_message();
                        }
                    }
                    EngineEvent::Error {
                        envelope,
                        recoverable: _,
                    } => {
                        let provider_before_error = app.api_provider;
                        let identity_before_error = ProviderIdentity {
                            provider: provider_before_error,
                            key: app.provider_identity_for_persistence().to_string(),
                            exact_id: app.provider_id_for_persistence().map(str::to_string),
                        };
                        let fallback_chain_before_error = app.provider_chain.clone();
                        let (health_provider, health_model) =
                            error_health_route(app, provider_before_error);
                        app.provider_health.record_failure(
                            config,
                            health_provider,
                            &health_model,
                            &envelope,
                        );
                        let rollback_after_auth_failure =
                            matches!(
                                envelope.category,
                                crate::error_taxonomy::ErrorCategory::Authentication
                            ) && app.pending_provider_switch.is_some();
                        apply_engine_error_to_app(app, envelope);
                        if app.api_provider != provider_before_error && app.is_fallback_active() {
                            // Several queued errors can be drained together.
                            // The first route remains the rollback authority;
                            // later chain advances must not overwrite it with
                            // an enum/key pair from the half-applied fallback.
                            fallback_after_engine_error.get_or_insert(ProviderFallbackRollback {
                                identity: identity_before_error,
                                chain: fallback_chain_before_error,
                            });
                        }
                        if rollback_after_auth_failure
                            && let Some(rollback_warning) =
                                rollback_provider_after_auth_failure(app, config)
                        {
                            respawn_after_provider_rollback = Some(rollback_warning);
                        }
                    }
                    EngineEvent::Status { message } => {
                        app.status_message = Some(message);
                    }
                    EngineEvent::GoalUpdated { snapshot } => {
                        if apply_goal_snapshot_to_app(app, &snapshot) {
                            transcript_batch_updated = true;
                        }
                    }
                    EngineEvent::SessionUpdated {
                        session_id,
                        messages,
                        system_prompt,
                        model,
                        workspace,
                    } => {
                        app.current_session_id = Some(session_id.clone());
                        app.api_messages = messages;
                        app.system_prompt = system_prompt;
                        if app.auto_model {
                            app.last_effective_model = Some(model);
                        } else {
                            app.set_model_selection(model);
                        }
                        app.update_model_compaction_budget();
                        if app.workspace != workspace {
                            apply_workspace_runtime_state(app, config, workspace);
                        }
                        if (app.is_loading || app.is_compacting || app.is_purging)
                            && let Ok(manager) = SessionManager::default_location()
                        {
                            if let Ok(session) = build_session_snapshot(app, &manager) {
                                app.session_title = Some(session.metadata.title.clone());
                                // Pin the id so every checkpoint of this
                                // session lands in the same per-session file
                                // and the eventual clear targets it.
                                if app.current_session_id.is_none() {
                                    app.current_session_id = Some(session.metadata.id.clone());
                                }
                                if let Err(err) = persist_with_pending_work_boundary(
                                    app,
                                    PersistRequest::SaveCheckpoint { session },
                                ) {
                                    app.status_message = Some(format!(
                                        "Work update is pending: checkpoint could not be queued ({err})"
                                    ));
                                }
                            }
                        } else if app.session_title.is_none() {
                            // Never synchronously reload the growing session
                            // JSON on the event-loop task just to recover a
                            // title. The in-memory metadata cache is authoritative.
                            let cached = app
                                .current_session_metadata
                                .as_ref()
                                .filter(|metadata| metadata.id == session_id)
                                .map(|metadata| metadata.title.clone());
                            app.session_title =
                                cached.or_else(|| derive_session_title(&app.api_messages));
                        }
                    }
                    EngineEvent::CompactionStarted { message, .. } => {
                        app.is_compacting = true;
                        app.status_message = Some(message);
                    }
                    EngineEvent::CompactionCompleted { message, .. } => {
                        app.is_compacting = false;
                        app.status_message = Some(message);
                    }
                    EngineEvent::CompactionFailed { message, .. } => {
                        app.is_compacting = false;
                        app.status_message = Some(message);
                    }
                    EngineEvent::PurgeStarted { message } => {
                        app.is_purging = true;
                        app.status_message = Some(message);
                    }
                    EngineEvent::PurgeCompleted { message, .. } => {
                        app.is_purging = false;
                        app.status_message = Some(message);
                    }
                    EngineEvent::PurgeFailed { message } => {
                        app.is_purging = false;
                        app.status_message = Some(message);
                    }
                    EngineEvent::PrefixCacheChange {
                        description,
                        stability_pct,
                        changed,
                        pinned_combined_hash,
                        ..
                    } => {
                        app.prefix_checks_total = app.prefix_checks_total.saturating_add(1);
                        app.prefix_stability_pct = Some(stability_pct);
                        app.last_pinned_prefix_hash =
                            (!pinned_combined_hash.is_empty()).then_some(pinned_combined_hash);
                        if changed {
                            app.prefix_change_count = app.prefix_change_count.saturating_add(1);
                            if !description.is_empty() {
                                app.last_prefix_change_desc = Some(description);
                            }
                        }
                    }
                    EngineEvent::LspRepairUpdate {
                        diagnostics_found,
                        files,
                        injected,
                    } => {
                        let repair = &mut app.lsp_repair;
                        repair.diagnostics_found =
                            repair.diagnostics_found.saturating_add(diagnostics_found);
                        repair.files_touched = repair.files_touched.saturating_add(files);
                        if injected {
                            // Injection itself is not a repair attempt — the model
                            // has only been shown the diagnostics so far (#4107).
                            repair.injected = true;
                            if repair.latest == "unavailable" || repair.latest.is_empty() {
                                repair.latest = "unknown";
                            }
                        } else if repair.injected {
                            // Diagnostics after a prior injection imply the model
                            // edited again (a repair attempt). Zero findings = resolved.
                            repair.repair_attempted = true;
                            repair.latest = if diagnostics_found == 0 {
                                "resolved"
                            } else {
                                "still_failing"
                            };
                        } else {
                            repair.latest = "unknown";
                        }
                    }
                    EngineEvent::PauseEvents { ack } => {
                        if !event_broker.is_paused() {
                            pause_terminal(
                                terminal,
                                app.use_alt_screen,
                                app.use_mouse_capture,
                                app.use_bracketed_paste,
                            )?;
                            event_broker.pause_events();
                            terminal_paused_at = Some(Instant::now());
                        }
                        if let Some(ack) = ack {
                            ack.notify_one();
                        }
                    }
                    EngineEvent::ResumeEvents => {
                        if event_broker.is_paused() {
                            resume_terminal(
                                terminal,
                                app.use_alt_screen,
                                app.use_mouse_capture,
                                app.use_bracketed_paste,
                                app.synchronized_output_enabled,
                            )?;
                            event_broker.resume_events();
                            terminal_paused_at = None;
                        }
                    }
                    EngineEvent::AgentSpawned {
                        id,
                        prompt,
                        parent_run_id,
                        spawn_depth,
                    } => {
                        let prompt_summary = bound_agent_activity_text(&prompt);
                        execute_subagent_observer_hook(
                            app,
                            HookEvent::SubagentSpawn,
                            &id,
                            "prompt",
                            &prompt,
                        );
                        app.agent_progress
                            .insert(id.clone(), format!("starting: {prompt_summary}"));
                        let meta = app.agent_progress_meta.entry(id.clone()).or_default();
                        meta.parent_run_id = parent_run_id;
                        meta.spawn_depth = spawn_depth;
                        meta.current_activity = Some(AgentCurrentActivity::bounded(
                            AgentCurrentActivityStatus::Starting,
                            Some(prompt_summary.clone()),
                            None,
                            None,
                        ));
                        meta.current_tool = None;
                        if app.agent_activity_started_at.is_none() {
                            app.agent_activity_started_at = Some(Instant::now());
                        }
                        // #3030: Assign a stable user-facing label for this
                        // agent and keep the raw id out of the status bar.
                        let label = app.ensure_agent_label(&id);
                        app.status_message = Some(format!("{label} starting: {prompt_summary}"));
                        subagent_list_refresh_requested = true;
                    }
                    EngineEvent::AgentProgress {
                        id,
                        status,
                        activity,
                        parent_run_id,
                        spawn_depth,
                    } => {
                        let display = bound_agent_activity_text(&friendly_subagent_progress(
                            app, &id, &status,
                        ));
                        if is_noisy_subagent_progress(&status) {
                            app.agent_progress
                                .entry(id.clone())
                                .or_insert_with(|| display.clone());
                        } else {
                            app.agent_progress.insert(id.clone(), display.clone());
                        }
                        let meta = app.agent_progress_meta.entry(id.clone()).or_default();
                        meta.parent_run_id = parent_run_id;
                        meta.spawn_depth = spawn_depth;
                        let current_tool = activity
                            .tool_name
                            .as_deref()
                            .map(subagent_progress_tool_display_name)
                            .map(str::to_string);
                        meta.current_activity = Some(AgentCurrentActivity::bounded(
                            activity.worker_status.into(),
                            Some(display.clone()),
                            current_tool.clone(),
                            activity.step,
                        ));
                        meta.current_tool = current_tool;
                        if app.agent_activity_started_at.is_none() {
                            app.agent_activity_started_at = Some(Instant::now());
                        }
                        // #3030: progress can arrive before AgentSpawned is
                        // observed — assign the stable label on first sight.
                        let label = app.ensure_agent_label(&id);
                        app.status_message = Some(format!("{label}: {display}"));
                        // A progress-first agent (its AgentSpawned was dropped
                        // under channel pressure) exists only in agent_progress
                        // until a ListSubAgents refresh promotes it into
                        // subagent_cache. Request that refresh like the
                        // AgentSpawned arm does, so the sidebar row survives
                        // reconciliation instead of flickering out.
                        if !app.subagent_cache.iter().any(|agent| agent.agent_id == id) {
                            subagent_list_refresh_requested = true;
                        }
                        // #3033: Throttle redraws from rapid AgentProgress events.
                        // When 4+ sub-agents are running concurrently, each firing
                        // progress events, the per-event `needs_redraw = true` saturates
                        // the render loop and starves terminal input.  Limit
                        // progress-driven repaints to at most one per 100ms; the
                        // status-animation timer (80ms cadence) provides a guaranteed
                        // floor for sidebar updates.  Data is still recorded immediately;
                        // the sidebar picks it up on the next permitted redraw.
                        if !agent_progress_redraw_permitted_for_drain(
                            &mut app.last_agent_progress_redraw,
                            &mut progress_redraw_agents,
                            &id,
                            Instant::now(),
                        ) {
                            // Restore the pre-event accumulator value: a
                            // throttled progress event contributes no redraw of
                            // its own, but earlier events' redraws survive.
                            received_engine_event = redraw_requested_before_event;
                        }
                    }
                    EngineEvent::AgentComplete { id, result } => {
                        execute_subagent_observer_hook(
                            app,
                            HookEvent::SubagentComplete,
                            &id,
                            "result",
                            &result,
                        );
                        let subagent_elapsed = app
                            .agent_activity_started_at
                            .or(app.turn_started_at)
                            .map(|started| started.elapsed())
                            .unwrap_or_default();
                        let has_other_running_subagents =
                            app.agent_progress.keys().any(|agent_id| agent_id != &id)
                                || app.subagent_cache.iter().any(|agent| {
                                    agent.agent_id != id
                                        && matches!(agent.status, SubAgentStatus::Running)
                                });
                        app.agent_progress.remove(&id);
                        let terminal_status = subagent_status_from_completion_result(&result);
                        let terminal_verb = subagent_terminal_verb(&terminal_status);
                        apply_subagent_terminal_projection(
                            app,
                            &id,
                            terminal_status.clone(),
                            Some(bound_agent_activity_text(&result)),
                        );
                        // #3030: stable label with raw-id fallback.
                        let label = app.agent_display_label(&id);
                        app.status_message = Some(format!(
                            "{label} {terminal_verb}: {}",
                            bound_agent_activity_text(&result)
                        ));
                        let should_recapture_terminal =
                            !has_other_running_subagents && app.use_alt_screen;
                        let subagent_notification_mode =
                            config.notifications_config().subagent_completion;
                        let workflow_tool_running = workflow_tool_is_running(app);
                        if should_notify_subagent_completion(
                            subagent_notification_mode,
                            has_other_running_subagents,
                            workflow_tool_running,
                        ) && let Some((method, threshold, include_summary)) =
                            notifications::settings(config)
                        {
                            let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                            let msg = notifications::subagent_terminal_message(
                                app.ui_locale,
                                &id,
                                &result,
                                &terminal_status,
                                include_summary,
                                subagent_elapsed,
                            );
                            crate::tui::notifications::notify_done(
                                method,
                                in_tmux,
                                &msg,
                                threshold,
                                subagent_elapsed,
                            );
                        }
                        if should_recapture_terminal && event_broker.is_paused() {
                            resume_terminal(
                                terminal,
                                app.use_alt_screen,
                                app.use_mouse_capture,
                                app.use_bracketed_paste,
                                app.synchronized_output_enabled,
                            )?;
                            event_broker.resume_events();
                            terminal_paused_at = None;
                            app.needs_redraw = true;
                        }
                        subagent_list_refresh_requested = true;
                    }
                    EngineEvent::AgentList {
                        agents,
                        coordination,
                    } => {
                        let mut sorted = agents.clone();
                        sort_subagents_in_place(&mut sorted);
                        sorted.retain(|a| !a.from_prior_session);
                        app.subagent_cache = sorted.clone();
                        apply_coordination_detail_projection(app, coordination);
                        reconcile_subagent_activity_state(app);
                        let view_agents = subagent_view_agents(app, &app.subagent_cache);
                        if app.view_stack.update_subagents(&view_agents) {
                            app.status_message =
                                Some(format!("Fleet workers: {} total", view_agents.len()));
                        }
                        // Individual spawn/complete events already log to history;
                        // full list available via /agents command.
                    }
                    EngineEvent::SubAgentMailbox { seq, message } => {
                        let should_refresh_subagents =
                            subagent_message_refreshes_workspace_context(&message);
                        let updated_transcript = handle_subagent_mailbox(app, seq, &message);
                        if let Some((agent_id, status, result)) =
                            subagent_terminal_projection_from_mailbox(&message)
                        {
                            apply_subagent_terminal_projection(app, agent_id, status, result);
                            subagent_list_refresh_requested = true;
                        }
                        if should_refresh_subagents {
                            subagent_list_refresh_requested = true;
                        }
                        if updated_transcript {
                            transcript_batch_updated = true;
                        } else if !should_refresh_subagents
                            && matches!(
                                message,
                                crate::tools::subagent::MailboxMessage::Progress { .. }
                            )
                        {
                            // Progress mailbox envelopes mirror AgentProgress.
                            // When the card state did not visibly change, do
                            // not let the duplicate envelope bypass the
                            // AgentProgress redraw throttle.
                            received_engine_event = redraw_requested_before_event;
                        }
                    }
                    EngineEvent::WorkflowUi { run_id, event } => {
                        // #4122: live typed workflow events → panel + history card.
                        apply_workflow_ui_event(app, &run_id, &event);
                        // #4095 residual: budget_updated is high-frequency under
                        // multi-agent fan-out. Data is already applied; pace the
                        // repaint like AgentProgress so the panel does not churn.
                        let is_budget = event
                            .get("type")
                            .and_then(|v| v.as_str())
                            .is_some_and(|t| t == "budget_updated");
                        if is_budget {
                            if workflow_budget_redraw_permitted(
                                &mut app.last_workflow_budget_redraw,
                                Instant::now(),
                            ) {
                                app.needs_redraw = true;
                            } else {
                                received_engine_event = redraw_requested_before_event;
                            }
                        }
                        transcript_batch_updated = true;
                    }
                    EngineEvent::ApprovalRequired {
                        id,
                        tool_name,
                        description,
                        input,
                        approval_key,
                        approval_grouping_key,
                        intent_summary,
                        approval_force_prompt,
                    } => {
                        use crate::core::authority::ApprovalRequestDisposition;
                        // One disposition path for every ApprovalRequired (#4412):
                        // session denial, Full Access policy hold, session/FA
                        // auto-approve, Never posture, or modal prompt.
                        match resolve_ui_approval_disposition(
                            app,
                            &tool_name,
                            &approval_grouping_key,
                            &approval_key,
                            approval_force_prompt,
                        ) {
                            ApprovalRequestDisposition::AutoDenySessionDenied => {
                                // The user already denied a matching approval key
                                // during this process; auto-deny so the
                                // model's retry loop doesn't keep re-prompting
                                // (#360).
                                auto_deny_session_approval(
                                    app,
                                    &engine_handle,
                                    &id,
                                    &tool_name,
                                    &approval_key,
                                )
                                .await;
                            }
                            ApprovalRequestDisposition::AutoDenyFullAccessPolicyHold => {
                                log_sensitive_event(
                                    "tool.approval.auto_deny_full_access_policy",
                                    serde_json::json!({
                                        "tool_name": tool_name,
                                        "session_id": app.current_session_id,
                                        "mode": app.mode.label(),
                                    }),
                                );
                                let _ = engine_handle.deny_tool_call(id.clone()).await;
                                let notice = app
                                    .tr(MessageId::ApprovalFullAccessPolicyBlocked)
                                    .replace("{tool}", &tool_name);
                                app.push_status_toast(
                                    notice,
                                    StatusToastLevel::Warning,
                                    Some(12_000),
                                );
                            }
                            ApprovalRequestDisposition::AutoApprove => {
                                log_sensitive_event(
                                    "tool.approval.auto_approve_session",
                                    serde_json::json!({
                                        "tool_name": tool_name,
                                        "approval_key": approval_key,
                                        "session_id": app.current_session_id,
                                        "mode": app.mode.label(),
                                    }),
                                );
                                let _ = engine_handle.approve_tool_call(id.clone()).await;
                            }
                            ApprovalRequestDisposition::AutoDenyNeverPosture => {
                                log_sensitive_event(
                                    "tool.approval.auto_deny",
                                    serde_json::json!({
                                        "tool_name": tool_name,
                                        "session_id": app.current_session_id,
                                        "mode": app.mode.label(),
                                    }),
                                );
                                let _ = engine_handle.deny_tool_call(id.clone()).await;
                                app.status_message = Some(format!(
                                    "Blocked tool '{tool_name}' (approval_mode=never)"
                                ));
                            }
                            ApprovalRequestDisposition::Prompt => {
                                let tool_input = input;

                                push_approval_request_view(
                                    app,
                                    &id,
                                    &tool_name,
                                    &description,
                                    &tool_input,
                                    &approval_key,
                                    intent_summary.as_deref(),
                                );
                                log_sensitive_event(
                                    "tool.approval.prompted",
                                    serde_json::json!({
                                        "tool_name": tool_name,
                                        "description": description,
                                        "session_id": app.current_session_id,
                                        "mode": app.mode.label(),
                                    }),
                                );
                                if let Some((method, _, _)) =
                                    crate::tui::notifications::settings(config)
                                {
                                    let in_tmux =
                                        std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                                    crate::tui::notifications::notify_done(
                                        method,
                                        in_tmux,
                                        &format!("Approval needed: {tool_name} - {description}"),
                                        Duration::ZERO,
                                        Duration::ZERO,
                                    );
                                }
                                app.status_message = Some(format!(
                                    "Approval required for '{tool_name}': {description}"
                                ));
                            }
                        }
                    }
                    EngineEvent::UserInputRequired { id, request } => {
                        if should_suppress_user_input_prompt(app) {
                            // A question may have been planned just before the
                            // user switched to Auto-Review. Cancel the stale
                            // request instead of opening a modal under an Auto
                            // header; the tool result tells the model to keep
                            // moving without inventing a user choice.
                            log_sensitive_event(
                                "tool.user_input.auto_cancelled_auto_review",
                                serde_json::json!({
                                    "tool_id": id.clone(),
                                    "session_id": app.current_session_id,
                                }),
                            );
                            let _ = engine_handle.cancel_user_input(id).await;
                            app.pending_user_input_prompt = None;
                            let notice = app.tr(MessageId::AutoReviewQuestionSkipped).into_owned();
                            app.push_status_toast(notice, StatusToastLevel::Info, Some(6_000));
                        } else {
                            app.pending_user_input_prompt = Some((id.clone(), request.clone()));
                            app.view_stack.push(UserInputView::new(id.clone(), request));
                            if let Some((method, _, _)) =
                                crate::tui::notifications::settings(config)
                            {
                                let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                                crate::tui::notifications::notify_done(
                                    method,
                                    in_tmux,
                                    "Action required: please respond in the terminal",
                                    Duration::ZERO,
                                    Duration::ZERO,
                                );
                            }
                            app.status_message = Some(
                                "Action required: answer the popup with 1-4, arrows, or Enter"
                                    .to_string(),
                            );
                        }
                    }
                    EngineEvent::ElevationRequired {
                        tool_id,
                        tool_name,
                        command,
                        denial_reason,
                        blocked_network,
                        blocked_write,
                    } => {
                        // Auto-approved modes may retry denied tools without another prompt.
                        if app_auto_approve_enabled(app) {
                            log_sensitive_event(
                                "tool.sandbox.auto_elevate",
                                serde_json::json!({
                                    "tool_name": tool_name,
                                    "tool_id": tool_id,
                                    "reason": denial_reason,
                                    "session_id": app.current_session_id,
                                }),
                            );
                            app.add_message(HistoryCell::System {
                                content: format!(
                                    "Sandbox denied {tool_name}: {denial_reason} - auto-elevating to full access"
                                ),
                            });
                            // Auto-elevate to full access (no sandbox)
                            let policy = crate::sandbox::SandboxPolicy::DangerFullAccess;
                            let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                        } else {
                            log_sensitive_event(
                                "tool.sandbox.prompt_elevation",
                                serde_json::json!({
                                    "tool_name": tool_name,
                                    "tool_id": tool_id,
                                    "reason": denial_reason,
                                    "session_id": app.current_session_id,
                                }),
                            );
                            // Show elevation dialog
                            let request = ElevationRequest::for_shell(
                                &tool_id,
                                command.as_deref().unwrap_or(&tool_name),
                                &denial_reason,
                                blocked_network,
                                blocked_write,
                            );
                            app.view_stack
                                .push(ElevationView::new(request, app.ui_locale));
                            if let Some((method, _, _)) =
                                crate::tui::notifications::settings(config)
                            {
                                let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                                crate::tui::notifications::notify_done(
                                    method,
                                    in_tmux,
                                    &format!("Sandbox: {denial_reason} for '{tool_name}'"),
                                    Duration::ZERO,
                                    Duration::ZERO,
                                );
                            }
                            app.status_message =
                                Some(format!("Sandbox blocked {tool_name}: {denial_reason}"));
                        }
                    }
                }
                events_drained = events_drained.saturating_add(1);
            }
        }
        if let Some(rollback) = fallback_after_engine_error {
            apply_provider_fallback_switch(app, &mut engine_handle, config, rollback).await;
        }
        if let Some(rollback_warning) = respawn_after_provider_rollback {
            let _ = engine_handle.send(Op::Shutdown).await;
            let engine_config = build_engine_config(app, config);
            engine_handle = spawn_tui_engine(engine_config, config);
            if !app.api_messages.is_empty() {
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id: app.current_session_id.clone(),
                        messages: app.api_messages.clone(),
                        system_prompt: app.system_prompt.clone(),
                        system_prompt_override: false,
                        model: app.model.clone(),
                        workspace: app.workspace.clone(),
                        mode: app.mode,
                    })
                    .await;
            }
            let _ = engine_handle
                .send(Op::SetCompaction {
                    config: app.compaction_config(),
                })
                .await;
            app.status_message = Some(rollback_warning);
        }
        if commit_streaming_display_tick(app, &mut stream_display_clock, Instant::now()) {
            transcript_batch_updated = true;
        }
        if transcript_batch_updated {
            app.mark_history_updated();
        }
        if received_engine_event {
            app.needs_redraw = true;
        }
        if subagent_list_refresh_requested {
            pending_subagent_list_refresh = true;
        }
        // #freeze: one trailing-edge sub-agent list refresh per drain, no
        // matter how many spawn/complete/mailbox events arrived this batch.
        // #3837: keep a sticky pending bit when the op channel is full so a
        // terminal lifecycle event cannot permanently lose the authoritative
        // ListSubAgents refresh.
        if pending_subagent_list_refresh {
            match engine_handle.try_send(Op::ListSubAgents) {
                Ok(()) => pending_subagent_list_refresh = false,
                Err(err) => {
                    if err
                        .downcast_ref::<tokio::sync::mpsc::error::TrySendError<Op>>()
                        .is_some_and(|send_err| {
                            matches!(send_err, tokio::sync::mpsc::error::TrySendError::Closed(_))
                        })
                    {
                        pending_subagent_list_refresh = false;
                    }
                }
            }
        }

        if let Some(next) = queued_to_send {
            let _ = dispatch_user_message_with_recovery(
                app,
                config,
                &engine_handle,
                next,
                DispatchRecovery::Queued {
                    restore_index: None,
                },
            )
            .await;

            app.needs_redraw = true;
        }

        // Avoid cloning the queued messages/draft every loop iteration
        // (~20-40 Hz) purely for change detection. When the queue is empty and
        // was empty last time — the overwhelmingly common case — there is
        // nothing to compare, so skip the clone entirely. A multi-KB queued
        // draft is only cloned while one is actually pending.
        let queue_now_empty = app.queued_messages.is_empty() && app.queued_draft.is_none();
        if !(queue_now_empty && last_queue_was_empty) {
            let queue_state = (app.queued_messages.clone(), app.queued_draft.clone());
            if queue_state != last_queue_state {
                persist_offline_queue_state(app);
                last_queue_state = queue_state;
                app.needs_redraw = true;
            }
            last_queue_was_empty = queue_now_empty;
        }

        if !app.view_stack.is_empty() {
            let events = app.view_stack.tick();
            if !events.is_empty() {
                app.needs_redraw = true;
                if handle_view_events_boxed(
                    terminal,
                    app,
                    config,
                    &task_manager,
                    &mut engine_handle,
                    &mut web_config_session,
                    events,
                )
                .await?
                {
                    return Ok(());
                }
            }
        }

        let has_running_agents = running_agent_count(app) > 0;
        if reconcile_turn_liveness(app, Instant::now(), has_running_agents) {
            app.needs_redraw = true;
        }
        maybe_throttled_recovery_snapshot(app, Instant::now(), &mut last_recovery_snapshot_at);
        let history_has_live_motion = history_has_live_motion(&app.history);
        let active_cell_has_live_motion = active_cell_has_live_motion(app);
        let translation_placeholder_has_live_motion = app.translation_enabled
            && (pending_thinking_translations > 0 || app.streaming_thinking_active_entry.is_some());
        // Idle ambient motion belongs to every underwater treatment: ombre
        // breathes its water column, while flat and Terminal-owned animate
        // foreground life only. Schedule redraws only when something can
        // actually move — the ombre field at any size, or ambient life once
        // the empty water is large enough to earn it.
        let ombre_field_breathes = app.ocean_treatment.is_ombre()
            && crate::tui::ocean::OceanRamp::for_theme(&app.ui_theme).is_some();
        let browsing_history = !app.viewport.transcript_scroll.is_at_tail();
        let empty_water_visible = app.history.is_empty()
            && app
                .active_cell
                .as_ref()
                .is_none_or(crate::tui::active_cell::ActiveCell::is_empty)
            && !app.is_loading;
        // A paused terminal owns the eye. Modal/launch/onboarding visibility
        // and attention stillness are centralized in the shell motion gate.
        let underwater_surface_obscured = event_broker.is_paused();
        let underwater_motion_visible = underwater_motion_surface_visible(
            app.viewport.last_transcript_area,
            ombre_field_breathes,
            empty_water_visible,
            underwater_surface_obscured,
        );
        let shell_motion_enabled = crate::tui::underwater::decorative_shell_motion_enabled(app);
        let underwater_ambient_motion = shell_motion_enabled
            && underwater_motion_visible
            && (browsing_history
                || matches!(
                    crate::tui::underwater::ShellPhase::from_app(app),
                    crate::tui::underwater::ShellPhase::Working
                        | crate::tui::underwater::ShellPhase::Verifying
                )
                || empty_water_visible);
        let underwater_completion_motion = shell_motion_enabled
            && !underwater_surface_obscured
            && matches!(app.runtime_turn_status.as_deref(), Some("completed"))
            && app
                .ocean_completion_started_at
                .is_some_and(|started| started.elapsed() < Duration::from_millis(800));
        let status_motion = should_tick_status_animation(
            app,
            has_running_agents,
            history_has_live_motion,
            active_cell_has_live_motion,
            translation_placeholder_has_live_motion,
        );
        let animation_interval_ms = animation_interval_ms(
            app,
            status_motion,
            underwater_ambient_motion || underwater_completion_motion,
        );
        let motion_policy = app.motion_policy();
        if (status_motion || underwater_ambient_motion || underwater_completion_motion)
            && last_status_frame.elapsed() >= Duration::from_millis(animation_interval_ms)
        {
            let translation_animated = streaming_thinking::animate_pending_translation(
                app,
                pending_thinking_translations > 0,
            );
            if !matches!(motion_policy.mode(), MotionMode::Still)
                && (history_has_live_motion || active_cell_has_live_motion)
            {
                if translation_animated {
                    if history_has_live_motion {
                        app.mark_live_history_motion_updated();
                    }
                } else {
                    app.mark_live_motion_updated();
                }
            }
            // Coalesce decorative animation wakes through the shared requester.
            // Reduced/Still drop these requests; state-change redraws still set
            // needs_redraw directly below for phase/working chrome.
            frame_requester.request_frame(Instant::now(), motion_policy);
            if frame_requester.take_due(Instant::now(), motion_policy)
                || !motion_policy.should_request_animation_frames()
            {
                // Full: emit only when the requester fires. Reduced/Still: keep
                // the existing calm redraw so working/phase chrome stays truthful
                // without decorative spin (TUI-DOG-008).
                app.needs_redraw = true;
            }
            last_status_frame = Instant::now();
        }

        if event_broker.is_paused() {
            let grace_active = terminal_paused_at
                .map(|paused_at| paused_at.elapsed() < Duration::from_millis(500))
                .unwrap_or(false);
            if terminal_pause_has_live_owner(app) || grace_active {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
            resume_terminal(
                terminal,
                app.use_alt_screen,
                app.use_mouse_capture,
                app.use_bracketed_paste,
                app.synchronized_output_enabled,
            )?;
            event_broker.resume_events();
            terminal_paused_at = None;
            app.status_message = Some("Terminal controls restored".to_string());
            app.needs_redraw = true;
            force_terminal_repaint = true;
        }

        let now = Instant::now();
        app.flush_paste_burst_if_enabled(now);
        app.sync_status_message_to_toasts();
        // Drain background-LLM cost (compaction summaries, seam
        // recompaction, cycle briefings) accumulated since the last
        // tick and fold it into the session-cost counter (#526).
        // Background callers populate `cost_status::report`; we sweep
        // the pool once per loop iteration so the footer chip matches
        // the DeepSeek website's billing.
        let pending_bg_cost = crate::cost_status::drain();
        if pending_bg_cost.is_positive() {
            app.accrue_subagent_cost_estimate(pending_bg_cost);
            app.needs_redraw = true;
        }
        // Drain completed file-tree walks (initial build / expands) so the
        // spliced children repaint without waiting for an input event (#3900).
        if let Some(tree) = app.file_tree.as_mut()
            && tree.poll_background()
        {
            app.needs_redraw = true;
        }
        // Completion discovery is serialized off-thread. Polling is
        // non-blocking and makes a finished initial `@` scan visible even
        // after the user stops typing (#4365).
        if crate::tui::file_mention::poll_background_mention_discovery(app) {
            app.needs_redraw = true;
        }
        // Expire the "Press Ctrl+C again to quit" prompt silently after its
        // window. Triggers a redraw if the prompt was visible.
        app.tick_quit_armed();
        app.tick_receipt();
        crate::tui::footer_ui::maybe_log_provider_wait_incident(app);
        // While the user is drag-selecting past the transcript edge, advance
        // the viewport on a fixed cadence and extend the selection head so a
        // long passage can be selected in one drag (#1163).
        tick_selection_autoscroll(app);
        let allow_workspace_context_refresh =
            !app.is_loading && !has_running_agents && !app.is_compacting && !app.is_purging;
        workspace_context::refresh_if_needed(app, now, allow_workspace_context_refresh);

        // Draw is gated by the frame-rate limiter (120 FPS cap). When a
        // redraw is needed but the limiter says we're inside the cooldown
        // window, leave `needs_redraw = true` and shorten the poll timeout
        // so the loop wakes up exactly when drawing is allowed.

        // Central motion contract: frame cap, stream catch-up, and chunking
        // all read from MotionPolicy so reduced motion stays semantically calm
        // (not a slow typewriter) and Full motion keeps the steady display clock.
        let motion_policy = app.motion_policy();
        frame_rate_limiter.set_low_motion(motion_policy.uses_constrained_frame_rate());
        app.streaming_state
            .set_low_motion(motion_policy.as_low_motion());
        stream_display_clock.set_allow_catch_up(motion_policy.allows_catch_up_bursts());

        let draw_wait = if app.needs_redraw {
            frame_rate_limiter.time_until_next_draw(now)
        } else {
            None
        };
        // Merge the per-app full-repaint hint (set by theme switches)
        // into the loop-level flag before the draw decision.
        if app.force_next_full_repaint {
            force_terminal_repaint = true;
            app.force_next_full_repaint = false;
        }
        if app.needs_redraw && draw_wait.is_none() {
            draw_app_frame_inner(terminal, app, config, force_terminal_repaint)?;
            force_terminal_repaint = false;
            frame_rate_limiter.mark_emitted(Instant::now());
            app.needs_redraw = false;
        }

        let mut poll_timeout =
            if app.is_loading || has_running_agents || app.is_compacting || app.is_purging {
                Duration::from_millis(active_poll_ms(app))
            } else {
                Duration::from_millis(idle_poll_ms(app))
            };
        if let Some(until_flush) = app.paste_burst_next_flush_delay_if_enabled(now) {
            poll_timeout = poll_timeout.min(until_flush);
        }
        if let Some(until_draw) = draw_wait {
            poll_timeout = poll_timeout.min(until_draw);
        }
        if let Some(until_stream_commit) = stream_display_clock.due_in(now) {
            poll_timeout = poll_timeout.min(until_stream_commit);
        }
        if let Some(until_anim) = frame_requester.due_in(now) {
            poll_timeout = poll_timeout.min(until_anim);
        }
        if web_config_session.is_some() {
            poll_timeout = poll_timeout.min(Duration::from_millis(WEB_CONFIG_POLL_MS));
        }
        // While the quit-confirmation prompt is armed, ensure we wake up to
        // expire it on time even if no input event arrives.
        if let Some(deadline) = app.quit_armed_until {
            let remaining = deadline.saturating_duration_since(now);
            poll_timeout = poll_timeout.min(remaining.max(Duration::from_millis(50)));
        }
        // Drag-edge auto-scroll wakes the loop on its own cadence so the
        // viewport keeps advancing while the user holds the mouse outside
        // the transcript rect (#1163).
        if let Some(state) = app.viewport.selection_autoscroll {
            let remaining = state.next_tick.saturating_duration_since(now);
            poll_timeout = poll_timeout.min(remaining);
        }
        poll_timeout = clamp_event_poll_timeout(poll_timeout);

        // #549/#3216: give the engine task a scheduler turn before waiting on
        // the terminal-input channel. Crossterm's blocking poll/read runs on
        // `TerminalInputPump`, so engine floods cannot pin the OS input read.
        tokio::task::yield_now().await;

        let maybe_terminal_event =
            next_terminal_event(&terminal_input, &mut pending_terminal_events, poll_timeout)?;
        if maybe_terminal_event.is_none() {
            let now = Instant::now();
            let input_stalled_for = terminal_input.stalled_for(now);
            if terminal_input_recovery_relevant(app, has_running_agents)
                && input_stalled_for >= TERMINAL_INPUT_STALL_TIMEOUT
                && now.duration_since(last_terminal_input_recovery)
                    >= TERMINAL_INPUT_RECOVERY_COOLDOWN
            {
                tracing::warn!(
                    stalled_ms = input_stalled_for.as_millis(),
                    "terminal input pump heartbeat stalled; attempting terminal input recovery"
                );
                recover_terminal_modes(
                    terminal.backend_mut(),
                    app.use_mouse_capture,
                    app.use_bracketed_paste,
                );
                match terminal_input.restart_detached() {
                    Ok(()) => {
                        app.push_status_toast(
                            if cfg!(target_os = "windows") {
                                "Recovered terminal input after a stalled Windows console poll."
                            } else {
                                "Recovered terminal input after a stalled terminal read."
                            },
                            StatusToastLevel::Warning,
                            None,
                        );
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to restart terminal input pump");
                        app.push_status_toast(
                            "Terminal input stalled; recovery failed. Restart Codewhale if keys stop responding.",
                            StatusToastLevel::Error,
                            None,
                        );
                    }
                }
                terminal_input.mark_alive();
                last_terminal_input_recovery = now;
                if app.is_loading
                    || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
                {
                    persist_recovery_snapshot(app);
                    last_recovery_snapshot_at = Some(now);
                }
                force_terminal_repaint = true;
                app.needs_redraw = true;
            }
        }

        if let Some(evt) = maybe_terminal_event {
            app.needs_redraw = true;

            // Handle bracketed paste events
            if let Event::Paste(text) = &evt {
                tracing::debug!(
                    paste_len = text.len(),
                    preview = %text.chars().take(80).collect::<String>(),
                    "Received bracketed paste event"
                );
                // Once a real bracketed-paste event has been observed in
                // this session, the rapid-keystroke heuristic in
                // paste_burst is redundant — disable it so fast typing /
                // IME commits / autocomplete bursts don't get
                // mis-classified as a paste.
                app.bracketed_paste_seen = true;
                if app.onboarding == OnboardingState::ApiKey {
                    // Paste into API key input
                    app.insert_api_key_str(text);
                    onboarding::sync_api_key_validation_status(app, config, false);
                } else if app.is_history_search_active() {
                    app.history_search_insert_str(text);
                } else if app.view_stack.handle_paste(text) {
                    // Modal consumed the paste (e.g. provider picker key entry)
                } else if !app.view_stack.is_empty() {
                    // A non-consumed modal is open — don't leak paste into composer
                } else {
                    // Paste into main input
                    app.insert_paste_text(text);
                }
                continue;
            }

            // Re-establish terminal mode flags on focus-gain and force a full
            // viewport reset before repainting. App-switching and interactive
            // handoffs can leave the host terminal scrolled away from row 0
            // and (on macOS) can drop the keyboard, mouse-tracking, or
            // bracketed-paste modes — recover_terminal_modes() is the
            // canonical place those flags live.
            if terminal_event_needs_viewport_recapture(&evt) {
                let now = Instant::now();
                if now.duration_since(last_focus_recovery) >= FOCUS_RECOVERY_DEBOUNCE {
                    recover_terminal_modes(
                        terminal.backend_mut(),
                        app.use_mouse_capture,
                        app.use_bracketed_paste,
                    );
                    last_focus_recovery = now;
                }
                force_terminal_repaint = true;
                app.needs_redraw = true;
            }
            if let Event::Resize(width, height) = evt {
                tracing::debug!(
                    width,
                    height,
                    use_alt_screen = app.use_alt_screen,
                    "Event::Resize received; clearing terminal"
                );
                // Drain any further Resize events queued in this poll cycle so we
                // act on the final size only, then issue a single clear + redraw.
                // crossterm coalesces some resize events but rapid drag-resizes
                // can still queue several; processing them all here avoids the
                // common "stale art on the right edge" symptom (#65) caused by
                // the diff renderer skipping cells that match a stale back
                // buffer between intermediate sizes.
                let mut final_w = width;
                let mut final_h = height;
                while let Some(next_evt) =
                    try_next_terminal_event(&terminal_input, &mut pending_terminal_events)?
                {
                    match next_evt {
                        Event::Resize(w, h) => {
                            final_w = w;
                            final_h = h;
                        }
                        other => {
                            pending_terminal_events.push_back(other);
                            break;
                        }
                    }
                }

                if final_w == 0 || final_h == 0 {
                    tracing::debug!(
                        final_w,
                        final_h,
                        "zero-size Resize event ignored while terminal is hidden/minimized"
                    );
                    force_terminal_repaint = true;
                    app.needs_redraw = true;
                    continue;
                }

                // #582: commit the event-reported size to ratatui's
                // viewport explicitly before the redraw, instead of
                // relying on `crossterm::terminal::size()` which gets
                // queried internally during `terminal.draw`. On
                // Windows ConHost specifically, `terminal::size()` has
                // been observed to return stale dimensions briefly
                // during a maximize→windowed transition; the next
                // `draw` then paints into a buffer that does not
                // match the post-restore viewport, producing the
                // unrecoverable black screen reported by @imakid.
                // The `Event::Resize` payload itself carries the
                // authoritative new size, so we forward it.
                if let Err(err) = terminal.resize(Rect::new(0, 0, final_w, final_h)) {
                    tracing::warn!(
                        ?err,
                        final_w,
                        final_h,
                        "terminal.resize during Resize event failed; falling back to clear+draw"
                    );
                }

                app.handle_resize(final_w, final_h);
                // #macos-resize: some terminals (macOS Terminal.app, Windows
                // ConHost) briefly report stale dimensions via
                // `terminal::size()` after a resize. ratatui's `draw()` calls
                // `autoresize()` internally, which queries the backend size;
                // if it sees the old dimension it shrinks the viewport back,
                // leaving the newly-expanded area filled with stale content
                // from the previous frame (duplicate UI panels).
                //
                // We force the backend to report the resize-event size for
                // this single draw so the buffer matches the real viewport.
                {
                    let backend = terminal.backend_mut();
                    let new_size = Size::new(final_w, final_h);
                    backend.force_size(new_size);
                    backend.set_terminal_size(new_size);
                }
                draw_app_frame_inner(terminal, app, config, true)?;
                {
                    let backend = terminal.backend_mut();
                    backend.clear_forced_size();
                }
                app.needs_redraw = false;
                continue;
            }

            if app.use_mouse_capture
                && let Event::Mouse(mouse) = evt
            {
                // Mouse interaction clears the ✅ completion marker.
                crate::tui::notifications::reset_title_on_interaction();
                if should_drop_loading_mouse_motion(app, mouse) {
                    continue;
                }
                let events = handle_mouse_event(app, mouse);
                if handle_view_events_boxed(
                    terminal,
                    app,
                    config,
                    &task_manager,
                    &mut engine_handle,
                    &mut web_config_session,
                    events,
                )
                .await?
                {
                    return Ok(());
                }
                if let Some(action) = app.pending_launch_action.take() {
                    match action {
                        crate::tui::underwater::LaunchAction::None => {}
                        crate::tui::underwater::LaunchAction::NewSession => {
                            let result = begin_launch_session(app, None);
                            if apply_command_result(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                result,
                            )
                            .await?
                            {
                                return Ok(());
                            }
                        }
                        crate::tui::underwater::LaunchAction::CreateWorktree(name) => {
                            app.launch.status =
                                Some(app.tr(MessageId::LaunchCreatingWorktree).into_owned());
                            match provision_launch_worktree(app.workspace.clone(), name).await {
                                Ok(workspace) => {
                                    let result = begin_launch_session(app, Some(workspace));
                                    if apply_command_result(
                                        terminal,
                                        app,
                                        &mut engine_handle,
                                        &task_manager,
                                        config,
                                        &mut web_config_session,
                                        result,
                                    )
                                    .await?
                                    {
                                        return Ok(());
                                    }
                                }
                                Err(err) => {
                                    app.launch.status = Some(
                                        app.tr(MessageId::LaunchWorktreeFailed)
                                            .replace("{error}", &err.to_string()),
                                    );
                                }
                            }
                        }
                        crate::tui::underwater::LaunchAction::Resume => {
                            if app.launch.workspace_session_count == 0 {
                                app.launch.status =
                                    Some(app.tr(MessageId::LaunchNoSavedSessions).into_owned());
                            } else {
                                app.view_stack
                                    .push(SessionPickerView::new(&app.workspace, app.ui_locale));
                            }
                        }
                        crate::tui::underwater::LaunchAction::Changelog => {
                            let title = app.tr(MessageId::LaunchMenuChangelog).into_owned();
                            open_text_pager(
                                app,
                                title,
                                include_str!("../../CHANGELOG.md").to_string(),
                            );
                        }
                        crate::tui::underwater::LaunchAction::Quit => {
                            let _ = engine_handle.send(Op::Shutdown).await;
                            return Ok(());
                        }
                    }
                    app.needs_redraw = true;
                }
                if let Some(slot) = app.pending_hotbar_slot.take()
                    && let Some(dispatch) = dispatch_hotbar_slot(app, config, slot)?
                {
                    match dispatch {
                        HotbarDispatch::Handled => app.needs_redraw = true,
                        HotbarDispatch::AppAction(action) => {
                            if apply_command_result(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                commands::CommandResult::action(action),
                            )
                            .await?
                            {
                                return Ok(());
                            }
                            if let Err(err) = persist_pending_work_checkpoint(app).await {
                                app.status_message = Some(format!(
                                    "Hotbar change applied, but its Work receipt is pending ({err})"
                                ));
                            }
                            app.needs_redraw = true;
                        }
                    }
                }
                persist_sidebar_settings_if_dirty(app);
                continue;
            }

            // User interaction — clear the ✅ completion marker from the title.
            crate::tui::notifications::reset_title_on_interaction();

            let Event::Key(mut key) = evt else {
                continue;
            };

            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Normalize macOS modifiers: map SUPER (Cmd) to CONTROL so that
            // keyboard shortcuts work consistently across terminal emulators
            // (Terminal.app, iTerm2, Kitty, etc.) that may report different
            // modifier flags (#2938). The select-all chord is exempt: `Cmd+A`
            // must stay distinguishable from readline `Ctrl+A` (start of
            // input) on terminals that forward Cmd, so it keeps its SUPER
            // modifier and routes through `is_select_all_shortcut`.
            if !key_shortcuts::is_select_all_shortcut(&key) {
                let mapped = crate::tui::composer_ui::normalize_macos_modifiers(key.modifiers);
                key.modifiers = mapped;
            }

            // Normalize the raw Ctrl+C control byte (0x03) delivered in
            // PTY/raw-mode — and by some kitty-keyboard-protocol terminals —
            // to canonical Ctrl+C so the quit-arm flow always runs (#4090).
            normalize_raw_ctrl_c(&mut key);

            // Approval is a decision boundary, not a viewport lock. Keep the
            // card focused for its ordinary selection keys while letting the
            // same transcript navigation used by the main shell review the
            // evidence above it (#4371).
            if handle_approval_transcript_key(app, &key) {
                continue;
            }

            // Decision card keyboard routing (v0.8.43 truth-surface).
            // When a card is active, number keys 1-9 select options,
            // j/k or Up/Down navigate, and Enter confirms.
            // Only route keys to the decision card when no other modal
            // (Help, Config, Pager, etc.) is on top of the view stack (#2005).
            if app.view_stack.is_empty()
                && let Some(card) = app.decision_card.as_mut()
            {
                if let Some(n) = decision_card_number_from_key(&key) {
                    card.select_number(n);
                    card.confirm();
                    app.status_message = card
                        .confirmed_label()
                        .map(|label| format!("Selected: {label}"));
                    app.decision_card = None;
                    app.needs_redraw = true;
                } else {
                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            card.select_next();
                            app.needs_redraw = true;
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            card.select_prev();
                            app.needs_redraw = true;
                        }
                        KeyCode::Enter => {
                            card.confirm();
                            app.status_message = card
                                .confirmed_label()
                                .map(|label| format!("Selected: {label}"));
                            app.decision_card = None;
                            app.needs_redraw = true;
                        }
                        KeyCode::Esc => {
                            app.decision_card = None;
                            app.status_message = Some("Decision cancelled".to_string());
                            app.needs_redraw = true;
                        }
                        _ => {}
                    }
                }
                submit_initial_input_if_ready(app, config, &engine_handle).await?;
                continue;
            }

            // Clicking the WorkflowPanel gives its non-text controls focus,
            // but ordinary characters always return directly to the composer.
            // This keeps the panel keyboard-accessible without stealing the
            // first t/c/j/k (or any other letter) of a new chat.
            if app.view_stack.is_empty() && handle_workflow_panel_key(app, &key) {
                submit_initial_input_if_ready(app, config, &engine_handle).await?;
                continue;
            }

            // The Ocean work surface is a real focus owner. Route its keys
            // before global transcript/composer navigation so PageUp/Down,
            // Home/End, arrows, and row actions stay panel-local.
            if app.view_stack.is_empty()
                && let Some(action) = crate::tui::work_surface::handle_key(app, key)
            {
                if let Some(action) = action {
                    match action {
                        crate::tui::app::SidebarRowAction::Command(command) => {
                            if execute_command_input(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                &command,
                            )
                            .await?
                            {
                                return Ok(());
                            }
                        }
                        crate::tui::app::SidebarRowAction::CancelAgent { agent_id } => {
                            app.status_message = Some(format!("Cancelling {agent_id}..."));
                            if engine_handle
                                .send(Op::CancelSubAgent {
                                    agent_id: agent_id.clone(),
                                })
                                .await
                                .is_err()
                            {
                                app.status_message = Some(format!("Could not cancel {agent_id}"));
                            }
                        }
                        other => {
                            let _ = crate::tui::mouse_ui::apply_sidebar_row_action(app, other);
                        }
                    }
                }
                submit_initial_input_if_ready(app, config, &engine_handle).await?;
                continue;
            }

            // Help is shell-global, including onboarding, launch, and modal
            // surfaces. `/help` remains the guaranteed textual route; this
            // handles function-key and control-key terminal encodings.
            if crate::tui::shell_key_routing::is_help_shortcut(&key) {
                if app.view_stack.top_kind() == Some(ModalKind::Help) {
                    app.view_stack.pop();
                } else {
                    let help = HelpView::new_for_shortcuts(app.ui_locale, &app.workspace);
                    app.view_stack.push(help);
                }
                continue;
            }

            // Provider onboarding is a real ProviderPickerView, not a
            // parallel ten-provider key handler. Route its keys before the
            // legacy onboarding switch so List/Key/Model/Confirm retain the
            // same behavior as `/provider` and `/setup`.
            if app.onboarding == OnboardingState::Provider
                && app.view_stack.top_kind() == Some(ModalKind::ProviderPicker)
            {
                if key.code == KeyCode::Esc {
                    // Onboarding has no committed provider choice yet. A
                    // single Escape always abandons the whole setup modal and
                    // returns to Language; do not let an inner key/model
                    // stage mutate config or mark onboarding complete.
                    app.view_stack.pop();
                    back_from_provider_onboarding(app);
                    app.needs_redraw = true;
                    continue;
                }
                let events = app.view_stack.handle_key(key);
                app.needs_redraw = true;
                if handle_view_events_boxed(
                    terminal,
                    app,
                    config,
                    &task_manager,
                    &mut engine_handle,
                    &mut web_config_session,
                    events,
                )
                .await?
                {
                    return Ok(());
                }
                continue;
            }

            // Handle onboarding flow
            if app.onboarding != OnboardingState::None {
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        let _ = engine_handle.send(Op::Shutdown).await;
                        return Ok(());
                    }
                    KeyCode::Esc if app.onboarding == OnboardingState::ApiKey => {
                        back_from_api_key_onboarding(app);
                    }
                    KeyCode::Esc if app.onboarding == OnboardingState::Provider => {
                        back_from_provider_onboarding(app);
                    }
                    KeyCode::Esc if app.onboarding == OnboardingState::Language => {
                        app.onboarding = OnboardingState::Welcome;
                        app.status_message = None;
                    }
                    KeyCode::Esc if app.onboarding == OnboardingState::MentalModels => {
                        onboarding::back_from_mental_models(app);
                    }
                    _ if app.onboarding == OnboardingState::MentalModels
                        && is_permission_cycle_shortcut(&key) =>
                    {
                        cycle_permission_posture(app, config, &engine_handle).await;
                    }
                    KeyCode::Tab
                        if app.onboarding == OnboardingState::MentalModels
                            && key.modifiers.is_empty() =>
                    {
                        app.cycle_mode();
                        sync_mode_update(app, &engine_handle).await;
                    }
                    // Language picker hotkeys select + persist (#566).
                    //
                    // Note: this used to be a single match-guard with `&& let`,
                    // but `if_let_guard` is a nightly-only feature on Rust
                    // before 1.94. Rewriting as a plain guard + nested `if let`
                    // keeps `cargo install` working on stable.
                    KeyCode::Char(c)
                        if app.onboarding == OnboardingState::Language && c.is_ascii_digit() =>
                    {
                        if let Some((_, tag, _, _)) = onboarding::language::LANGUAGE_OPTIONS
                            .iter()
                            .find(|(hotkey, _, _, _)| *hotkey == c)
                        {
                            match app.set_locale_from_onboarding(tag) {
                                Ok(()) => {
                                    app.push_status_toast(
                                        format!("Language set to {tag}"),
                                        StatusToastLevel::Info,
                                        Some(2_500),
                                    );
                                    onboarding::advance_onboarding_after_language(app);
                                    open_onboarding_provider_picker(
                                        app,
                                        config,
                                        &engine_handle,
                                        false,
                                    )
                                    .await;
                                }
                                Err(err) => {
                                    app.status_message =
                                        Some(format!("Failed to save locale: {err}"));
                                }
                            }
                        }
                    }
                    KeyCode::Enter => match app.onboarding {
                        OnboardingState::Welcome => {
                            onboarding::advance_onboarding_from_welcome(app);
                        }
                        OnboardingState::Language => {
                            // Enter without a digit pick keeps the existing
                            // setting (which defaults to "auto").
                            onboarding::advance_onboarding_after_language(app);
                            open_onboarding_provider_picker(app, config, &engine_handle, false)
                                .await;
                        }
                        OnboardingState::Provider => {
                            open_onboarding_provider_picker(app, config, &engine_handle, false)
                                .await;
                        }
                        OnboardingState::ApiKey => {
                            let key = app.api_key_input.trim().to_string();
                            if let onboarding::ApiKeyValidation::Reject(message) =
                                onboarding::validate_api_key_for_onboarding(
                                    config,
                                    app.onboarding_provider,
                                    &key,
                                )
                            {
                                app.status_message = Some(message);
                                continue;
                            }
                            if key.is_empty() {
                                let _ = submit_keyless_onboarding_provider(
                                    app,
                                    &mut engine_handle,
                                    config,
                                )
                                .await;
                                continue;
                            }
                            match app.submit_api_key() {
                                Ok(saved) => {
                                    // Surface where the key landed so the
                                    // user can verify the shared config
                                    // file path before the welcome
                                    // screen advances. The toast queue
                                    // outlives the onboarding state
                                    // transition, so it stays visible on
                                    // the next screen too.
                                    app.push_status_toast(
                                        format!("API key saved to {}", saved.describe()),
                                        StatusToastLevel::Info,
                                        Some(4_000),
                                    );
                                    app.status_message = None;
                                    mirror_saved_api_key_in_config(
                                        config,
                                        app.onboarding_provider,
                                        key.clone(),
                                    );
                                    switch_provider(
                                        app,
                                        &mut engine_handle,
                                        config,
                                        app.onboarding_provider,
                                        None,
                                    )
                                    .await;
                                    app.offline_mode = false;

                                    onboarding::advance_onboarding_after_api_key(app);
                                }
                                Err(e) => {
                                    app.status_message = Some(e.to_string());
                                }
                            }
                        }
                        OnboardingState::TrustDirectory => {
                            // Trusting a workspace is a security boundary, so it
                            // must be a deliberate choice. Enter — the "advance"
                            // key on every other onboarding screen — must NOT
                            // grant trust by reflex (accidental-trust risk). Nor
                            // is it a silent dead key: point the user at the
                            // explicit keys the footer advertises.
                            app.status_message = Some(
                                "Press 1 or Y to trust this workspace, or 2 or N to exit."
                                    .to_string(),
                            );
                        }
                        OnboardingState::MentalModels => {
                            app.status_message = None;
                            app.onboarding = OnboardingState::Tips;
                        }
                        OnboardingState::Tips => {
                            app.finish_onboarding_without_feature_intro();
                            if !app.launch.visible
                                && !open_setup_checkpoint_if_due(app, config, false)
                            {
                                app.maybe_show_feature_intro();
                            }
                        }
                        OnboardingState::None => {}
                    },
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1')
                        if app.onboarding == OnboardingState::TrustDirectory =>
                    {
                        if let Err(err) = complete_trust_directory_onboarding(app, config) {
                            app.status_message = Some(format!("Failed to trust workspace: {err}"));
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2')
                        if app.onboarding == OnboardingState::TrustDirectory =>
                    {
                        let _ = engine_handle.send(Op::Shutdown).await;
                        return Ok(());
                    }
                    KeyCode::Esc if app.onboarding == OnboardingState::TrustDirectory => {
                        let _ = engine_handle.send(Op::Shutdown).await;
                        return Ok(());
                    }
                    KeyCode::Backspace if app.onboarding == OnboardingState::ApiKey => {
                        app.delete_api_key_char();
                        onboarding::sync_api_key_validation_status(app, config, false);
                    }
                    KeyCode::Char('h')
                        if key_shortcuts::is_ctrl_h_backspace(&key)
                            && app.onboarding == OnboardingState::ApiKey =>
                    {
                        app.delete_api_key_char();
                        onboarding::sync_api_key_validation_status(app, config, false);
                    }
                    _ if key_shortcuts::is_paste_shortcut(&key)
                        && app.onboarding == OnboardingState::ApiKey =>
                    {
                        // Cmd+V / Ctrl+V paste (bracketed paste handled above)
                        if app.paste_api_key_from_clipboard() {
                            onboarding::sync_api_key_validation_status(app, config, false);
                        }
                    }
                    KeyCode::Char(c)
                        if app.onboarding == OnboardingState::ApiKey
                            && key_shortcuts::is_text_input_key(&key) =>
                    {
                        app.insert_api_key_char(c);
                        onboarding::sync_api_key_validation_status(app, config, false);
                    }
                    _ => {}
                }
                continue;
            }

            // The pre-session launch menu owns every key until the user has
            // chosen a real session/worktree action. Resume and changelog may
            // place a shared surface above it; those views keep their normal
            // handlers while the launch screen remains the stable backdrop.
            if app.launch.visible {
                if !app.view_stack.is_empty() {
                    let events = app.view_stack.handle_key(key);
                    app.needs_redraw = true;
                    if handle_view_events_boxed(
                        terminal,
                        app,
                        config,
                        &task_manager,
                        &mut engine_handle,
                        &mut web_config_session,
                        events,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                    continue;
                }

                let launch_locale = app.ui_locale;
                match crate::tui::underwater::handle_launch_key(&mut app.launch, key, launch_locale)
                {
                    crate::tui::underwater::LaunchAction::None => {}
                    crate::tui::underwater::LaunchAction::NewSession => {
                        let result = begin_launch_session(app, None);
                        if apply_command_result(
                            terminal,
                            app,
                            &mut engine_handle,
                            &task_manager,
                            config,
                            &mut web_config_session,
                            result,
                        )
                        .await?
                        {
                            return Ok(());
                        }
                    }
                    crate::tui::underwater::LaunchAction::CreateWorktree(name) => {
                        app.launch.status =
                            Some(app.tr(MessageId::LaunchCreatingWorktree).into_owned());
                        match provision_launch_worktree(app.workspace.clone(), name).await {
                            Ok(workspace) => {
                                let result = begin_launch_session(app, Some(workspace));
                                if apply_command_result(
                                    terminal,
                                    app,
                                    &mut engine_handle,
                                    &task_manager,
                                    config,
                                    &mut web_config_session,
                                    result,
                                )
                                .await?
                                {
                                    return Ok(());
                                }
                            }
                            Err(err) => {
                                app.launch.status = Some(
                                    app.tr(MessageId::LaunchWorktreeFailed)
                                        .replace("{error}", &err.to_string()),
                                );
                            }
                        }
                    }
                    crate::tui::underwater::LaunchAction::Resume => {
                        if app.launch.workspace_session_count == 0 {
                            app.launch.status =
                                Some(app.tr(MessageId::LaunchNoSavedSessions).into_owned());
                        } else {
                            app.view_stack
                                .push(SessionPickerView::new(&app.workspace, app.ui_locale));
                        }
                    }
                    crate::tui::underwater::LaunchAction::Changelog => {
                        let title = app.tr(MessageId::LaunchMenuChangelog).into_owned();
                        open_text_pager(app, title, include_str!("../../CHANGELOG.md").to_string());
                    }
                    crate::tui::underwater::LaunchAction::Quit => {
                        let _ = engine_handle.send(Op::Shutdown).await;
                        return Ok(());
                    }
                }
                app.needs_redraw = true;
                continue;
            }

            if key.code == KeyCode::Char('x')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && prefill_jobs_cancel_all_if_tasks_sidebar(app)
            {
                continue;
            }

            if key.code == KeyCode::Char('k') && key.modifiers.contains(KeyModifiers::CONTROL) {
                // When the composer is the active input target (no modal/pager
                // intercepting keys), Ctrl+K performs an emacs-style kill to
                // end-of-line. If the kill is a no-op (cursor at end of empty
                // input), fall through to the existing command palette.
                if app.view_stack.is_empty() && app.kill_to_end_of_line() {
                    continue;
                }
                app.view_stack.push(CommandPaletteView::new_for_locale(
                    app.ui_locale,
                    build_command_palette_entries(
                        app.ui_locale,
                        &app.skills_dir,
                        app.skills_scan_codewhale_only,
                        &app.workspace,
                        &app.mcp_config_path,
                        app.mcp_snapshot.as_ref(),
                        app.plugin_registry.as_ref(),
                    ),
                ));
                continue;
            }

            // y / Y in the Activity sidebar: yank the current turn id (y)
            // or copy full task detail (Y) to the system clipboard.
            // Only active when the composer is empty to avoid stealing
            // keystrokes from typed input (#2000).
            if app.view_stack.is_empty()
                && app.sidebar_focus == SidebarFocus::Tasks
                && app.input.is_empty()
                && !app.runtime_turn_id.as_deref().unwrap_or("").is_empty()
            {
                if key.code == KeyCode::Char('y') && key.modifiers == KeyModifiers::NONE {
                    if let Some(turn_id) = app.runtime_turn_id.as_ref()
                        && app.clipboard.write_text(turn_id).is_ok()
                    {
                        app.status_message = Some(format!("Copied turn id {turn_id}"));
                    }
                    continue;
                }
                if key.code == KeyCode::Char('Y') && key.modifiers == KeyModifiers::NONE {
                    let mut detail = String::new();
                    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
                        let _ = write!(detail, "turn {turn_id}");
                    }
                    if let Some(status) = app.runtime_turn_status.as_deref() {
                        let _ = write!(detail, "  status={status}");
                    }
                    if !detail.is_empty() && app.clipboard.write_text(&detail).is_ok() {
                        app.status_message = Some(format!("Copied {detail}"));
                    }
                    continue;
                }
            }

            // Shifted shortcuts toggle the file-tree pane. Keep plain Ctrl+E
            // reserved for the composer end-of-line binding used by shells.
            if key_shortcuts::is_file_tree_toggle_shortcut(&key) {
                if let Some(_state) = app.file_tree.as_mut() {
                    // File tree visible → hide it.
                    app.file_tree = None;
                    app.status_message = Some("File tree closed".to_string());
                } else {
                    // Build the file tree from the current workspace.
                    let state = crate::tui::file_tree::FileTreeState::new(&app.workspace);
                    app.file_tree = Some(state);
                    app.status_message = Some(
                        "File tree: \u{2191}/\u{2193} navigate  Enter select  Esc close"
                            .to_string(),
                    );
                }
                app.needs_redraw = true;
                continue;
            }

            // Ctrl+P opens the fuzzy file-picker overlay. Bound only when the
            // composer is focused (no other modal or inline popup on top) and the
            // engine is not actively streaming a turn.
            if key.code == KeyCode::Char('p')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && visible_slash_menu_entries(app, SLASH_MENU_LIMIT).is_empty()
                && app.view_stack.is_empty()
                && !app.is_loading
            {
                file_picker_relevance::open_file_picker(app);
                continue;
            }

            if matches!(key.code, KeyCode::Char('l') | KeyCode::Char('L'))
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && app.view_stack.is_empty()
            {
                app.status_message = Some(if app.is_compacting {
                    "Context compaction already in progress...".to_string()
                } else {
                    "Compacting context (Ctrl+L)...".to_string()
                });
                if !app.is_compacting {
                    match validated_app_runtime_route(app, config) {
                        Ok(route) => {
                            let compaction = compaction_for_validated_route(app, &route);
                            let _ = engine_handle
                                .send(Op::CompactContext {
                                    route: Box::new(route.into_resolved()),
                                    compaction: Box::new(compaction),
                                })
                                .await;
                        }
                        Err(err) => {
                            app.status_message = Some(format!(
                                "Cannot compact because the active provider route is invalid: {err}"
                            ));
                        }
                    }
                }
                app.needs_redraw = true;
                continue;
            }

            if matches!(key.code, KeyCode::Char('b') | KeyCode::Char('B'))
                && key_shortcuts::has_control_like_modifier(key.modifiers)
                && app.view_stack.is_empty()
            {
                // #3032/#3859: Ctrl+B moves the active foreground shell wait
                // into /jobs instead of opening a two-step shell-control menu.
                // When nothing is movable, the status message tells the user
                // what's going on.
                request_foreground_shell_background(app);
                app.needs_redraw = true;
                continue;
            }

            if crate::tui::shell_key_routing::is_context_inspector_shortcut(&key)
                && app.view_stack.is_empty()
            {
                open_context_inspector(app);
                continue;
            }

            // Shift+Tab is a shell-level permission control. Keep it live in
            // the composer and the Config surface, while leaving approval,
            // elevation, setup, and other focused workflows in full control
            // of their own keys. Accept both terminal encodings used for the
            // same chord (`BackTab` and `Tab` + SHIFT).
            if is_permission_cycle_shortcut(&key)
                && matches!(app.view_stack.top_kind(), None | Some(ModalKind::Config))
            {
                cycle_permission_posture(app, config, &engine_handle).await;
                continue;
            }

            if !app.view_stack.is_empty() {
                let closing_work_inspector = app.work_surface.opened.is_some()
                    && app.view_stack.top_kind() == Some(ModalKind::Pager);
                let events = app.view_stack.handle_key(key);
                clear_work_inspector_after_pager_close(app, closing_work_inspector);
                app.needs_redraw = true;
                if handle_view_events_boxed(
                    terminal,
                    app,
                    config,
                    &task_manager,
                    &mut engine_handle,
                    &mut web_config_session,
                    events,
                )
                .await?
                {
                    return Ok(());
                }
                persist_sidebar_settings_if_dirty(app);
                continue;
            }

            if let Some(slot) = hotbar_slot_from_key(app, &key) {
                if let Some(dispatch) = dispatch_hotbar_slot(app, config, slot)? {
                    match dispatch {
                        HotbarDispatch::Handled => {
                            app.needs_redraw = true;
                        }
                        HotbarDispatch::AppAction(action) => {
                            if apply_command_result(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                commands::CommandResult::action(action),
                            )
                            .await?
                            {
                                return Ok(());
                            }
                            if let Err(err) = persist_pending_work_checkpoint(app).await {
                                app.status_message = Some(format!(
                                    "Hotbar change applied, but its Work receipt is pending ({err})"
                                ));
                            }
                            app.needs_redraw = true;
                        }
                    }
                }
                continue;
            }

            // File-tree navigation: delegated to key_actions module.
            if key_actions::handle_file_tree_key(app, &key) {
                continue;
            }

            if app.is_history_search_active() {
                handle_history_search_key(app, key);
                continue;
            }

            if matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
                && key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER)
            {
                app.start_history_search();
                continue;
            }

            let now = Instant::now();
            app.flush_paste_burst_if_enabled(now);

            // On Windows, AltGr is delivered as `Ctrl+Alt`; treat
            // AltGr-typed chars (e.g. European layouts producing `@`, `\`,
            // `|`) as plain text rather than swallowing them as a modified
            // shortcut. `key_hint::has_ctrl_or_alt` filters AltGr out.
            let has_ctrl_alt_or_super = super::widgets::key_hint::has_ctrl_or_alt(key.modifiers)
                || key.modifiers.contains(KeyModifiers::SUPER);
            let is_plain_char = matches!(key.code, KeyCode::Char(_)) && !has_ctrl_alt_or_super;
            let is_enter = matches!(key.code, KeyCode::Enter);

            // Tool details: Alt+V / Option+V only. Bare `v` always types `v`
            // in every focus state (TUI-DOG-002).
            if crate::tui::shell_key_routing::is_tool_details_shortcut(&key) {
                open_tool_details_pager(app);
                continue;
            }

            if !is_plain_char
                && !is_enter
                && let Some(pending) = app.flush_paste_burst_before_modified_input_if_enabled()
            {
                app.insert_str(&pending);
            }

            if (is_plain_char || is_enter) && super::paste::handle_paste_burst_key(app, &key, now) {
                continue;
            }

            let slash_menu_entries = visible_slash_menu_entries(app, SLASH_MENU_LIMIT);
            let slash_menu_open = !slash_menu_entries.is_empty();
            if slash_menu_open && app.slash_menu_selected >= slash_menu_entries.len() {
                app.slash_menu_selected = slash_menu_entries.len().saturating_sub(1);
            }
            let mention_menu_limit = app.mention_menu_limit;
            let mention_menu_entries =
                crate::tui::file_mention::visible_mention_menu_entries(app, mention_menu_limit);
            let mention_menu_open = !mention_menu_entries.is_empty();
            if mention_menu_open && app.mention_menu_selected >= mention_menu_entries.len() {
                app.mention_menu_selected = mention_menu_entries.len().saturating_sub(1);
            }

            // Cancel a pending Esc-Esc prime as soon as any non-Esc key
            // arrives. Without this the prime would hang around for the
            // rest of the session and the user's next genuine Esc would
            // suddenly skip straight into the backtrack overlay.
            if !matches!(key.code, KeyCode::Esc)
                && matches!(
                    app.backtrack.phase,
                    crate::tui::backtrack::BacktrackPhase::Primed
                )
            {
                app.backtrack.reset();
            }

            // Global keybindings
            if handle_reasoning_effort_key(app, &key) {
                if let Err(err) = persist_pending_work_checkpoint(app).await {
                    app.status_message = Some(format!(
                        "Reasoning effort changed, but its Work receipt is pending ({err})"
                    ));
                }
                continue;
            }
            match key.code {
                KeyCode::Enter
                    if app.input.is_empty()
                        && app.viewport.transcript_selection.is_active()
                        && open_pager_for_selection(app) =>
                {
                    continue;
                }
                KeyCode::Enter
                    if key.modifiers == KeyModifiers::NONE
                        && app.input.is_empty()
                        && detail_target_cell_index(app)
                            .is_some_and(|idx| app.toggle_tool_run_expansion_at(idx)) =>
                {
                    continue;
                }
                KeyCode::Char('l')
                    if key_shortcuts::alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && open_pager_for_last_message(app) =>
                {
                    continue;
                }
                _ if key_shortcuts::is_turn_inspector_shortcut(&key)
                    && open_turn_inspector_pager(app) =>
                {
                    continue;
                }
                // Space toggles fold/unfold of the focused thinking block
                // when the composer is empty. For thinking cells, toggles
                // between summary and full content; for other cells, toggles
                // visibility (#1972, #2348).
                KeyCode::Char(' ')
                    if key.modifiers == KeyModifiers::NONE && app.input.is_empty() =>
                {
                    if let Some(idx) = detail_target_cell_index(app) {
                        if app.toggle_tool_run_expansion_at(idx) {
                            continue;
                        }
                        let is_thinking = app
                            .history
                            .get(idx)
                            .is_some_and(|c| matches!(c, HistoryCell::Thinking { .. }));
                        if is_thinking {
                            if app.folded_thinking.contains(&idx) {
                                app.folded_thinking.remove(&idx);
                                app.status_message = Some("Thinking block expanded".to_string());
                            } else {
                                app.folded_thinking.insert(idx);
                                app.status_message = Some("Thinking block folded".to_string());
                            }
                        } else if app.collapsed_cells.contains(&idx) {
                            app.collapsed_cells.remove(&idx);
                            app.status_message = Some("Cell expanded".to_string());
                        } else {
                            app.collapsed_cells.insert(idx);
                            app.status_message = Some("Cell collapsed".to_string());
                        }
                        app.mark_history_updated();
                        app.needs_redraw = true;
                    }
                    continue;
                }
                KeyCode::Char('t') | KeyCode::Char('T')
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.modifiers.contains(KeyModifiers::SHIFT) =>
                {
                    toggle_live_transcript_overlay(app);
                    continue;
                }
                KeyCode::Char('1')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && key_shortcuts::has_control_like_modifier(key.modifiers) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Pinned);
                    app.status_message = Some("Sidebar focus: pinned".to_string());
                    continue;
                }
                KeyCode::Char('2')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && key_shortcuts::has_control_like_modifier(key.modifiers) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Tasks);
                    app.status_message = Some("Sidebar focus: activity".to_string());
                    continue;
                }
                KeyCode::Char('3')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && key_shortcuts::has_control_like_modifier(key.modifiers) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Agents);
                    app.status_message = Some("Sidebar focus: agents".to_string());
                    continue;
                }
                KeyCode::Char('4')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && key_shortcuts::has_control_like_modifier(key.modifiers) =>
                {
                    apply_alt_4_shortcut(app, key.modifiers);
                    continue;
                }
                // Sidebar focus via Alt+! / Alt+@ / Alt+# / Alt+$ / Alt+%)
                // AltGr on European keyboards emits Ctrl+Alt on Windows, so
                // exclude Ctrl to avoid swallowing AltGr-typed characters
                // like @ (AltGr+0 on French AZERTY) and # (AltGr+3). This
                // matches the has_ctrl_or_alt / is_altgr philosophy in
                // key_hint.rs: treat Ctrl+Alt as AltGr, not a shortcut.
                KeyCode::Char('!')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Pinned);
                    app.status_message = Some("Sidebar focus: pinned".to_string());
                    continue;
                }
                KeyCode::Char('@')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Tasks);
                    app.status_message = Some("Sidebar focus: activity".to_string());
                    continue;
                }
                KeyCode::Char('#')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Agents);
                    app.status_message = Some("Sidebar focus: agents".to_string());
                    continue;
                }
                KeyCode::Char('$') | KeyCode::Char('%')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Context);
                    app.status_message = Some("Sidebar focus: context".to_string());
                    continue;
                }
                KeyCode::Char(')')
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.set_sidebar_focus(SidebarFocus::Auto);
                    app.status_message = Some("Sidebar focus: auto".to_string());
                    continue;
                }
                KeyCode::Char('0') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_alt_0_shortcut(app, key.modifiers);
                    continue;
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Scope the picker to the current workspace so Ctrl+R
                    // never restores a different project's history by
                    // surprise (#1395). Press `a` inside the picker to
                    // broaden to every saved session.
                    app.view_stack
                        .push(SessionPickerView::new(&app.workspace, app.ui_locale));
                    continue;
                }
                KeyCode::Char('c') | KeyCode::Char('C')
                    if key_shortcuts::is_copy_shortcut(&key) =>
                {
                    let sel = app.selected_text();
                    if !sel.is_empty() {
                        if app.clipboard.write_text(&sel).is_ok() {
                            app.push_status_toast(
                                "Copied to clipboard",
                                StatusToastLevel::Info,
                                None,
                            );
                            app.clear_selection();
                        } else {
                            app.push_status_toast("Copy failed", StatusToastLevel::Error, None);
                        }
                    } else {
                        copy_active_selection(app);
                    }
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Four behaviors layered on Ctrl+C in priority order — see
                    // `CtrlCDisposition` for the unit-tested decision table.
                    // 1. selection active → copy + clear (Windows convention,
                    //    #1337); 2. turn in flight → cancel; 3. quit-armed →
                    //    exit; 4. otherwise → arm the 2-second exit prompt.
                    match ctrl_c_disposition(app) {
                        CtrlCDisposition::CopySelection => {
                            copy_active_selection(app);
                            app.viewport.transcript_selection.clear();
                        }
                        CtrlCDisposition::CancelTurn => {
                            engine_handle.cancel();
                            mark_active_turn_cancelled_locally(app);
                            current_streaming_text.clear();
                            stream_display_clock.reset();
                            let prompt_restored = app.restore_last_submitted_prompt_if_empty();
                            let base = if prompt_restored {
                                "Request cancelled; prompt restored to composer"
                            } else {
                                "Request cancelled"
                            };
                            app.status_message = Some(parent_stop_status(app, base));
                            app.disarm_quit();
                        }
                        CtrlCDisposition::ConfirmExit => {
                            let _ = engine_handle.send(Op::Shutdown).await;
                            return Ok(());
                        }
                        CtrlCDisposition::ArmExit => {
                            app.arm_quit();
                        }
                    }
                }
                KeyCode::Char('d')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && app.input.is_empty() =>
                {
                    let _ = engine_handle.send(Op::Shutdown).await;
                    return Ok(());
                }
                // Vim composer mode: Esc from Insert/Visual → Normal.
                // This arm runs before the generic Esc handler so Insert mode
                // Esc doesn't accidentally cancel an in-flight request.
                KeyCode::Esc
                    if app.composer.vim_enabled
                        && app.composer.vim_mode != crate::tui::app::VimMode::Normal =>
                {
                    app.vim_enter_normal();
                    continue;
                }
                KeyCode::Esc if app.clear_composer_attachment_selection() => {
                    continue;
                }
                KeyCode::Esc if mention_menu_open => {
                    app.mention_menu_hidden = true;
                    app.mention_menu_selected = 0;
                }
                KeyCode::Esc if app.sidebar_hover_tooltip.is_some() => {
                    app.sidebar_hover_tooltip = None;
                    app.needs_redraw = true;
                }
                KeyCode::Esc => {
                    match next_escape_action(app, slash_menu_open) {
                        EscapeAction::CloseSlashMenu => {
                            // A popup-style action wins over backtrack — clear
                            // any prime so a stale Primed state can't jump us
                            // straight into Selecting on the next Esc.
                            app.backtrack.reset();
                            app.close_slash_menu();
                        }
                        EscapeAction::CancelRequest => {
                            app.backtrack.reset();
                            if app.paused || app.paused_quarry.is_some() {
                                clear_paused_command_state(app, &engine_handle);
                                if app.is_loading
                                    || matches!(
                                        app.runtime_turn_status.as_deref(),
                                        Some("in_progress")
                                    )
                                {
                                    engine_handle.cancel();
                                    mark_active_turn_cancelled_locally(app);
                                    current_streaming_text.clear();
                                    stream_display_clock.reset();
                                }
                                app.active_allowed_tools = None;
                                app.hunt.quarry = None;
                                app.hunt.tokens_used = 0;
                                app.hunt.time_used_seconds = 0;
                                app.hunt.continuation_count = 0;
                                app.status_message =
                                    Some(parent_stop_status(app, "Paused command cancelled"));
                            } else {
                                engine_handle.cancel();
                                mark_active_turn_cancelled_locally(app);
                                current_streaming_text.clear();
                                stream_display_clock.reset();
                                app.status_message =
                                    Some(parent_stop_status(app, "Request cancelled"));
                            }
                        }
                        EscapeAction::PauseCommand => {
                            app.backtrack.reset();
                            pause_pausable_command(app, &engine_handle);
                        }
                        EscapeAction::DiscardQueuedDraft => {
                            app.backtrack.reset();
                            if app.cancel_queued_draft_edit() {
                                app.status_message =
                                    Some("Queued edit canceled; follow-up restored".to_string());
                            }
                        }
                        EscapeAction::ClearInput => {
                            app.backtrack.reset();
                            app.edit_in_progress = false;
                            app.clear_input_recoverable();
                            let _ = app.maybe_show_behavioral_tip(
                                crate::tui::behavioral_tips::BehavioralTip::ClearedInputRestore,
                            );
                        }
                        EscapeAction::Noop => {
                            // Nothing else cares about this Esc — route it
                            // through the backtrack state machine. While
                            // streaming or with the live transcript already
                            // open, fall through silently (#133 acceptance:
                            // "during streaming Esc-Esc is a silent no-op").
                            if app.is_loading
                                || app.view_stack.top_kind() == Some(ModalKind::LiveTranscript)
                            {
                                continue;
                            }
                            let total = count_user_history_cells(app);
                            match app.backtrack.handle_esc(total) {
                                crate::tui::backtrack::EscEffect::None => {}
                                crate::tui::backtrack::EscEffect::Prime => {
                                    app.status_message =
                                        Some("Press Esc again to backtrack".to_string());
                                    app.needs_redraw = true;
                                }
                                crate::tui::backtrack::EscEffect::Cancel => {
                                    app.status_message = Some("Backtrack canceled".to_string());
                                    app.needs_redraw = true;
                                }
                                crate::tui::backtrack::EscEffect::OpenOverlay => {
                                    open_backtrack_overlay(app);
                                }
                            }
                        }
                    }
                }
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SUPER) => {
                    app.scroll_up(app.viewport.last_transcript_visible.max(3));
                }
                KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.scroll_up(3);
                }
                KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.scroll_up(3);
                }
                KeyCode::Up
                    if key.modifiers.is_empty()
                        && mention_menu_open
                        && app.mention_menu_selected > 0 =>
                {
                    app.mention_menu_selected = app.mention_menu_selected.saturating_sub(1);
                }
                KeyCode::Up if key.modifiers.is_empty() && slash_menu_open => {
                    select_previous_slash_menu_entry(app, slash_menu_entries.len());
                }
                KeyCode::Char('p')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && slash_menu_open =>
                {
                    select_previous_slash_menu_entry(app, slash_menu_entries.len());
                }
                KeyCode::Up
                    if key.modifiers.is_empty()
                        && app.selected_composer_attachment_index().is_some() =>
                {
                    let _ = app.select_previous_composer_attachment();
                }
                KeyCode::Up
                    if key.modifiers.is_empty()
                        && app.cursor_position == 0
                        && !mention_menu_open
                        && !slash_menu_open
                        && app.composer_attachment_count() > 0 =>
                {
                    let _ = app.select_previous_composer_attachment();
                    continue;
                }
                // #85: ↑ edits the most-recent queued message when the composer
                // is idle and the pending-input preview is showing queued work.
                KeyCode::Up
                    if key.modifiers.is_empty()
                        && app.input.is_empty()
                        && app.cursor_position == 0
                        && app.queued_draft.is_none()
                        && !app.queued_messages.is_empty()
                        && !mention_menu_open
                        && !slash_menu_open
                        && app.selected_composer_attachment_index().is_none() =>
                {
                    let _ = app.pop_last_queued_into_draft();
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::SUPER) => {
                    app.scroll_down(app.viewport.last_transcript_visible.max(3));
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.scroll_down(3);
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.scroll_down(3);
                }
                KeyCode::Down if key.modifiers.is_empty() && mention_menu_open => {
                    app.mention_menu_selected = (app.mention_menu_selected + 1)
                        .min(mention_menu_entries.len().saturating_sub(1));
                }
                KeyCode::Down if key.modifiers.is_empty() && slash_menu_open => {
                    select_next_slash_menu_entry(app, slash_menu_entries.len());
                }
                KeyCode::Char('n')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && slash_menu_open =>
                {
                    select_next_slash_menu_entry(app, slash_menu_entries.len());
                }
                KeyCode::Down
                    if key.modifiers.is_empty()
                        && app.selected_composer_attachment_index().is_some() =>
                {
                    let _ = app.select_next_composer_attachment();
                }
                KeyCode::PageUp => {
                    let page = app.viewport.last_transcript_visible.max(1);
                    app.scroll_up(page);
                }
                KeyCode::PageDown => {
                    let page = app.viewport.last_transcript_visible.max(1);
                    app.scroll_down(page);
                }
                KeyCode::Tab => {
                    if mention_menu_open
                        && crate::tui::file_mention::apply_mention_menu_selection(
                            app,
                            &mention_menu_entries,
                        )
                    {
                        continue;
                    }
                    if slash_menu_open && apply_slash_menu_selection(app, &slash_menu_entries, true)
                    {
                        continue;
                    }
                    if try_autocomplete_slash_command(app) {
                        continue;
                    }
                    if crate::tui::file_mention::try_autocomplete_file_mention(app) {
                        continue;
                    }
                    if app.is_loading && queue_current_draft_for_next_turn(app) {
                        continue;
                    }
                    if app.input.is_empty()
                        && let Some(suggestion) = app.prompt_suggestion.take()
                    {
                        app.input = suggestion;
                        app.cursor_position = app.input.chars().count();
                        app.needs_redraw = true;
                        continue;
                    }
                    let prior_model = app.model.clone();
                    let prior_mode = app.mode;
                    app.cycle_mode();
                    if app.mode != prior_mode {
                        sync_mode_update(app, &engine_handle).await;
                    }
                    if app.model != prior_model {
                        let _ = engine_handle
                            .send(Op::SetModel {
                                model: app.model.clone(),
                                mode: app.mode,
                                route_limits: app.active_route_limits,
                            })
                            .await;
                    }
                }
                // Transcript-nav shortcuts now require Alt, leaving most bare
                // letters free to insert as text. Before v0.8.30, bare `g`,
                // `G`, `[`, `]`, `?`, and `l` on an empty composer were
                // hijacked for navigation — typing "good" yielded "ood" with
                // no whale and no warning. The Alt-prefixed shortcuts mirror
                // the Alt+R / Alt+C pattern already in use. Shift is
                // permitted for most capital-letter forms.
                KeyCode::Char('g')
                    if key_shortcuts::alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open =>
                {
                    if let Some(anchor) =
                        TranscriptScroll::anchor_for(app.viewport.transcript_cache.line_meta(), 0)
                    {
                        app.viewport.transcript_scroll = anchor;
                    }
                }
                KeyCode::Char('G')
                    if key_shortcuts::alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open =>
                {
                    app.scroll_to_bottom();
                }
                KeyCode::Char('[')
                    if key_shortcuts::alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open
                        && !jump_to_adjacent_tool_cell(app, SearchDirection::Backward) =>
                {
                    app.status_message = Some("No previous tool output".to_string());
                }
                KeyCode::Char(']')
                    if key_shortcuts::alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open
                        && !jump_to_adjacent_tool_cell(app, SearchDirection::Forward) =>
                {
                    app.status_message = Some("No next tool output".to_string());
                }
                // Help chords (Alt+?, F1, Ctrl+/) are handled above via
                // shell_key_routing::is_help_shortcut so printable layout
                // characters stay text.
                // Shift+Enter steers a running turn. When idle, the
                // normal composer-newline branch below still handles it
                // as a multiline input gesture.
                KeyCode::Enter
                    if app.is_loading
                        && key.modifiers.contains(KeyModifiers::SHIFT)
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    if let Some(input) = app.submit_input() {
                        if handle_bang_shell_input(app, &engine_handle, &input).await? {
                            continue;
                        }
                        if looks_like_slash_command_input(&input) {
                            if execute_command_input(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                &input,
                            )
                            .await?
                            {
                                return Ok(());
                            }
                        } else {
                            let queued = if let Some(mut draft) = app.queued_draft.take() {
                                draft.display = input;
                                draft
                            } else {
                                build_queued_message(app, input)
                            };
                            attempt_steer_with_queue_fallback(app, &engine_handle, queued).await;
                        }
                    }
                }
                // Input handling
                _ if is_composer_newline_key(key)
                    && !(app.is_loading && is_forced_submit_key(key)) =>
                {
                    app.insert_char('\n');
                }
                KeyCode::Enter
                    if mention_menu_open
                        && crate::tui::file_mention::apply_mention_menu_selection(
                            app,
                            &mention_menu_entries,
                        ) =>
                {
                    continue;
                }
                // #382: Ctrl+Enter forces a steer into the current turn.
                // Some terminals report Ctrl/Cmd+Enter as Ctrl+J; while a
                // turn is running, accept that encoding here instead of
                // inserting a newline.
                _ if is_forced_submit_key(key)
                    && (matches!(key.code, KeyCode::Enter) || app.is_loading) =>
                {
                    if let Some(input) = app.submit_input() {
                        if handle_bang_shell_input(app, &engine_handle, &input).await? {
                            continue;
                        }
                        if looks_like_slash_command_input(&input) {
                            if execute_command_input(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                &input,
                            )
                            .await?
                            {
                                return Ok(());
                            }
                        } else {
                            let queued = if let Some(mut draft) = app.queued_draft.take() {
                                draft.display = input;
                                draft
                            } else {
                                build_queued_message(app, input)
                            };
                            if app.is_loading {
                                // Engine is busy — steer into the current turn.
                                attempt_steer_with_queue_fallback(
                                    app,
                                    &engine_handle,
                                    queued.clone(),
                                )
                                .await;
                            } else {
                                // Engine is idle — send as a regular message
                                // so the content is not lost to rx_steer's
                                // stale-drain in handle_send_message (#1331).
                                submit_or_steer_message(app, config, &engine_handle, queued)
                                    .await?;
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    // #573: when the user typed a slash-command prefix that
                    // the popup is matching (e.g. `/mo` → `/model`), Enter
                    // should run the *highlighted match* rather than
                    // sending the literal `/mo` text. Only kick in when the
                    // popup has at least one entry; otherwise fall through
                    // to the legacy submit path.
                    let selecting_inline_skill = slash_menu_open
                        && partial_inline_skill_mention_at_cursor(&app.input, app.cursor_position)
                            .is_some();
                    if slash_menu_open
                        && !slash_menu_entries.is_empty()
                        && apply_slash_menu_selection(app, &slash_menu_entries, false)
                    {
                        app.close_slash_menu();
                        if selecting_inline_skill {
                            continue;
                        }
                    }
                    if let Some(input) = app.handle_composer_enter() {
                        if handle_plan_choice(app, config, &engine_handle, &input).await? {
                            continue;
                        }
                        // `# foo` quick-add (#492) — when memory is enabled,
                        // a single line starting with `#` (but not `##` /
                        // `#!` shebangs / Markdown headings the user might
                        // be pasting in) is intercepted: the text is
                        // appended to the user memory file and the input
                        // is consumed without firing a turn. Disabled
                        // behaviour falls through to normal turn submit.
                        // TODO(v0.8.71): remove legacy quick-add when Moraine recall stable; see #3490, #3495
                        if should_intercept_memory_quick_add(config, &input) {
                            handle_memory_quick_add(app, &input, config);
                            continue;
                        }
                        if handle_bang_shell_input(app, &engine_handle, &input).await? {
                            continue;
                        }
                        if looks_like_slash_command_input(&input) {
                            if execute_command_input(
                                terminal,
                                app,
                                &mut engine_handle,
                                &task_manager,
                                config,
                                &mut web_config_session,
                                &input,
                            )
                            .await?
                            {
                                return Ok(());
                            }
                        } else {
                            let queued = if let Some(mut draft) = app.queued_draft.take() {
                                draft.display = input;
                                draft
                            } else {
                                build_queued_message(app, input)
                            };
                            // #383: /edit — if the user invoked /edit to revise
                            // the last message, undo the last exchange before
                            // dispatching the replacement. Sync the engine
                            // session so it also drops the old exchange.
                            if app.edit_in_progress {
                                crate::commands::execute("/undo", app);
                                app.edit_in_progress = false;
                                let _ = engine_handle
                                    .send(Op::SyncSession {
                                        session_id: app.current_session_id.clone(),
                                        messages: app.api_messages.clone(),
                                        system_prompt: app.system_prompt.clone(),
                                        system_prompt_override: false,
                                        model: app.model.clone(),
                                        workspace: app.workspace.clone(),
                                        mode: app.mode,
                                    })
                                    .await;
                            }
                            submit_or_steer_message(app, config, &engine_handle, queued).await?;
                        }
                    }
                }
                KeyCode::Backspace
                    if key.modifiers.contains(KeyModifiers::SUPER)
                        && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_to_start_of_line();
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {}
                KeyCode::Backspace
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_word_backward();
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::ALT) => {}
                KeyCode::Backspace
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_word_backward();
                }
                KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) => {}
                KeyCode::Delete
                    if key.modifiers.contains(KeyModifiers::ALT)
                        && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_word_forward();
                }
                KeyCode::Delete if key.modifiers.contains(KeyModifiers::ALT) => {}
                KeyCode::Delete
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_word_forward();
                }
                KeyCode::Delete if key.modifiers.contains(KeyModifiers::CONTROL) => {}
                KeyCode::Backspace if !app.remove_selected_composer_attachment() => {
                    app.delete_char();
                }
                KeyCode::Backspace => {}
                KeyCode::Char('h')
                    if key_shortcuts::is_ctrl_h_backspace(&key)
                        && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_char();
                }
                KeyCode::Char('h') if key_shortcuts::is_ctrl_h_backspace(&key) => {}
                KeyCode::Delete if !app.remove_selected_composer_attachment() => {
                    app.delete_char_forward();
                }
                KeyCode::Delete => {}
                _ if key_shortcuts::is_select_all_shortcut(&key) => {
                    app.select_all();
                }
                KeyCode::Left
                    if key.modifiers.contains(KeyModifiers::SHIFT)
                        && is_word_cursor_modifier(key.modifiers) =>
                {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_word_backward();
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_left();
                }
                KeyCode::Left if is_word_cursor_modifier(key.modifiers) => {
                    app.clear_selection();
                    app.move_cursor_word_backward();
                }
                KeyCode::Left => {
                    app.clear_selection();
                    app.move_cursor_left();
                }
                KeyCode::Right
                    if key.modifiers.contains(KeyModifiers::SHIFT)
                        && is_word_cursor_modifier(key.modifiers) =>
                {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_word_forward();
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_right();
                }
                KeyCode::Right if is_word_cursor_modifier(key.modifiers) => {
                    app.clear_selection();
                    app.move_cursor_word_forward();
                }
                KeyCode::Right => {
                    app.clear_selection();
                    app.move_cursor_right();
                }
                // Selection-extending Home/End. Ctrl+Shift extends to the
                // buffer edge, bare Shift to the logical line edge. These sit
                // above the Ctrl+Home/Ctrl+End transcript-scroll arms so the
                // shifted chords always edit the selection, never the
                // viewport.
                KeyCode::Home
                    if key.modifiers.contains(KeyModifiers::SHIFT)
                        && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_start();
                }
                KeyCode::End
                    if key.modifiers.contains(KeyModifiers::SHIFT)
                        && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_end();
                }
                KeyCode::Home if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_line_start();
                }
                KeyCode::End if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    if app.selection_anchor.is_none() {
                        app.selection_anchor = Some(app.cursor_position);
                    }
                    app.move_cursor_line_end();
                }
                KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(anchor) =
                        TranscriptScroll::anchor_for(app.viewport.transcript_cache.line_meta(), 0)
                    {
                        app.viewport.transcript_scroll = anchor;
                    }
                }
                KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.scroll_to_bottom();
                }
                KeyCode::Home | KeyCode::Char('a')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.clear_selection();
                    app.move_cursor_start();
                }
                KeyCode::Home => {
                    app.clear_selection();
                    app.move_cursor_line_start();
                }
                KeyCode::End => {
                    app.clear_selection();
                    app.move_cursor_line_end();
                }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.clear_selection();
                    app.move_cursor_end();
                }
                _ if handle_composer_alt_word_motion_key(app, key) => {}
                _ if key_shortcuts::is_external_editor_shortcut(&key) => {
                    // Ctrl+Shift+O (or F4 on terminals that cannot report the
                    // shifted chord): spawn $EDITOR on the composer contents
                    // (#91). Plain Ctrl+O belongs exclusively to the Turn
                    // Inspector, even while the composer holds a draft (#4482).
                    // Only fires when no modal is active (the !view_stack
                    // branch above already returns early in that case) and
                    // the composer is the focused input target. We accept the
                    // shortcut whether or not a model turn is streaming —
                    // editing the buffer never disturbs in-flight work.
                    let seed = app.input.clone();
                    let editor_result = terminal_input.pause_for_child_terminal().and_then(|()| {
                        let result = drain_terminal_input_queue(
                            &terminal_input,
                            &mut pending_terminal_events,
                        )
                        .and_then(|()| {
                            super::external_editor::spawn_editor_for_input(
                                terminal,
                                app.use_alt_screen,
                                app.use_mouse_capture,
                                app.use_bracketed_paste,
                                &seed,
                            )
                        });
                        terminal_input.resume_after_child_terminal();
                        force_terminal_repaint = true;
                        result
                    });
                    match editor_result {
                        Ok(super::external_editor::EditorOutcome::Edited(new)) => {
                            app.input = new;
                            app.move_cursor_end();
                            let editor = std::env::var("VISUAL")
                                .ok()
                                .filter(|s| !s.trim().is_empty())
                                .or_else(|| {
                                    std::env::var("EDITOR")
                                        .ok()
                                        .filter(|s| !s.trim().is_empty())
                                })
                                .unwrap_or_else(|| "vi".to_string());
                            app.status_message = Some(format!("Edited in {editor}"));
                        }
                        Ok(super::external_editor::EditorOutcome::Unchanged) => {
                            app.status_message = Some("Editor closed (no changes)".to_string());
                        }
                        Ok(super::external_editor::EditorOutcome::Cancelled) => {
                            app.status_message = Some("Editor cancelled".to_string());
                        }
                        Err(err) => {
                            app.status_message = Some(format!("Editor error: {err}"));
                        }
                    }
                    app.needs_redraw = true;
                }
                KeyCode::Up => {
                    let _ =
                        handle_composer_history_arrow(app, key, slash_menu_open, mention_menu_open);
                }
                KeyCode::Down => {
                    let _ =
                        handle_composer_history_arrow(app, key, slash_menu_open, mention_menu_open);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.clear_input_recoverable();
                    let _ = app.maybe_show_behavioral_tip(
                        crate::tui::behavioral_tips::BehavioralTip::ClearedInputRestore,
                    );
                }
                KeyCode::Char('z')
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && app.restore_last_cleared_input_if_empty() =>
                {
                    app.status_message = Some("Restored cleared draft".to_string());
                }
                KeyCode::Char('w') | KeyCode::Char('W')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.delete_word_backward();
                }
                KeyCode::Char('s')
                | KeyCode::Char('S')
                | KeyCode::Char('g')
                | KeyCode::Char('G')
                    if key.modifiers == KeyModifiers::CONTROL =>
                {
                    if send_shortcut_queued_message_now(app, config, &engine_handle).await? {
                        continue;
                    }
                    // #440: park the current draft to the persistent stash and
                    // clear the composer. Ctrl+G is the terminal-safe alias for
                    // hosts such as Cursor/VS Code that reserve Ctrl+S for Save.
                    // Empty composers are a no-op so a stray shortcut cannot
                    // pollute the file. Surface a toast so the user sees the
                    // confirmation (no-op feels broken otherwise).
                    if !app.input.is_empty() {
                        crate::composer_stash::push_stash(&app.input);
                        app.clear_input_recoverable();
                        app.push_status_toast(
                            "Draft stashed — `/stash pop` to restore",
                            StatusToastLevel::Info,
                            Some(3_000),
                        );
                    }
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // #379: context-sensitive Ctrl+Y.
                    // When the composer has content → emacs-style yank
                    // from the kill buffer at the cursor.
                    // When the composer is empty (transcript focus) →
                    // copy the focused cell text to the system clipboard.
                    if app.input.is_empty() && app.view_stack.is_empty() {
                        if copy_focused_cell(app) {
                            app.push_status_toast(
                                "Copied to clipboard",
                                StatusToastLevel::Info,
                                Some(2_000),
                            );
                        } else {
                            app.status_message = Some("No transcript cell to copy".to_string());
                        }
                    } else {
                        app.yank();
                    }
                }
                KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let sel = app.selected_text();
                    if !sel.is_empty() {
                        if app.clipboard.write_text(&sel).is_ok() {
                            app.push_status_toast("Cut to clipboard", StatusToastLevel::Info, None);
                            app.delete_selection();
                        } else {
                            app.push_status_toast("Cut failed", StatusToastLevel::Error, None);
                        }
                    }
                }
                _ if key_shortcuts::is_paste_shortcut(&key) => {
                    app.paste_from_clipboard();
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_mode_update(app, &engine_handle, AppMode::Agent).await;
                    continue;
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_mode_update(app, &engine_handle, AppMode::Yolo).await;
                    continue;
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_mode_update(app, &engine_handle, AppMode::Plan).await;
                    continue;
                }
                KeyCode::Char('A') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_mode_update(app, &engine_handle, AppMode::Agent).await;
                    continue;
                }
                KeyCode::Char('Y') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_mode_update(app, &engine_handle, AppMode::Yolo).await;
                    continue;
                }
                KeyCode::Char('P') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_mode_update(app, &engine_handle, AppMode::Plan).await;
                    continue;
                }
                // Vim composer: Normal-mode motion / operator keys.
                // Only fires when vim is enabled, the input is focused (no modal
                // open on top), and the key has no modifier (pure char).
                KeyCode::Char(c)
                    if app.vim_is_normal_mode()
                        && key.modifiers.is_empty()
                        && !slash_menu_open
                        && !mention_menu_open
                        && app.view_stack.is_empty() =>
                {
                    vim_mode::handle_vim_normal_key(app, c);
                    continue;
                }
                // Vim composer: in Visual mode plain chars are ignored
                // (no text insertion until `i` / `a` enters Insert).
                KeyCode::Char(_)
                    if app.vim_is_visual_mode()
                        && key.modifiers.is_empty()
                        && app.view_stack.is_empty() =>
                {
                    // absorb — Visual mode not yet fully implemented
                }
                KeyCode::Char(c) if is_plain_char => {
                    app.insert_char(c);
                }
                KeyCode::Char(_) => {}
                _ => {}
            }

            if !is_plain_char && !is_enter {
                app.paste_burst.deactivate_keep_window();
            }
        }
    }
}

fn clear_work_inspector_after_pager_close(app: &mut App, was_work_inspector: bool) {
    if was_work_inspector && app.view_stack.top_kind() != Some(ModalKind::Pager) {
        app.work_surface.opened = None;
    }
}

fn apply_coordination_detail_projection(
    app: &mut App,
    projection: crate::tools::subagent::CoordinationDetailProjection,
) {
    app.coordination_detail = Some(projection);
}

/// The event-loop seam for Ctrl+T. Keeping the `KeyEvent` predicate and App
/// mutation together makes the real terminal route directly testable rather
/// than testing `cycle_effort` in isolation.
fn handle_reasoning_effort_key(app: &mut App, key: &event::KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
        || key.modifiers != KeyModifiers::CONTROL
    {
        return false;
    }
    app.cycle_effort();
    true
}

fn hotbar_slot_from_key(app: &App, key: &event::KeyEvent) -> Option<u8> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    if !('1'..='8').contains(&c) {
        return None;
    }
    let slot = c.to_digit(10).and_then(|digit| u8::try_from(digit).ok())?;

    if key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::SUPER)
    {
        if app.onboarding != OnboardingState::None
            || !app.view_stack.is_empty()
            || app.is_history_search_active()
            || app.decision_card.is_some()
            || !visible_slash_menu_entries(app, SLASH_MENU_LIMIT).is_empty()
        {
            return None;
        }

        return Some(slot);
    }

    None
}

fn decision_card_number_from_key(key: &event::KeyEvent) -> Option<usize> {
    let KeyCode::Char(c @ '1'..='9') = key.code else {
        return None;
    };
    if !key.modifiers.is_empty() {
        return None;
    }

    Some((c as u8 - b'1' + 1) as usize)
}

/// Let the transcript remain reviewable while an approval card owns focus.
fn handle_approval_transcript_key(app: &mut App, key: &event::KeyEvent) -> bool {
    if app.view_stack.top_kind() != Some(ModalKind::Approval) {
        return false;
    }

    let page = app.viewport.last_transcript_visible.max(1);
    match key.code {
        KeyCode::PageUp => app.scroll_up(page),
        KeyCode::PageDown => app.scroll_down(page),
        KeyCode::Up
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.scroll_up(3);
        }
        KeyCode::Down
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.scroll_down(3);
        }
        KeyCode::Home => app.scroll_up(usize::MAX),
        KeyCode::End => app.scroll_to_bottom(),
        _ => return false,
    }
    true
}

/// Route only non-text controls to a focused workflow panel.
///
/// Returning `false` for every character is deliberate: the caller then lets
/// the normal composer path insert it. A prior bare-letter contract used
/// t/c/j/k here, which made the first matching letter of a new chat disappear
/// after the user clicked the workflow card.
fn handle_workflow_panel_key(app: &mut App, key: &event::KeyEvent) -> bool {
    if !app
        .workflow_panel
        .as_ref()
        .is_some_and(|panel| panel.keyboard_focus)
    {
        return false;
    }

    if matches!(key.code, KeyCode::Char(_)) {
        if let Some(panel) = app.workflow_panel.as_mut() {
            panel.keyboard_focus = false;
        }
        app.needs_redraw = true;
        return false;
    }

    if !key.modifiers.is_empty() && key.code != KeyCode::Esc {
        return false;
    }

    match key.code {
        KeyCode::Esc => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.keyboard_focus = false;
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Enter => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                let _ = panel.toggle_expanded();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Down => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.select_next_phase();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Up => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.select_prev_phase();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Delete => {
            let Some(run_id) = app
                .workflow_panel
                .as_ref()
                .and_then(|panel| panel.lifecycle.is_running().then(|| panel.run_id.clone()))
            else {
                return false;
            };
            app.input = format!("/workflow cancel {run_id}");
            app.cursor_position = app.input.chars().count();
            app.status_message = Some(app.tr(MessageId::SidebarDestructiveArmed).into_owned());
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.keyboard_focus = false;
            }
            app.needs_redraw = true;
            true
        }
        _ => false,
    }
}

fn dispatch_hotbar_slot(
    app: &mut App,
    config: &Config,
    slot: u8,
) -> Result<Option<HotbarDispatch>> {
    let known_action_ids = app
        .hotbar_actions
        .iter()
        .map(|action| action.id())
        .collect::<Vec<_>>();
    let bindings = config.resolve_hotbar_bindings(&known_action_ids).bindings;
    let Some(action_id) = bindings
        .iter()
        .find(|binding| binding.slot == slot)
        .map(|binding| binding.action.clone())
    else {
        return Ok(None);
    };

    let Some(action) = app.hotbar_actions.get(&action_id) else {
        app.status_message = Some(format!(
            "Hotbar slot {slot} action is not available: {action_id}"
        ));
        app.needs_redraw = true;
        return Ok(Some(HotbarDispatch::Handled));
    };

    if let Some(reason) = action.disabled_reason(app) {
        app.status_message = Some(format!(
            "Hotbar slot {slot} action is not available: {reason}"
        ));
        app.needs_redraw = true;
        return Ok(Some(HotbarDispatch::Handled));
    }

    action.dispatch(app).map(Some)
}

fn apply_alt_4_shortcut(app: &mut App, _modifiers: KeyModifiers) {
    app.set_sidebar_focus(SidebarFocus::Agents);
    app.status_message = Some("Sidebar focus: agents".to_string());
}

fn persist_sidebar_settings_if_dirty(app: &mut App) {
    if !app.sidebar_width_dirty && !app.sidebar_focus_dirty {
        return;
    }

    let width_dirty = app.sidebar_width_dirty;
    let focus_dirty = app.sidebar_focus_dirty;
    app.sidebar_width_dirty = false;
    app.sidebar_focus_dirty = false;

    if let Ok(mut settings) = Settings::load_persisted() {
        if width_dirty {
            settings.update_sidebar_width(app.sidebar_width_percent);
        }
        if focus_dirty {
            let _ = settings.set("sidebar_focus", app.sidebar_focus.as_setting());
        }
        let _ = settings.save();
    }
}

fn apply_alt_0_shortcut(app: &mut App, modifiers: KeyModifiers) {
    if modifiers.contains(KeyModifiers::CONTROL) {
        if app.sidebar_focus == SidebarFocus::Hidden {
            app.set_sidebar_focus(SidebarFocus::Pinned);
            app.status_message = Some("Sidebar focus: pinned".to_string());
        } else {
            app.set_sidebar_focus(SidebarFocus::Hidden);
            app.status_message = Some("Sidebar hidden".to_string());
        }
    } else {
        app.set_sidebar_focus(SidebarFocus::Auto);
        app.status_message = Some("Sidebar focus: auto".to_string());
    }
}

async fn fetch_available_models(config: &Config) -> Result<Vec<String>> {
    use crate::client::DeepSeekClient;

    let client = DeepSeekClient::new(config)?;
    let models = tokio::time::timeout(Duration::from_secs(20), client.list_models()).await??;
    let mut ids = models.into_iter().map(|model| model.id).collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

async fn run_cache_warmup(app: &App, config: &Config) -> Result<(Usage, String, PromptInspection)> {
    let client = DeepSeekClient::new(config)?;
    let base_url = client.base_url().to_string();
    let reasoning_effort = if app.reasoning_effort == ReasoningEffort::Auto {
        app.last_effective_reasoning_effort
            .and_then(|effort| effort.api_value_for_route(app.api_provider, &base_url, &app.model))
            .map(str::to_string)
    } else {
        app.reasoning_effort
            .api_value_for_route(app.api_provider, &base_url, &app.model)
            .map(str::to_string)
    };
    let request = MessageRequest {
        model: app.model.clone(),
        messages: app.api_messages.clone(),
        max_tokens: 1024,
        system: app.system_prompt.clone(),
        tools: app.session.last_tool_catalog.clone(),
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: None,
        temperature: None,
        top_p: None,
    };
    let warmup = build_cache_warmup_request(&request);
    let inspection = inspect_prompt_for_request(&warmup);
    let response =
        tokio::time::timeout(Duration::from_secs(45), client.create_message(warmup)).await??;
    Ok((response.usage, base_url, inspection))
}

/// One-shot "draft my constitution" call against the user's first configured
/// model, requested by `A` on the setup Constitution card. Runs inline in the
/// event loop like [`fetch_available_models`] (the wizard modal stays open
/// underneath) with a hard timeout so a slow provider cannot wedge setup.
///
/// On success the sanitized, bounded draft is installed into the open wizard
/// and its ratification preview opens on top — nothing persists until the
/// user ratifies with `G`. Every failure (no client, timeout, request error,
/// invalid or empty JSON) is a status line, never an error state: the
/// deterministic guided draft remains the standing fallback.
async fn handle_setup_constitution_model_draft(
    app: &mut App,
    config: &Config,
    draft: crate::tui::setup::GuidedConstitutionDraft,
    freeform_note: Option<String>,
    locale: crate::localization::Locale,
) {
    // Spawn the draft off the event loop (same pattern as the fleet drafter,
    // #3757 review): awaiting it inline parked the whole TUI for up to the
    // timeout. The loop polls constitution_draft_cell and delivers the result.
    const DRAFT_TIMEOUT: Duration = Duration::from_secs(20);
    let model_label = app.model_display_label();
    let client = match DeepSeekClient::new(config) {
        Ok(client) => client,
        Err(err) => {
            deliver_constitution_draft_result(
                app,
                model_label.clone(),
                locale,
                Err(format!("provider not ready: {err:#}")),
            );
            return;
        }
    };
    let request_model = app.model.clone();
    let cell = app.constitution_draft_cell.clone();
    let spawn_label = model_label.clone();
    let request_gen = app.next_draft_gen();
    app.status_message = Some(match locale {
        crate::localization::Locale::ZhHans => {
            format!(
                "{model_label} 正在生成协作准则草案……（最多 {}s）",
                DRAFT_TIMEOUT.as_secs()
            )
        }
        _ => format!(
            "{model_label} is drafting your constitution… (up to {}s)",
            DRAFT_TIMEOUT.as_secs()
        ),
    });
    app.needs_redraw = true;
    tokio::spawn(async move {
        let outcome = match tokio::time::timeout(
            DRAFT_TIMEOUT,
            crate::tui::setup::draft_constitution_with_model(
                &client,
                &request_model,
                draft,
                freeform_note,
                locale,
            ),
        )
        .await
        {
            Err(_) => Err(format!("timed out after {}s", DRAFT_TIMEOUT.as_secs())),
            Ok(result) => result,
        };
        if let Ok(mut guard) = cell.lock() {
            *guard = Some((request_gen, spawn_label, locale, outcome));
        }
    });
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
/// One-shot fleet-profile draft: same contract as the constitution drafter —
/// minimal payload out, untrusted gate in, preview before ratify, degrade to
/// the manual authoring flow on any failure.
async fn handle_fleet_profile_model_draft(
    app: &mut App,
    config: &Config,
    role: String,
    model: String,
    provider: Option<String>,
    reasoning_effort: Option<String>,
    locale: crate::localization::Locale,
) {
    // The route the operator actually picked at `m`-press time (#4093). A
    // model draft always comes back `provider: None` (the untrusted gate
    // strips any provider), so this captured `(provider, model)` is what the
    // ratified profile is pinned to — immune to the model omitting/altering
    // the route AND to the selection changing while the draft is in flight.
    // `None` for an `inherit` pick (no concrete route to keep).
    let picked_route = provider.map(|provider| (provider, model.clone()));
    // Do NOT await the network call on the event loop — that parks the whole
    // TUI for up to the timeout (#3757 review). Spawn it into the shared
    // fleet_draft_cell and let the loop poll + deliver the result, keeping
    // the wizard interactive with a drafting status.
    const DRAFT_TIMEOUT: Duration = Duration::from_secs(20);
    let model_label = app.model_display_label();
    let client = match DeepSeekClient::new(config) {
        Ok(client) => client,
        Err(err) => {
            deliver_fleet_draft_result(
                app,
                model_label.clone(),
                picked_route.clone(),
                reasoning_effort.clone(),
                Err(format!("provider not ready: {err:#}")),
                locale,
            );
            return;
        }
    };
    let request_model = app.model.clone();
    let cell = app.fleet_draft_cell.clone();
    let spawn_label = model_label.clone();
    let request_gen = app.next_draft_gen();
    let workspace = app.workspace.clone();
    app.status_message = Some(match locale {
        crate::localization::Locale::ZhHans => {
            format!(
                "{model_label} 正在起草配置……（最多 {}s）",
                DRAFT_TIMEOUT.as_secs()
            )
        }
        _ => format!(
            "{model_label} is drafting the profile… (up to {}s)",
            DRAFT_TIMEOUT.as_secs()
        ),
    });
    app.needs_redraw = true;
    tokio::spawn(async move {
        // Redacted, bounded workspace fingerprint (manifest names, test
        // commands, branch + dirty count — no contents, secrets, or absolute
        // paths). Computed off the event loop; the untrusted-output gate on
        // the reply is unchanged.
        let fingerprint = tokio::task::spawn_blocking(move || {
            crate::tui::setup::workspace_fingerprint(&workspace)
        })
        .await
        .unwrap_or_default();
        let outcome = match tokio::time::timeout(
            DRAFT_TIMEOUT,
            crate::tui::setup::draft_fleet_profile_with_model(
                &client,
                &request_model,
                &role,
                &model,
                locale,
                &fingerprint,
            ),
        )
        .await
        {
            Err(_) => Err(format!("timed out after {}s", DRAFT_TIMEOUT.as_secs())),
            Ok(result) => result,
        };
        if let Ok(mut guard) = cell.lock() {
            *guard = Some((
                request_gen,
                spawn_label,
                picked_route,
                reasoning_effort,
                outcome,
            ));
        }
    });
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

fn build_session_snapshot(
    app: &mut App,
    _manager: &SessionManager,
) -> Result<SavedSession, String> {
    let model = app.model_selection_for_persistence();
    let work_state = match app.try_work_state_snapshot() {
        Ok(work_state) => work_state,
        Err(err) => app.last_known_work_state.clone().ok_or_else(|| {
            format!("automatic session snapshot skipped while Work state is busy: {err}")
        })?,
    };
    let mut session = if let Some(existing_id) = app.current_session_id.as_ref() {
        create_saved_session_with_id_and_mode(
            existing_id.clone(),
            &app.api_messages,
            &model,
            &app.workspace,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
            Some(app.mode.as_setting()),
        )
    } else {
        create_saved_session_with_mode(
            &app.api_messages,
            &model,
            &app.workspace,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
            Some(app.mode.as_setting()),
        )
    };
    if let Some(cached) = app
        .current_session_metadata
        .as_ref()
        .filter(|cached| cached.id == session.metadata.id)
    {
        session.metadata.created_at = cached.created_at;
        session.metadata.title.clone_from(&cached.title);
        session
            .metadata
            .parent_session_id
            .clone_from(&cached.parent_session_id);
        session.metadata.forked_from_message_count = cached.forked_from_message_count;
    }
    session
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    app.sync_cost_to_metadata(&mut session.metadata);
    session.context_references = app.session_context_references.clone();
    session.artifacts = app.session_artifacts.clone();
    session.work_state = work_state;
    session.last_auto_route = app.auto_route_for_persistence();
    app.current_session_metadata = Some(session.metadata.clone());
    Ok(session)
}

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

async fn publish_pending_work_projection(app: &mut App) -> Result<bool, String> {
    let Some(work) = app.runtime_services.work.clone() else {
        return Ok(false);
    };
    let published = work.publish_pending().await?;
    if published {
        app.cached_work_summary = None;
    }
    Ok(published)
}

async fn persist_pending_work_checkpoint(app: &mut App) -> Result<bool, String> {
    let Some(work) = app.runtime_services.work.clone() else {
        return Ok(false);
    };
    if !work.has_pending_publish() {
        return Ok(false);
    }
    let manager = SessionManager::default_location()
        .map_err(|err| format!("could not open sessions directory: {err}"))?;
    let session = build_session_snapshot(app, &manager)?;
    if app.current_session_id.is_none() {
        app.current_session_id = Some(session.metadata.id.clone());
    }
    if !persistence_actor::try_persist(PersistRequest::SaveCheckpoint { session }) {
        return Err("persistence actor is unavailable".to_string());
    }
    publish_pending_work_projection(app).await
}

fn persist_with_pending_work_boundary(
    app: &mut App,
    request: PersistRequest,
) -> Result<(), String> {
    let has_pending = app
        .runtime_services
        .work
        .as_ref()
        .is_some_and(|work| work.has_pending_publish());
    if !has_pending {
        persistence_actor::persist(request);
        return Ok(());
    }
    if !persistence_actor::try_persist(request) {
        return Err("persistence actor is unavailable".to_string());
    }
    app.publish_pending_work_state().map(|_| ())
}

fn apply_picker_session_rename_to_active_app(
    app: &mut App,
    metadata: crate::session_manager::SessionMetadata,
) -> bool {
    if app.current_session_id.as_deref() != Some(metadata.id.as_str()) {
        return false;
    }
    app.session_title = Some(metadata.title.clone());
    app.current_session_metadata = Some(metadata);
    true
}

fn queued_ui_to_session(msg: &QueuedMessage) -> QueuedSessionMessage {
    QueuedSessionMessage {
        display: msg.display.clone(),
        skill_instruction: msg.skill_instruction.clone(),
        skill_provenance: msg.skill_provenance.clone(),
    }
}

fn queued_session_to_ui(msg: QueuedSessionMessage) -> QueuedMessage {
    QueuedMessage {
        display: msg.display,
        skill_instruction: msg.skill_instruction,
        skill_provenance: msg.skill_provenance,
    }
}

fn reconcile_turn_liveness(app: &mut App, now: Instant, has_running_agents: bool) -> bool {
    if app.is_loading
        && app.runtime_turn_status.is_none()
        && !has_running_agents
        && !app.is_compacting
        && !app.is_purging
        && app.dispatch_started_at.is_some_and(|started| {
            now.saturating_duration_since(started) > DISPATCH_WATCHDOG_TIMEOUT
        })
    {
        // #2739: the user's prompt was already appended to api_messages
        // before dispatch, but the turn never reached `in_progress`. Persist
        // it before clearing turn state so `--continue` keeps the prompt
        // instead of loading the previous save.
        persist_recovery_snapshot(app);
        app.is_loading = false;
        app.dispatch_started_at = None;
        app.turn_started_at = None;
        app.turn_last_activity_at = None;
        app.pending_turn_route = None;
        app.pending_auto_route_receipt = None;
        app.active_turn = None;
        app.suppress_stream_events_until_turn_complete = false;
        app.push_status_toast(
            "Turn dispatch timed out; the engine may have stopped. Please try again.",
            StatusToastLevel::Error,
            None,
        );
        return true;
    }

    if app.is_loading
        && matches!(
            app.runtime_turn_status.as_deref(),
            Some("completed" | "interrupted" | "failed")
        )
        && !has_running_agents
        && !app.is_compacting
        && !app.is_purging
    {
        app.is_loading = false;
        app.dispatch_started_at = None;
        app.turn_started_at = None;
        app.turn_last_activity_at = None;
        app.pending_turn_route = None;
        app.pending_auto_route_receipt = None;
        app.active_turn = None;
        app.suppress_stream_events_until_turn_complete = false;
        app.push_status_toast(
            "Recovered from an inconsistent busy state.",
            StatusToastLevel::Warning,
            None,
        );
        return true;
    }

    // Branch 3: turn started but never completed — engine may have
    // panicked, sub-agent may be stuck, or the completion event was lost.
    if app.is_loading
        && matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        && !has_running_agents
        && !app.is_compacting
        && !active_turn_has_running_tool(app)
        && app
            .turn_last_activity_at
            .or(app.turn_started_at)
            .is_some_and(|last_activity| {
                now.saturating_duration_since(last_activity) > turn_stall_watchdog_timeout(app)
            })
    {
        recover_stalled_runtime_turn(
            app,
            "Turn stalled — no completion signal received. Please try again.",
            StatusToastLevel::Error,
        );
        return true;
    }

    if app.is_loading
        && matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        && !has_running_agents
        && !app.is_compacting
        && !app.is_purging
        && active_turn_has_running_tool(app)
        && app
            .turn_last_activity_at
            .or(app.turn_started_at)
            .is_some_and(|last_activity| {
                now.saturating_duration_since(last_activity) > TOOL_HANG_WATCHDOG_TIMEOUT
            })
    {
        recover_stalled_runtime_turn(
            app,
            "Tool stalled with no progress for 10m — recovered; the command may still be running in the background. Use exec_shell_cancel or retry.",
            StatusToastLevel::Error,
        );
        return true;
    }

    false
}

fn turn_stall_watchdog_timeout(app: &App) -> Duration {
    let stream_budget = Duration::from_secs(app.stream_chunk_timeout_secs)
        .saturating_add(TURN_STALL_WATCHDOG_GRACE);
    TURN_STALL_WATCHDOG_TIMEOUT.max(stream_budget)
}

/// #2739: persist the current in-memory session state before a recovery or
/// cancellation path clears turn bookkeeping. Without this snapshot, the
/// just-finalised partial turn lives only in `app.api_messages` and is never
/// written to disk, so `--continue` loads the *previous* save — effectively
/// losing the entire in-progress turn.
fn persist_recovery_snapshot(app: &mut App) {
    if let Ok(manager) = SessionManager::default_location()
        && let Ok(session) = build_session_snapshot(app, &manager)
    {
        if app.current_session_id.is_none() {
            app.current_session_id = Some(session.metadata.id.clone());
        }
        if let Err(err) =
            persist_with_pending_work_boundary(app, PersistRequest::SessionSnapshot(session))
        {
            app.status_message = Some(format!(
                "Work update is pending: recovery snapshot could not be queued ({err})"
            ));
        }
    }
}

fn persist_full_reset_snapshot(app: &mut App) {
    if let Ok(manager) = SessionManager::default_location()
        && let Ok(session) = build_session_snapshot(app, &manager)
    {
        app.current_session_id = Some(session.metadata.id.clone());
        if let Err(err) =
            persist_with_pending_work_boundary(app, PersistRequest::SessionSnapshot(session))
        {
            app.status_message = Some(format!(
                "Work update is pending: reset snapshot could not be queued ({err})"
            ));
        }
    }
    // `/clear` and `/new` are explicit boundaries. Never let an older
    // in-flight checkpoint resurrect the session the user just discarded,
    // even if the replacement snapshot could not be constructed.
    // `build_session_snapshot` reuses `current_session_id`, so this id is the
    // discarded session's id whether or not the snapshot above succeeded.
    if let Some(session_id) = app.current_session_id.clone() {
        persistence_actor::persist(PersistRequest::ClearCheckpoint { session_id });
    }
}

fn maybe_throttled_recovery_snapshot(
    app: &mut App,
    now: Instant,
    last_snapshot_at: &mut Option<Instant>,
) {
    if !app.is_loading && !matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
        return;
    }
    if last_snapshot_at
        .is_some_and(|last| now.saturating_duration_since(last) < RECOVERY_SNAPSHOT_INTERVAL)
    {
        return;
    }
    persist_recovery_snapshot(app);
    *last_snapshot_at = Some(now);
}

fn enqueue_offline_message(app: &mut App, message: QueuedMessage) {
    app.queue_message(message);
    persist_offline_queue_state(app);
}

fn recover_stalled_runtime_turn(app: &mut App, message: &str, level: StatusToastLevel) {
    // Finalize in-flight thinking / assistant / tool cells so the
    // transcript doesn't show permanent spinners after recovery.
    streaming_thinking::finalize_current(app);
    app.finalize_streaming_assistant_as_interrupted();
    app.finalize_active_cell_as_interrupted();
    app.streaming_state.reset();
    app.streaming_message_index = None;
    app.streaming_thinking_active_entry = None;

    // #2739: persist the partial turn's api_messages before clearing
    // turn state. Without this snapshot the stalled/cancelled turn's
    // messages are held only in memory and --continue sees the
    // *previous* save, losing the entire in-progress turn.
    persist_recovery_snapshot(app);

    app.is_loading = false;
    app.turn_started_at = None;
    app.turn_last_activity_at = None;
    app.runtime_turn_status = None;
    app.runtime_turn_id = None;
    app.dispatch_started_at = None;
    app.pending_turn_route = None;
    app.pending_auto_route_receipt = None;
    app.active_turn = None;
    app.suppress_stream_events_until_turn_complete = false;
    // Per-turn scroll lock — clear so the next turn auto-scrolls.
    app.user_scrolled_during_stream = false;
    app.push_status_toast(message, level, None);
}

/// #3033: gate progress-driven repaints to at most one per 100ms.
///
/// Returns whether the current `AgentProgress` event may request a redraw,
/// updating the last-redraw timestamp when it may. Data updates are never
/// throttled — only the repaint request is.
fn agent_progress_redraw_permitted(last_redraw: &mut Option<Instant>, now: Instant) -> bool {
    match *last_redraw {
        Some(last) if now.duration_since(last) < Duration::from_millis(100) => false,
        _ => {
            *last_redraw = Some(now);
            true
        }
    }
}

/// #4095 residual: pace workflow budget-only repaints under fan-out.
///
/// Same 100ms floor as AgentProgress. High-signal workflow lifecycle events
/// bypass this gate and always paint.
fn workflow_budget_redraw_permitted(last_redraw: &mut Option<Instant>, now: Instant) -> bool {
    agent_progress_redraw_permitted(last_redraw, now)
}

fn agent_progress_redraw_permitted_for_drain(
    last_redraw: &mut Option<Instant>,
    seen_agents: &mut HashSet<String>,
    agent_id: &str,
    now: Instant,
) -> bool {
    if !seen_agents.insert(agent_id.to_string()) {
        return false;
    }
    agent_progress_redraw_permitted(last_redraw, now)
}

fn recover_engine_event_disconnect(app: &mut App) -> bool {
    let had_live_work = app.is_loading
        || app.is_compacting
        || app.is_purging
        || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        || app.pending_turn_route.is_some()
        || app.active_turn.is_some()
        || app.suppress_stream_events_until_turn_complete
        || app.streaming_message_index.is_some()
        || app.streaming_thinking_active_entry.is_some()
        || app
            .active_cell
            .as_ref()
            .is_some_and(|cell| !cell.is_empty());

    if !had_live_work {
        return false;
    }

    streaming_thinking::finalize_current(app);
    app.finalize_streaming_assistant_as_interrupted();
    app.finalize_active_cell_as_interrupted();
    app.streaming_state.reset();
    app.streaming_message_index = None;
    app.streaming_thinking_active_entry = None;

    // #2739: persist partial turn before clearing state.
    persist_recovery_snapshot(app);

    app.is_loading = false;
    app.is_compacting = false;
    app.is_purging = false;
    app.turn_started_at = None;
    app.turn_last_activity_at = None;
    app.runtime_turn_status = None;
    app.runtime_turn_id = None;
    app.dispatch_started_at = None;
    app.pending_turn_route = None;
    app.pending_auto_route_receipt = None;
    app.active_turn = None;
    app.suppress_stream_events_until_turn_complete = false;
    app.user_scrolled_during_stream = false;

    for msg in app.drain_pending_steers() {
        app.queue_message(msg);
    }

    app.add_message(HistoryCell::Error {
        message: "Engine stopped before completing the turn. Check ~/.codewhale/crashes and retry."
            .to_string(),
        severity: crate::error_taxonomy::ErrorSeverity::Error,
    });
    app.push_status_toast(
        "Engine stopped before completing the turn.",
        StatusToastLevel::Error,
        None,
    );
    true
}

fn capture_turn_started_metadata(app: &mut App, event: &EngineEvent) {
    if let EngineEvent::TurnStarted {
        turn_id,
        created_at,
        route,
    } = event
    {
        app.ocean_completion_started_at = None;
        let auto_route_receipt = if route.as_ref().is_some_and(|route| route.auto_model) {
            app.pending_auto_route_receipt.take()
        } else {
            app.pending_auto_route_receipt = None;
            None
        };
        app.active_turn = Some(ActiveTurnMetadata {
            turn_id: turn_id.clone(),
            created_at: *created_at,
            route: route.clone(),
            auto_route_receipt,
        });
        app.pending_turn_route = None;
    }
}

fn error_health_route(app: &App, fallback_provider: ApiProvider) -> (ApiProvider, String) {
    app.active_turn
        .as_ref()
        .and_then(|turn| turn.route.as_ref())
        .map(|route| (route.provider, route.model.clone()))
        .or_else(|| {
            app.pending_turn_route
                .as_ref()
                .map(|(provider, model, _)| (*provider, model.clone()))
        })
        .unwrap_or_else(|| (fallback_provider, app.model.clone()))
}

fn record_turn_activity(app: &mut App, event: &EngineEvent, now: Instant) {
    if matches!(event, EngineEvent::TurnStarted { .. }) {
        app.turn_last_activity_at = Some(now);
        return;
    }

    if app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
        app.turn_last_activity_at = Some(now);
    }
}

fn active_turn_has_running_tool(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|cell| match cell {
            HistoryCell::Tool(tool) => tool_cell_is_running(tool),
            _ => false,
        })
    })
}

fn terminal_input_recovery_relevant(app: &App, has_running_agents: bool) -> bool {
    app.is_loading
        || has_running_agents
        || app.is_compacting
        || app.is_purging
        || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        || active_turn_has_running_tool(app)
}

fn tool_cell_is_running(tool: &ToolCell) -> bool {
    match tool {
        ToolCell::Exec(cell) => cell.status == ToolStatus::Running,
        ToolCell::Exploring(cell) => cell
            .entries
            .iter()
            .any(|entry| entry.status == ToolStatus::Running),
        ToolCell::PlanUpdate(cell) => cell.status == ToolStatus::Running,
        ToolCell::PatchSummary(cell) => cell.status == ToolStatus::Running,
        ToolCell::Review(cell) => cell.status == ToolStatus::Running,
        ToolCell::DiffPreview(_) => false,
        ToolCell::Mcp(cell) => cell.status == ToolStatus::Running,
        ToolCell::ViewImage(_) => false,
        ToolCell::WebSearch(cell) => cell.status == ToolStatus::Running,
        ToolCell::Generic(cell) => cell.status == ToolStatus::Running,
    }
}

/// Translate an `EngineEvent::Error` into UI state updates.
///
/// The engine's `recoverable` flag (mirrored on `ErrorEnvelope`) decides
/// whether the session flips into offline mode: stream stalls, chunk
/// timeouts, transient network errors, and rate-limit/server hiccups arrive
/// recoverable and must NOT flip into offline. Hard failures (auth, billing,
/// invalid request) arrive non-recoverable; those flip offline so subsequent
/// messages get queued instead of silently lost mid-flight.
///
/// `severity` drives transcript color: red for `Error`/`Critical`, amber for
/// `Warning`, dim for `Info`.
pub(crate) fn apply_engine_error_to_app(
    app: &mut App,
    envelope: crate::error_taxonomy::ErrorEnvelope,
) {
    let recoverable = envelope.recoverable;
    let message = envelope.message.clone();
    let severity = envelope.severity;
    let turn_was_in_progress =
        app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"));
    streaming_thinking::finalize_current(app);
    if turn_was_in_progress {
        app.finalize_streaming_assistant_as_interrupted();
        app.finalize_active_cell_as_interrupted();
        app.runtime_turn_status = Some("failed".to_string());
    }
    app.streaming_state.reset();
    app.streaming_message_index = None;
    app.streaming_thinking_active_entry = None;

    // #455 (observer-only): fire `on_error` hooks so operators can
    // page on auth / billing / invalid-request failures without
    // tailing the audit log. Read-only — the hook can react but not
    // suppress the error from reaching the transcript. Fast-path
    // skip when no hooks configured.
    if app
        .hooks
        .has_hooks_for_event(crate::hooks::HookEvent::OnError)
    {
        let context = app.base_hook_context().with_error(&message);
        let _ = app.execute_hooks(crate::hooks::HookEvent::OnError, &context);
    }

    app.add_message(HistoryCell::Error {
        message: message.clone(),
        severity,
    });
    app.is_loading = false;
    app.dispatch_started_at = None;
    app.turn_error_posted = true;
    if matches!(
        envelope.category,
        crate::error_taxonomy::ErrorCategory::Authentication
    ) && app.api_key_env_only
    {
        app.offline_mode = true;
        app.onboarding_needs_api_key = true;
        app.onboarding = OnboardingState::Provider;
        app.status_message = Some(
            "The API key from DEEPSEEK_API_KEY was rejected. Paste a valid key to save it to ~/.codewhale/config.toml, or update the environment variable.".to_string(),
        );
        return;
    }
    if recoverable
        && matches!(
            envelope.category,
            crate::error_taxonomy::ErrorCategory::Network
                | crate::error_taxonomy::ErrorCategory::RateLimit
                | crate::error_taxonomy::ErrorCategory::Timeout
        )
        && app.advance_fallback(message.clone()).is_some()
    {
        let position = app.fallback_chain_position().unwrap_or(0);
        let total = app.fallback_chain_len();
        app.status_message = Some(format!(
            "Switched to {} (fallback {position}/{}) after recoverable provider error.",
            app.api_provider.as_str(),
            total.saturating_sub(1)
        ));
        return;
    }
    if !recoverable {
        app.offline_mode = true;
    }
    // Error is already in the transcript as HistoryCell::Error above;
    // don't emit a redundant status_message that would become a sticky
    // toast in the footer — that duplicates the transcript entry.
}

fn rollback_provider_after_auth_failure(app: &mut App, config: &mut Config) -> Option<String> {
    let pending = app.pending_provider_switch.take()?;
    let PendingProviderSwitch {
        previous_provider,
        previous_model,
        previous_model_ids_passthrough,
        previous_route_limits,
        previous_route_base_url,
        previous_context_window_source,
        previous_context_window_override,
        previous_config,
        previous_onboarding,
        previous_onboarding_needs_api_key,
        previous_api_key_env_only,
    } = pending;

    *config = previous_config;
    if let Ok(identity) = config.active_provider_identity(previous_provider) {
        app.set_provider_identity_record(identity);
    } else {
        app.set_provider_identity(
            previous_provider,
            config.provider_identity_for(previous_provider),
        );
    }
    app.billing_presentation = crate::route_billing::for_route(config, previous_provider);
    app.set_model_selection(previous_model.clone());
    app.provider_models.insert(
        app.provider_identity_for_persistence().to_string(),
        previous_model,
    );
    app.model_ids_passthrough = previous_model_ids_passthrough;
    app.active_context_window_override = previous_context_window_override;
    app.active_route_limits = previous_route_limits;
    app.active_route_base_url = previous_route_base_url;
    app.active_context_window_source = previous_context_window_source;
    app.update_model_compaction_budget();
    app.clear_model_scoped_telemetry();
    app.offline_mode = false;
    app.onboarding = previous_onboarding;
    app.onboarding_needs_api_key = previous_onboarding_needs_api_key;
    app.api_key_env_only = previous_api_key_env_only;

    let mut persistence_errors = Vec::new();
    if let Err(err) = (|| -> anyhow::Result<()> {
        crate::config_persistence::persist_root_string_key(
            app.config_path.as_deref(),
            "provider",
            app.provider_identity_for_persistence(),
        )?;
        let mut settings = crate::settings::Settings::load_persisted()?;
        settings.default_provider = Some(app.provider_identity_for_persistence().to_string());
        settings.set_model_for_provider(
            app.provider_identity_for_persistence(),
            &app.model_selection_for_persistence(),
        );
        if matches!(
            previous_provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN
        ) {
            settings.set("default_model", &app.model_selection_for_persistence())?;
        }
        settings.save()?;
        Ok(())
    })() {
        persistence_errors.push(err.to_string());
    }
    if let Err(err) = crate::tui::setup::record_provider_model_setup_state_for_app(app, config) {
        persistence_errors.push(format!("setup state was not saved: {err}"));
    }
    let persistence_error = if persistence_errors.is_empty() {
        None
    } else {
        Some(format!(
            "provider rollback not fully persisted: {}",
            persistence_errors.join("; ")
        ))
    };

    Some(match persistence_error {
        Some(warning) => format!(
            "Provider switch failed and has been rolled back to {}. {}",
            previous_provider.as_str(),
            warning
        ),
        None => format!(
            "Provider switch failed and has been rolled back to {}.",
            previous_provider.as_str()
        ),
    })
}

fn persist_offline_queue_state(app: &App) {
    if app.queued_messages.is_empty() && app.queued_draft.is_none() {
        persistence_actor::persist(PersistRequest::ClearOfflineQueue);
        return;
    }
    let state = OfflineQueueState {
        messages: app
            .queued_messages
            .iter()
            .map(queued_ui_to_session)
            .collect(),
        draft: app.queued_draft.as_ref().map(queued_ui_to_session),
        ..OfflineQueueState::default()
    };
    persistence_actor::persist(PersistRequest::OfflineQueue {
        state,
        session_id: app.current_session_id.clone(),
    });
}

/// Strip ANSI control codes / non-printable bytes from a streaming
/// text chunk. `pub(super)` because `tui::notifications` consumes it
/// from `super::ui` for its per-turn message composition.
pub(super) fn sanitize_stream_chunk(chunk: &str) -> String {
    // Keep printable characters and common whitespace; drop control bytes.
    chunk
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

// Per-turn notification composition (settings, message body, summary)
// moved to `tui/notifications.rs` alongside the dispatch primitives.

/// Ensure an in-flight streaming Assistant cell exists in history and return
/// its index. Thinking cells go through `streaming_thinking::ensure_active_entry`
/// (active cell) instead.
fn ensure_streaming_assistant_history_cell(app: &mut App) -> usize {
    if let Some(index) = app.streaming_message_index {
        return index;
    }
    app.add_message(HistoryCell::Assistant {
        content: String::new(),
        streaming: true,
    });
    let index = app.history.len().saturating_sub(1);
    app.streaming_message_index = Some(index);
    index
}

fn append_streaming_text(app: &mut App, index: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(HistoryCell::Assistant { content, .. }) = app.history.get_mut(index) {
        content.push_str(text);
        // Bump only the streaming cell's per-cell revision so the transcript
        // cache re-renders just this cell. Without this, the cache would
        // either skip the update entirely (now that the global
        // history_version is no longer fanned out across every cell) or fall
        // back to a full re-wrap of the entire transcript every chunk.
        app.bump_history_cell(index);
    }
}

fn accrue_streaming_token_estimate(app: &mut App, visible_text: &str) {
    if visible_text.is_empty() {
        return;
    }
    app.streaming_output_token_estimate = app
        .streaming_output_token_estimate
        .saturating_add(estimate_output_tokens_from_text(visible_text));
}

fn commit_streaming_display_tick(
    app: &mut App,
    stream_display_clock: &mut StreamDisplayClock,
    now: Instant,
) -> bool {
    if !stream_display_clock.take_due(now) {
        return false;
    }

    let mut updated = false;
    if let Some(index) = app.streaming_message_index {
        let committed = app.streaming_state.commit_text(0);
        if !committed.is_empty() {
            append_streaming_text(app, index, &committed);
            accrue_streaming_token_estimate(app, &committed);
            updated = true;
        }
    } else if let Some(entry_idx) = app.streaming_thinking_active_entry {
        let committed = app.streaming_state.commit_text(0);
        if !committed.is_empty() {
            if app.translation_enabled {
                streaming_thinking::set_placeholder(app, entry_idx);
            } else {
                streaming_thinking::append(app, entry_idx, &committed);
            }
            updated = true;
        }
    }

    if app.streaming_state.has_pending_chunker_lines(0) {
        stream_display_clock.note_delta(now);
    }

    updated
}

fn push_assistant_message(
    app: &mut App,
    text: String,
    thinking: Option<String>,
    tool_uses: PendingToolUses,
) {
    let mut blocks = Vec::new();
    if let Some(thinking) = thinking {
        blocks.push(ContentBlock::Thinking {
            thinking,
            signature: None,
        });
    }
    if !text.is_empty() {
        blocks.push(ContentBlock::Text {
            text,
            cache_control: None,
        });
    }
    for (id, name, input) in tool_uses {
        blocks.push(ContentBlock::ToolUse {
            id,
            name,
            input,
            caller: None,
        });
    }

    let has_sendable_content = blocks.iter().any(|block| {
        matches!(
            block,
            ContentBlock::Text { .. } | ContentBlock::ToolUse { .. }
        )
    });
    if has_sendable_content {
        app.api_messages.push(Message {
            role: "assistant".to_string(),
            content: blocks,
        });
    }
}

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

fn live_tool_receipt_messages(app: &App, id: &str, raw: &str, success: bool) -> Vec<Message> {
    let mut messages = Vec::with_capacity(2);
    if let Some(tool_use_msg) = app.api_messages.iter().rev().find(|message| {
        message.content.iter().any(|block| {
            matches!(block, ContentBlock::ToolUse { id: tool_use_id, .. } if tool_use_id == id)
        })
    }) {
        messages.push(tool_use_msg.clone());
    }
    messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: raw.to_string(),
            is_error: Some(!success),
            content_blocks: None,
        }],
    });
    messages
}

fn compact_live_tool_receipt(
    messages: Vec<Message>,
    artifacts: Vec<crate::artifacts::ArtifactRecord>,
    raw: String,
) -> Option<String> {
    let (compacted, _) =
        crate::tool_output_receipts::compact_messages_for_persistence(&messages, &artifacts);
    let content = compacted
        .last()
        .and_then(|message| message.content.first())
        .and_then(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })?;
    if content != &raw && live_tool_content_is_receipt(content) {
        Some(content.clone())
    } else {
        None
    }
}

fn live_tool_content_is_receipt(content: &str) -> bool {
    content.trim_start().starts_with("[TOOL_OUTPUT_RECEIPT]")
}

fn replace_matching_assistant_text(
    app: &mut App,
    original_text: &str,
    translated_text: String,
) -> bool {
    for message in app.api_messages.iter_mut().rev() {
        if message.role != "assistant" {
            continue;
        }
        for block in &mut message.content {
            if let ContentBlock::Text { text, .. } = block
                && text == original_text
            {
                *text = translated_text;
                return true;
            }
        }
    }
    false
}

// Streaming-thinking lifecycle helpers moved to `tui/streaming_thinking.rs`.

fn build_queued_message(app: &mut App, input: String) -> QueuedMessage {
    let skill_instruction = app.active_skill.take();
    let skill_provenance = app.active_skill_provenance.take();
    QueuedMessage::new(input, skill_instruction).with_skill_provenance(skill_provenance)
}

const INITIAL_PROMPT_DEFERRED_STATUS: &str = "Initial prompt ready; complete setup to send it";

async fn submit_initial_input_if_ready(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
) -> Result<()> {
    if !app.auto_submit_initial_input {
        return Ok(());
    }

    if app.onboarding != OnboardingState::None {
        if app.status_message.is_none() && !app.input.trim().is_empty() {
            app.status_message = Some(INITIAL_PROMPT_DEFERRED_STATUS.to_string());
        }
        return Ok(());
    }

    app.auto_submit_initial_input = false;
    if let Some(input) = app.submit_input() {
        if app.status_message.as_deref() == Some(INITIAL_PROMPT_DEFERRED_STATUS) {
            app.status_message = None;
        }
        let queued = build_queued_message(app, input);
        dispatch_user_message_with_recovery(
            app,
            config,
            engine_handle,
            queued,
            DispatchRecovery::Initial,
        )
        .await?;
    }
    Ok(())
}

fn queue_current_draft_for_next_turn(app: &mut App) -> bool {
    let Some(input) = app.submit_input() else {
        return false;
    };
    let queued = if let Some(mut draft) = app.queued_draft.take() {
        draft.display = input;
        draft
    } else {
        build_queued_message(app, input)
    };
    enqueue_offline_message(app, queued);
    let toast = format!(
        "{} queued follow-up(s) — sends after current output; ↑ edit last, /queue send <n>",
        app.queued_message_count()
    );
    app.status_message = Some(toast.clone());
    app.push_status_toast(toast, StatusToastLevel::Info, Some(3_000));
    true
}

fn take_shortcut_queued_message(app: &mut App) -> Option<(QueuedMessage, Option<usize>)> {
    if let Some(mut draft) = app.queued_draft.take() {
        if let Some(input) = app.submit_input() {
            draft.display = input;
        }
        return Some((draft, None));
    }
    if app.input.is_empty() {
        return app
            .remove_queued_message(0)
            .map(|message| (message, Some(0)));
    }
    None
}

async fn send_shortcut_queued_message_now(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
) -> Result<bool> {
    let Some((message, restore_index)) = take_shortcut_queued_message(app) else {
        return Ok(false);
    };
    send_taken_queued_message_now(app, config, engine_handle, message, restore_index).await?;
    Ok(true)
}

async fn send_queued_message_at_index_now(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    index: usize,
) -> Result<bool> {
    let Some(message) = app.remove_queued_message(index) else {
        app.status_message = Some("Queued message not found".to_string());
        return Ok(true);
    };
    send_taken_queued_message_now(app, config, engine_handle, message, Some(index)).await?;
    Ok(true)
}

async fn send_taken_queued_message_now(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
    restore_index: Option<usize>,
) -> Result<()> {
    if app.offline_mode {
        restore_queued_message(app, restore_index, message);
        app.status_message = Some(format!(
            "Offline: {} queued follow-up(s) — /queue send <n>, /queue clear",
            app.queued_message_count()
        ));
        return Ok(());
    }

    let display = message.display.clone();
    if app.dispatch_in_flight {
        // A spawned dispatch is still resolving route/sending its op (#4605):
        // there is no turn to steer into yet. Re-queue; the completion/turn
        // lifecycle will drive the next drain.
        restore_queued_message(app, restore_index, message);
        app.status_message = Some(format!(
            "{} queued follow-up(s) — sends after current dispatch starts",
            app.queued_message_count()
        ));
        return Ok(());
    }
    if app.is_loading {
        if let Err(err) = steer_user_message(app, engine_handle, message.clone()).await {
            restore_queued_message(app, restore_index, message);
            app.status_message = Some(format!(
                "Steer failed ({err}); {} queued follow-up(s) — /queue send <n>, /queue clear",
                app.queued_message_count()
            ));
        } else {
            app.push_status_toast(
                "Sent queued follow-up into current turn",
                StatusToastLevel::Info,
                Some(1_500),
            );
        }
    } else if let Err(_err) = dispatch_user_message_with_recovery(
        app,
        config,
        engine_handle,
        message,
        DispatchRecovery::Queued { restore_index },
    )
    .await
    {
        // The completion closure re-queued the message and set the status.
    } else {
        app.status_message = Some(format!("Sent queued follow-up: {display}"));
    }
    Ok(())
}

fn restore_queued_message(app: &mut App, index: Option<usize>, message: QueuedMessage) {
    if let Some(index) = index
        && index <= app.queued_messages.len()
    {
        app.queued_messages.insert(index, message);
    } else {
        app.queue_message(message);
    }
}

fn queued_message_content_for_app(
    app: &App,
    message: &QueuedMessage,
    cwd: Option<PathBuf>,
) -> Result<String> {
    if let Some(authority) = message.skill_provenance.as_ref() {
        if authority.workspace != app.workspace {
            anyhow::bail!("Queued plugin skill belongs to a different workspace and was denied");
        }
        crate::plugins::registry::verify_plugin_authority(authority).map_err(anyhow::Error::msg)?;
    }
    // Pass the process CWD explicitly so the resolver's two-pass logic can
    // honor the user's launch directory when it differs from `--workspace`
    // (issue #101 — file mentions silently routing to the wrong root).
    let user_request = crate::tui::file_mention::user_request_with_file_mentions(
        &message.display,
        &app.workspace,
        cwd,
    );
    if let Some(skill_instruction) = message.skill_instruction.as_ref() {
        Ok(format!(
            "{skill_instruction}\n\n---\n\nUser request: {user_request}"
        ))
    } else {
        Ok(user_request)
    }
}

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

fn validated_app_runtime_route(
    app: &App,
    config: &Config,
) -> Result<crate::route_runtime::ValidatedRuntimeRoute, String> {
    let (identity, scoped) = app_scoped_runtime_config(app, config);
    resolve_runtime_route_for_identity(&scoped, &identity, Some(&app.model))?.validate()
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

fn compaction_for_validated_route(
    app: &App,
    route: &crate::route_runtime::ValidatedRuntimeRoute,
) -> crate::compaction::CompactionConfig {
    app.compaction_config_for_route(
        route.identity.provider,
        &route.model,
        crate::route_budget::known_route_limits(route.candidate.limits()),
    )
}

fn validated_profile_default_route(
    config: &Config,
) -> Result<crate::route_runtime::ValidatedRuntimeRoute> {
    let provider = config.api_provider();
    let model = config.default_model();
    resolve_runtime_route(config, provider, Some(&model))
        .and_then(crate::route_runtime::ResolvedRuntimeRoute::validate)
        .map_err(anyhow::Error::msg)
}

#[derive(Debug, Clone, Copy)]
enum DispatchRecovery {
    /// Normal immediate composer submit: restore the composer on failure.
    Immediate,
    /// A queued follow-up pulled from the queue; re-insert at the prior index.
    Queued { restore_index: Option<usize> },
    /// Initial `--prompt` / startup input.
    Initial,
    /// Plan accept followup.
    PlanFollowup,
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
    next_history_revision: u64,
    api_messages_len: usize,
}

/// Data captured synchronously before the async dispatch phase. All values are
/// Send so the spawned task can resolve routes and send without holding `&mut App`.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
struct UserDispatchPrepare {
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
    show_thinking: bool,
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
struct UserDispatchOutcome {
    turn_compaction: CompactionConfig,
    effective_provider: ApiProvider,
    effective_model: String,
    effective_provider_identity: String,
    effective_provider_label: String,
    selected_reasoning_effort: Option<ReasoningEffort>,
    auto_selection: Option<crate::model_routing::AutoRouteSelection>,
}

async fn dispatch_user_message(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) -> Result<()> {
    dispatch_user_message_with_recovery(
        app,
        config,
        engine_handle,
        message,
        DispatchRecovery::Immediate,
    )
    .await
}

async fn dispatch_user_message_with_recovery(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    mut message: QueuedMessage,
    recovery: DispatchRecovery,
) -> Result<()> {
    // #1364: run mutable `message_submit` hooks before dispatch. Hooks see the
    // user's display text and may replace or block it before file mentions,
    // skill wrapping, history, and model input are resolved.
    // Fast-path skip when no hooks configured.
    if app
        .hooks
        .has_hooks_for_event(crate::hooks::HookEvent::MessageSubmit)
    {
        let context = app.base_hook_context().with_message(&message.display);
        let outcome = app
            .hooks
            .execute_message_submit_transform(&context, &message.display);
        if let Some(warning) = outcome.warning() {
            app.status_message = Some(warning.to_string());
        }
        match outcome {
            crate::hooks::MessageSubmitOutcome::Unchanged { .. } => {}
            crate::hooks::MessageSubmitOutcome::Replaced { text, .. } => {
                message.display = text;
            }
            crate::hooks::MessageSubmitOutcome::Blocked { reason } => {
                app.status_message = Some(reason);
                app.is_loading = false;
                app.dispatch_started_at = None;
                app.runtime_turn_status = None;
                return Ok(());
            }
        }
    }

    let _ = app.maybe_nudge_for_planning_prompt(&message.display);

    // Plan paused-command changes without touching App or the engine pause
    // gate. Route selection can await and client preflight can fail; neither
    // may resume or discard a paused command unless a turn is ready to send.
    let paused_dispatch = plan_paused_command_message(app, &message.display);

    let cwd = std::env::current_dir().ok();
    let references = crate::tui::file_mention::context_references_from_input(
        &message.display,
        &app.workspace,
        cwd.clone(),
    );
    let mut content = queued_message_content_for_app(app, &message, cwd)?;
    if let Some(note) = paused_dispatch.note() {
        content.push_str(note);
    }
    let (app_route_identity, route_config) = app_scoped_runtime_config(app, config);

    let should_auto_resolve = auto_router::should_resolve_auto_model_selection(app);
    let auto_router_context = auto_router::recent_auto_router_context(&app.api_messages);

    // Capture the App state before any optimistic mutation so a failure can
    // roll back cleanly.
    let snapshot = UserDispatchSnapshot {
        is_loading: app.is_loading,
        runtime_turn_status: app.runtime_turn_status.clone(),
        receipt_text: app.receipt_text.clone(),
        receipt_started_at: app.receipt_started_at,
        tool_evidence: app.tool_evidence.clone(),
        history_len: app.history.len(),
        history_revisions_len: app.history_revisions.len(),
        history_version: app.history_version,
        next_history_revision: app.next_history_revision,
        api_messages_len: app.api_messages.len(),
    };

    // --- Sync prepare: show the user message and spinner immediately so the
    // event loop can repaint before network I/O (#4605). The async phase runs
    // the auto-model route, compaction, and engine send off the render thread.
    app.is_loading = true;
    app.runtime_turn_status = None;
    app.clear_receipt();
    app.tool_evidence.clear();
    app.needs_redraw = true;

    let message_index = app.api_messages.len();
    app.add_message(HistoryCell::User {
        content: message.display.clone(),
    });
    let history_cell = app.history.len().saturating_sub(1);
    app.scroll_to_bottom();
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: content.clone(),
            cache_control: None,
        }],
    });

    let goal_objective = paused_dispatch.goal_objective(app);

    let prepare = UserDispatchPrepare {
        message,
        content,
        references,
        paused_dispatch,
        app_route_identity,
        route_config,
        goal_objective,
        goal_status: app.hunt.verdict.goal_status(),
        goal_token_budget: app.hunt.token_budget,
        mode: app.mode,
        api_provider: app.api_provider,
        app_model: app.model.clone(),
        auto_model: app.auto_model,
        reasoning_effort: app.reasoning_effort,
        allow_shell: app.allow_shell,
        trust_mode: app.trust_mode,
        auto_approve: app_auto_approve_enabled(app),
        approval_mode: app.approval_mode,
        translation_enabled: app.translation_enabled,
        show_thinking: app.show_thinking,
        allowed_tools: app.active_allowed_tools.clone(),
        hook_executor: app.runtime_services.hook_executor.clone(),
        verbosity: app.verbosity.clone(),
        provenance: UserInputProvenance::ExternalUser,
        auto_router_context,
        should_auto_resolve,
        auto_compact_user_configured: app.auto_compact_user_configured,
        auto_compact: app.auto_compact,
        auto_compact_threshold_percent: app.auto_compact_threshold_percent,
        snapshot,
        message_index,
        history_cell,
    };

    let completion_tx = match app.dispatch_completion_tx.clone() {
        Some(tx) => tx,
        None => {
            // Tests don't set the completion channel; await the spawned task
            // inline so the test observes the final App state synchronously.
            let (tx, mut rx) =
                tokio::sync::mpsc::unbounded_channel::<crate::tui::app::DispatchApplyFn>();
            tokio::spawn(spawned_dispatch_execute(
                prepare,
                recovery,
                engine_handle.clone(),
                tx,
            ));
            let apply = rx
                .recv()
                .await
                .ok_or_else(|| anyhow::Error::msg("dispatch completion channel closed"))?;
            return apply(app, engine_handle, config);
        }
    };

    // #4605: spawn the async phase so the event loop can draw immediately.
    // Mark the dispatch in flight so a submit after an Esc-cancel queues
    // instead of spawning a second dispatch that could reorder ops.
    app.dispatch_in_flight = true;
    tokio::spawn(spawned_dispatch_execute(
        prepare,
        recovery,
        engine_handle.clone(),
        completion_tx,
    ));
    Ok(())
}

async fn spawned_dispatch_execute(
    prepare: UserDispatchPrepare,
    recovery: DispatchRecovery,
    engine_handle: EngineHandle,
    completion_tx: tokio::sync::mpsc::UnboundedSender<crate::tui::app::DispatchApplyFn>,
) {
    let apply = spawned_dispatch_inner(prepare, recovery, engine_handle).await;
    let _ = completion_tx.send(apply);
}

async fn spawned_dispatch_inner(
    prepare: UserDispatchPrepare,
    recovery: DispatchRecovery,
    engine_handle: EngineHandle,
) -> crate::tui::app::DispatchApplyFn {
    let auto_selection = if prepare.should_auto_resolve {
        match crate::model_routing::resolve_auto_route_with_inventory_for_session(
            &prepare.route_config,
            &prepare.content,
            &prepare.auto_router_context,
            prepare.mode.as_setting(),
            if prepare.auto_model { "auto" } else { "fixed" },
            prepare
                .reasoning_effort
                .as_setting_for_provider(prepare.api_provider),
        )
        .await
        {
            Ok(selection) => Some(selection),
            Err(err) => {
                return build_dispatch_error_closure(prepare, recovery, err.to_string());
            }
        }
    } else {
        None
    };

    let effective_provider = auto_selection
        .as_ref()
        .map(|selection| selection.provider)
        .unwrap_or(prepare.api_provider);

    let effective_model = if prepare.auto_model {
        auto_selection
            .as_ref()
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| {
                crate::model_routing::auto_model_heuristic(
                    &prepare.message.display,
                    &prepare.app_model,
                )
            })
    } else {
        prepare.app_model.clone()
    };

    let turn_route = if effective_provider == prepare.app_route_identity.provider {
        resolve_runtime_route_for_identity(
            &prepare.route_config,
            &prepare.app_route_identity,
            Some(&effective_model),
        )
    } else {
        resolve_runtime_route(
            &prepare.route_config,
            effective_provider,
            Some(&effective_model),
        )
    };

    let turn_route = match turn_route {
        Ok(route) => route,
        Err(err) => {
            return build_dispatch_error_closure(prepare, recovery, err.to_string());
        }
    };

    let turn_route = if engine_handle.client_preflight_required() {
        match turn_route.preflight() {
            Ok(route) => route,
            Err(err) => {
                return build_dispatch_error_closure(prepare, recovery, err);
            }
        }
    } else {
        turn_route
    };

    let turn_route_limits = crate::route_budget::known_route_limits(turn_route.candidate.limits());
    let effective_provider_identity = turn_route.identity.key.clone();
    let effective_provider_label = if effective_provider == ApiProvider::Custom {
        effective_provider_identity.clone()
    } else {
        effective_provider.display_name().to_string()
    };

    let turn_compaction = CompactionConfig {
        enabled: if prepare.auto_compact_user_configured {
            prepare.auto_compact
        } else {
            crate::route_budget::auto_compact_default_for_route(
                turn_route.identity.provider,
                &turn_route.model,
                turn_route_limits,
            )
        },
        token_threshold: crate::route_budget::compaction_threshold_for_route_at_percent(
            turn_route.identity.provider,
            &turn_route.model,
            turn_route_limits,
            prepare.auto_compact_threshold_percent,
        ),
        model: turn_route.model.clone(),
        effective_context_window: Some(crate::route_budget::route_context_window_tokens(
            turn_route.identity.provider,
            &turn_route.model,
            turn_route_limits,
        )),
        ..Default::default()
    };

    let auto_controls_reasoning =
        prepare.auto_model || prepare.reasoning_effort == ReasoningEffort::Auto;
    let selected_reasoning_effort = if auto_controls_reasoning {
        let effort = auto_selection
            .as_ref()
            .and_then(|selection| selection.reasoning_effort)
            .unwrap_or_else(|| crate::auto_reasoning::select(false, &prepare.message.display));
        Some(effort)
    } else {
        None
    };

    let effective_reasoning_effort = if let Some(effort) = selected_reasoning_effort {
        effort
            .api_value_for_route(
                effective_provider,
                &turn_route.candidate.endpoint().base_url,
                &turn_route.model,
            )
            .map(str::to_string)
    } else {
        prepare
            .reasoning_effort
            .api_value_for_route(
                effective_provider,
                &turn_route.candidate.endpoint().base_url,
                &turn_route.model,
            )
            .map(str::to_string)
    };

    if let Err(err) = engine_handle
        .send(Op::SendMessage {
            content: prepare.content.clone(),
            mode: prepare.mode,
            route: Box::new(turn_route),
            compaction: Box::new(turn_compaction.clone()),
            goal_objective: prepare.goal_objective.clone(),
            goal_token_budget: prepare.goal_token_budget,
            goal_status: prepare.goal_status,
            reasoning_effort: effective_reasoning_effort,
            reasoning_effort_auto: auto_controls_reasoning,
            auto_model: prepare.auto_model,
            allow_shell: prepare.allow_shell,
            trust_mode: prepare.trust_mode,
            auto_approve: prepare.auto_approve,
            approval_mode: prepare.approval_mode,
            translation_enabled: prepare.translation_enabled,
            show_thinking: prepare.show_thinking,
            allowed_tools: prepare.allowed_tools.clone(),
            dynamic_tools: Vec::new(),
            hook_executor: prepare.hook_executor.clone(),
            verbosity: prepare.verbosity.clone(),
            provenance: prepare.provenance,
        })
        .await
    {
        return build_dispatch_error_closure(prepare, recovery, err.to_string());
    }

    build_dispatch_success_closure(
        prepare,
        UserDispatchOutcome {
            turn_compaction,
            effective_provider,
            effective_model,
            effective_provider_identity,
            effective_provider_label,
            selected_reasoning_effort,
            auto_selection,
        },
    )
}

fn build_dispatch_success_closure(
    prepare: UserDispatchPrepare,
    outcome: UserDispatchOutcome,
) -> crate::tui::app::DispatchApplyFn {
    Box::new(
        move |app: &mut App, engine_handle: &EngineHandle, config: &Config| -> anyhow::Result<()> {
            app.dispatch_in_flight = false;
            prepare.paused_dispatch.apply(app, engine_handle);

            let dispatch_started_at = Instant::now();
            app.is_loading = true;
            app.dispatch_started_at = Some(dispatch_started_at);
            app.runtime_turn_status = None;
            app.last_send_at = Some(dispatch_started_at);
            app.last_submitted_prompt = Some(prepare.message.display.clone());
            app.clear_receipt();
            app.tool_evidence.clear();

            app.system_prompt = Some(build_app_system_prompt_with_goal(
                app,
                config,
                app.hunt.quarry.as_deref(),
            ));
            // History and api_messages were already appended in the sync prepare
            // phase; record references now that the turn is accepted.
            app.record_context_references(
                prepare.history_cell,
                prepare.message_index,
                prepare.references,
            );
            app.scroll_to_bottom();

            app.last_effective_reasoning_effort = outcome.selected_reasoning_effort;
            if prepare.auto_model {
                app.last_effective_model = Some(outcome.effective_model.clone());
                app.last_effective_provider = Some(outcome.effective_provider);
                app.last_effective_provider_identity =
                    Some(outcome.effective_provider_identity.clone());
                if let Some(selection) = outcome.auto_selection.as_ref() {
                    app.last_auto_route_receipt = selection.receipt.clone();
                    let status = app
                        .tr(MessageId::AutoRouteSelectedToast)
                        .replace("{provider}", &outcome.effective_provider_label)
                        .replace("{model}", &outcome.effective_model)
                        .replace("{source}", selection.source.label());
                    app.push_status_toast(status, StatusToastLevel::Info, Some(6_000));
                }
            } else {
                app.last_effective_model = None;
                app.last_effective_provider = None;
                app.last_effective_provider_identity = None;
                app.last_auto_route_receipt = None;
            }
            app.pending_auto_route_receipt = outcome
                .auto_selection
                .as_ref()
                .and_then(|selection| selection.receipt.clone());
            app.pending_turn_route = Some((
                outcome.effective_provider,
                outcome.effective_model,
                prepare.auto_model,
            ));

            maybe_warn_context_pressure_for_config(app, &outcome.turn_compaction);
            app.session.last_prompt_tokens = None;
            app.session.last_completion_tokens = None;
            app.session.last_output_throughput = None;
            app.session.last_prompt_cache_hit_tokens = None;
            app.session.last_prompt_cache_miss_tokens = None;
            app.session.last_reasoning_replay_tokens = None;

            if let Ok(manager) = SessionManager::default_location()
                && let Ok(session) = build_session_snapshot(app, &manager)
            {
                if app.current_session_id.is_none() {
                    app.current_session_id = Some(session.metadata.id.clone());
                }
                if let Err(err) = persist_with_pending_work_boundary(
                    app,
                    PersistRequest::SaveCheckpoint { session },
                ) {
                    app.status_message = Some(format!(
                        "Work update is pending: turn checkpoint could not be queued ({err})"
                    ));
                }
            }

            Ok(())
        },
    )
}

fn build_dispatch_error_closure(
    prepare: UserDispatchPrepare,
    recovery: DispatchRecovery,
    error: String,
) -> crate::tui::app::DispatchApplyFn {
    Box::new(
        move |app: &mut App,
              _engine_handle: &EngineHandle,
              _config: &Config|
              -> anyhow::Result<()> {
            app.dispatch_in_flight = false;
            // Roll back the optimistic sync prepare mutations.
            app.is_loading = prepare.snapshot.is_loading;
            app.runtime_turn_status = prepare.snapshot.runtime_turn_status.clone();
            app.receipt_text = prepare.snapshot.receipt_text.clone();
            app.receipt_started_at = prepare.snapshot.receipt_started_at;
            app.tool_evidence = prepare.snapshot.tool_evidence.clone();
            app.history.truncate(prepare.snapshot.history_len);
            app.history_revisions
                .truncate(prepare.snapshot.history_revisions_len);
            app.history_version = prepare.snapshot.history_version;
            app.next_history_revision = prepare.snapshot.next_history_revision;
            app.api_messages.truncate(prepare.snapshot.api_messages_len);
            app.needs_redraw = true;

            match recovery {
                DispatchRecovery::Immediate => {
                    restore_failed_immediate_submit(
                        app,
                        prepare.message,
                        &anyhow::Error::msg(error.clone()),
                    );
                }
                DispatchRecovery::Queued { restore_index } => {
                    restore_queued_message(app, restore_index, prepare.message);
                    app.status_message = Some(
                        app.tr(MessageId::DispatchFailedQueued)
                            .replace("{error}", &error)
                            .replace("{count}", &app.queued_message_count().to_string()),
                    );
                }
                DispatchRecovery::Initial => {
                    app.status_message = Some(
                        app.tr(MessageId::DispatchFailedInitial)
                            .replace("{error}", &error),
                    );
                }
                DispatchRecovery::PlanFollowup => {
                    app.queue_message(prepare.message);
                    app.status_message = Some(
                        app.tr(MessageId::DispatchFailedPlanFollowup)
                            .replace("{error}", &error),
                    );
                }
            }

            Err(anyhow::Error::msg(error))
        },
    )
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

pub(crate) fn apply_goal_snapshot_to_app(app: &mut App, snapshot: &GoalSnapshot) -> bool {
    // An explicit engine-side clear is represented by the one canonical empty
    // state emitted by GoalState::snapshot. Require both fields so a malformed
    // objective-less Active/Blocked update cannot erase valid visible state.
    if snapshot.objective.is_none() && snapshot.status.trim() == "none" {
        let changed = app.hunt.quarry.is_some()
            || app.hunt.token_budget.is_some()
            || app.hunt.tokens_used != 0
            || app.hunt.time_used_seconds != 0
            || app.hunt.continuation_count != 0
            || app.hunt.started_at.is_some()
            || app.hunt.finished_at.is_some()
            || app.hunt.verdict != HuntVerdict::default();
        app.hunt = crate::tui::app::HuntState::default();
        return changed;
    }

    let Some(objective) = snapshot
        .objective
        .as_deref()
        .map(str::trim)
        .filter(|objective| !objective.is_empty())
    else {
        tracing::warn!(
            "ignoring objective-less runtime goal snapshot with non-clear status: {}",
            snapshot.status
        );
        return false;
    };
    let Some(status) = goal_status_from_snapshot(snapshot) else {
        tracing::warn!("ignoring unknown runtime goal status: {}", snapshot.status);
        return false;
    };
    let verdict = HuntVerdict::from_goal_status(status);
    let objective_changed = app.hunt.quarry.as_deref() != Some(objective);
    let changed = objective_changed
        || app.hunt.token_budget != snapshot.token_budget
        || app.hunt.tokens_used != snapshot.tokens_used
        || app.hunt.time_used_seconds != snapshot.time_used_seconds
        || app.hunt.continuation_count != snapshot.continuation_count
        || app.hunt.verdict != verdict;
    if !changed {
        return false;
    }

    app.hunt.quarry = Some(objective.to_string());
    app.hunt.token_budget = snapshot.token_budget;
    app.hunt.tokens_used = snapshot.tokens_used;
    app.hunt.time_used_seconds = snapshot.time_used_seconds;
    app.hunt.continuation_count = snapshot.continuation_count;
    app.hunt.verdict = verdict;
    if objective_changed || app.hunt.started_at.is_none() {
        app.hunt.started_at = Some(Instant::now());
    }
    // Freeze the elapsed timer the first time a goal leaves the active state.
    // Paused (Wounded) goals freeze too — usage snapshots keep arriving while
    // paused, and clearing here would silently un-freeze a timer the user just
    // paused (matching close_hunt, which records the pause instant). Only an
    // explicit resume back to Hunting re-arms the timer.
    match verdict {
        HuntVerdict::Hunted | HuntVerdict::Escaped | HuntVerdict::Wounded => {
            if app.hunt.finished_at.is_none() {
                app.hunt.finished_at = Some(Instant::now());
            }
        }
        HuntVerdict::Hunting => app.hunt.finished_at = None,
    }
    true
}

async fn sync_mode_update(app: &App, engine_handle: &EngineHandle) {
    let _ = engine_handle
        .send(Op::ChangeMode {
            mode: app.mode,
            allow_shell: app.allow_shell,
            trust_mode: app.trust_mode,
            auto_approve: app_auto_approve_enabled(app),
            approval_mode: app.approval_mode,
        })
        .await;
}

fn is_permission_cycle_shortcut(key: &KeyEvent) -> bool {
    let forbidden = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER;
    if key.modifiers.intersects(forbidden) {
        return false;
    }
    matches!(key.code, KeyCode::BackTab)
        || (matches!(key.code, KeyCode::Tab) && key.modifiers.contains(KeyModifiers::SHIFT))
}

async fn cycle_permission_posture(
    app: &mut App,
    config: &mut Config,
    engine_handle: &EngineHandle,
) {
    let control = config.approval_policy_control(
        app.config_path.as_deref(),
        app.config_profile.as_deref(),
        &app.workspace,
    );
    let changed = if control == crate::config::ApprovalPolicyControl::RootConfig {
        app.cycle_root_approval_posture()
    } else {
        app.cycle_approval_posture()
    };
    if changed {
        if control == crate::config::ApprovalPolicyControl::RootConfig {
            config.approval_policy = None;
        }
        sync_mode_update(app, engine_handle).await;
        refresh_config_view_if_open(app, "permission_posture");
    }
}

async fn apply_mode_update(app: &mut App, engine_handle: &EngineHandle, mode: AppMode) -> bool {
    if app.set_mode(mode) {
        sync_mode_update(app, engine_handle).await;
        true
    } else {
        false
    }
}

async fn handle_bang_shell_input(
    app: &mut App,
    engine_handle: &EngineHandle,
    input: &str,
) -> Result<bool> {
    let command = match shell_command_from_bang_input(input) {
        Ok(Some(command)) => command,
        Ok(None) => return Ok(false),
        Err(message) => {
            app.status_message = Some(format!("Error: {message}"));
            return Ok(true);
        }
    };

    engine_handle
        .send(Op::RunShellCommand {
            command: command.to_string(),
            mode: app.mode,
            allow_shell: app.allow_shell,
            trust_mode: app.trust_mode,
            auto_approve: app_auto_approve_enabled(app),
            approval_mode: app.approval_mode,
        })
        .await?;
    app.status_message = Some(format!("Shell command submitted: {command}"));
    Ok(true)
}

fn is_model_visible_tool_call(id: &str) -> bool {
    !id.starts_with(USER_SHELL_TOOL_ID_PREFIX)
}

async fn apply_model_and_compaction_update(
    engine_handle: &EngineHandle,
    compaction: crate::compaction::CompactionConfig,
    mode: AppMode,
    route_limits: Option<codewhale_config::route::RouteLimits>,
) {
    let _ = engine_handle
        .send(Op::SetModel {
            model: compaction.model.clone(),
            mode,
            route_limits,
        })
        .await;
    let _ = engine_handle
        .send(Op::SetCompaction { config: compaction })
        .await;
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

/// Apply the choice made in the `/model` picker (#39): mutate App state so
/// the next turn uses the new model/effort, persist the selection to
/// `~/.codewhale/settings.toml` (legacy: `~/.deepseek/settings.toml`) so it survives a restart, push the change to
/// the running engine via `Op::SetModel`/`Op::SetCompaction`, and surface
/// a one-line status describing what changed.
// The model/effort transition needs both the previous and next model+effort
// plus the engine, app, and config handles; bundling them into a struct here
// would only obscure a straightforward orchestration step.
#[allow(clippy::too_many_arguments)]
async fn apply_model_picker_choice(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    model: String,
    target_provider: Option<ApiProvider>,
    target_provider_id: Option<String>,
    mut effort: crate::tui::app::ReasoningEffort,
    previous_model: String,
    previous_effort: crate::tui::app::ReasoningEffort,
) {
    let target_provider = target_provider.unwrap_or(app.api_provider);
    let target_identity = if target_provider == ApiProvider::Custom {
        target_provider_id.unwrap_or_else(|| config.provider_identity_for(target_provider))
    } else {
        target_provider.as_str().to_string()
    };
    let model_is_auto = model.trim().eq_ignore_ascii_case("auto");
    if model_is_auto {
        effort = ReasoningEffort::Auto;
    }
    if target_provider != app.api_provider
        || target_identity != app.provider_identity_for_persistence()
    {
        config.provider = Some(target_identity.clone());
        switch_provider(
            app,
            engine_handle,
            config,
            target_provider,
            (!model_is_auto).then_some(model.clone()),
        )
        .await;
        if app.api_provider != target_provider
            || app.provider_identity_for_persistence() != target_identity
        {
            return;
        }
        if !model_is_auto {
            effort = effort.normalize_for_route(
                app.api_provider,
                &app.active_route_base_url,
                &app.model,
            );
            apply_picker_effort_choice(app, engine_handle, effort, previous_effort).await;
            return;
        }
    }

    let model_changed = model != previous_model || app.auto_model != model_is_auto;
    let mut resolved_model = model.clone();
    let mut route_base_url = config.deepseek_base_url();
    if !model_is_auto {
        let saved_provider_model = config
            .provider_config_for(app.api_provider)
            .and_then(|provider| provider.model.as_deref());
        match crate::route_runtime::resolve_route_candidate_with_context_metadata(
            app.api_provider,
            Some(&model),
            saved_provider_model,
            Some(config.deepseek_base_url()),
            config.context_window_for_provider_config(app.api_provider),
            None,
        ) {
            Ok(resolution) => {
                resolved_model = resolution.candidate.wire_model_id().as_str().to_string();
                route_base_url = resolution.candidate.endpoint().base_url.clone();
                if model_changed {
                    app.set_active_context_window_override(
                        config.context_window_for_provider_config(app.api_provider),
                    );
                    app.set_active_route_resolution(
                        route_base_url.clone(),
                        resolution.candidate.limits(),
                        resolution.context_window.source,
                    );
                }
            }
            Err(reason) => {
                app.status_message = Some(reason);
                return;
            }
        }
    } else if model_changed {
        app.set_active_context_window_override(
            config.context_window_for_provider_config(app.api_provider),
        );
        app.active_route_limits = app.context_window_override_limits();
        app.active_route_base_url = route_base_url.clone();
        app.active_context_window_source = if app.active_context_window_override.is_some() {
            crate::route_runtime::ContextWindowSource::Configured
        } else {
            crate::route_runtime::ContextWindowSource::Fallback
        };
    }

    if !model_is_auto {
        effort = effort.normalize_for_route(app.api_provider, &route_base_url, &resolved_model);
    }
    let effort_changed = effort != previous_effort;
    if !model_changed && !effort_changed {
        app.status_message = Some(format!(
            "Model unchanged: {model} · thinking {}",
            effort.display_label_for_provider(app.api_provider)
        ));
        return;
    }

    if model_changed {
        app.set_model_selection(resolved_model.clone());
        app.provider_models.insert(
            app.provider_identity_for_persistence().to_string(),
            resolved_model.clone(),
        );
        app.clear_model_scoped_telemetry();
    }
    if effort_changed {
        app.reasoning_effort = effort;
        app.reasoning_effort_explicit = true;
        app.last_effective_reasoning_effort = None;
    }
    if model_changed || effort_changed {
        app.update_model_compaction_budget();
    }

    // Best-effort persist; surface a status warning if the settings file
    // can't be written rather than aborting the in-memory change.
    let mut persist_warning: Option<String> = None;
    let persist_result = (|| -> anyhow::Result<()> {
        let mut settings = crate::settings::Settings::load_persisted()?;
        if model_changed {
            if matches!(
                app.api_provider,
                ApiProvider::Deepseek | ApiProvider::DeepseekCN
            ) {
                settings.set("default_model", &resolved_model)?;
            }
            settings
                .set_model_for_provider(app.provider_identity_for_persistence(), &resolved_model);
        }
        if effort_changed {
            settings.set(
                "reasoning_effort",
                effort.as_setting_for_route(app.api_provider, &route_base_url, &resolved_model),
            )?;
        }
        settings.save()
    })();
    if let Err(err) = persist_result {
        persist_warning = Some(format!("(not persisted: {err})"));
    }

    if model_changed {
        apply_model_and_compaction_update(
            engine_handle,
            app.compaction_config(),
            app.mode,
            app.active_route_limits,
        )
        .await;
    }

    let model_summary = if model_is_auto {
        "auto (per-turn model)".to_string()
    } else {
        resolved_model.clone()
    };
    let previous_effort_summary = previous_effort.display_label_for_provider(app.api_provider);
    let effort_summary = if effort == ReasoningEffort::Auto {
        "auto (per-turn thinking)".to_string()
    } else {
        effort
            .display_label_for_provider(app.api_provider)
            .to_string()
    };

    let mut summary = match (model_changed, effort_changed) {
        (true, true) => format!(
            "Model: {previous_model} → {model_summary} · thinking: {previous_effort_summary} → {effort_summary}"
        ),
        (true, false) => {
            format!("Model: {previous_model} → {model_summary} · thinking {effort_summary}")
        }
        (false, true) => format!(
            "Thinking: {previous_effort_summary} → {effort_summary} · model {model_summary}"
        ),
        (false, false) => unreachable!(),
    };
    let persisted = persist_warning.is_none();
    if let Some(warning) = persist_warning {
        summary.push(' ');
        summary.push_str(&warning);
    }
    app.status_message = Some(summary);
    if model_changed && persisted {
        record_provider_model_setup_progress(app, config);
    }
}

async fn apply_picker_effort_choice(
    app: &mut App,
    engine_handle: &EngineHandle,
    mut effort: ReasoningEffort,
    previous_effort: ReasoningEffort,
) {
    effort = effort.normalize_for_route(app.api_provider, &app.active_route_base_url, &app.model);
    if effort == previous_effort {
        return;
    }

    app.reasoning_effort = effort;
    app.reasoning_effort_explicit = true;
    app.last_effective_reasoning_effort = None;
    app.update_model_compaction_budget();

    let persist_warning = (|| -> anyhow::Result<()> {
        let mut settings = crate::settings::Settings::load_persisted()?;
        settings.set(
            "reasoning_effort",
            effort.as_setting_for_route(app.api_provider, &app.active_route_base_url, &app.model),
        )?;
        settings.save()
    })()
    .err()
    .map(|err| format!(" (not persisted: {err})"));

    apply_model_and_compaction_update(
        engine_handle,
        app.compaction_config(),
        app.mode,
        app.active_route_limits,
    )
    .await;

    let mut summary = format!(
        "Thinking: {} → {} · model {}",
        previous_effort.display_label_for_provider(app.api_provider),
        effort.display_label_for_provider(app.api_provider),
        app.model_display_label()
    );
    if let Some(warning) = persist_warning {
        summary.push_str(&warning);
    }
    app.status_message = Some(summary);
}

/// Apply a `/provider` switch by resolving a complete route candidate before
/// mutating state, then respawning the engine so the API client picks up the
/// new base URL/key. When `model_override` is set, it replaces the active
/// model post-switch after provider-scoped normalization.
async fn switch_provider(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    target: ApiProvider,
    model_override: Option<String>,
) -> bool {
    let previous_provider = app.api_provider;
    let previous_identity = app.provider_identity_for_persistence().to_string();
    let requested_identity = config.provider_identity_for(target);
    let previous_model = app.model.clone();
    let previous_model_ids_passthrough = app.model_ids_passthrough;
    let mut previous_config = config.clone();
    previous_config.provider = Some(previous_identity.clone());
    app.pending_provider_switch = Some(PendingProviderSwitch {
        previous_provider,
        previous_model: previous_model.clone(),
        previous_model_ids_passthrough,
        previous_route_limits: app.active_route_limits,
        previous_route_base_url: app.active_route_base_url.clone(),
        previous_context_window_source: app.active_context_window_source,
        previous_context_window_override: app.active_context_window_override,
        previous_config: previous_config.clone(),
        previous_onboarding: app.onboarding,
        previous_onboarding_needs_api_key: app.onboarding_needs_api_key,
        previous_api_key_env_only: app.api_key_env_only,
    });

    let resolved_route = match resolve_runtime_route(config, target, model_override.as_deref()) {
        Ok(route) => route,
        Err(reason) => {
            app.pending_provider_switch = None;
            // #3830: if the switch failed only because the target provider has
            // no key or local runtime, hand off to /provider already focused
            // on that provider's key prompt instead of dead-ending with an
            // error the user has to translate into an action.
            if !crate::config::has_api_key_for(config, target)
                && app.view_stack.top_kind() != Some(ModalKind::ProviderPicker)
            {
                let runtime_status = query_provider_runtime_status(engine_handle).await;
                if let Some(picker) =
                    crate::tui::provider_picker::ProviderPickerView::new_for_missing_auth(
                        previous_provider,
                        target,
                        config,
                        runtime_status,
                    )
                    .map(|picker| {
                        picker
                            .with_locale(app.ui_locale)
                            .with_provider_health(&app.provider_health)
                    })
                {
                    *config = previous_config;
                    app.view_stack.push(picker);
                    app.status_message = Some(format!(
                        "{} needs a key or local runtime — enter one to switch.",
                        target.display_name()
                    ));
                    app.needs_redraw = true;
                    return false;
                }
            }
            *config = previous_config;
            app.add_message(HistoryCell::System {
                content: format!(
                    "Cannot switch to {}: {reason}\nProvider unchanged ({}).",
                    requested_identity, previous_identity
                ),
            });
            app.status_message = Some(format!(
                "Route rejected before provider switch: {}.",
                target.as_str()
            ));
            return false;
        }
    };
    let validated_route = match resolved_route.validate() {
        Ok(route) => route,
        Err(err) => {
            app.pending_provider_switch = None;
            *config = previous_config;
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to switch provider to {}: {err}\nProvider unchanged ({}).",
                    requested_identity, previous_identity
                ),
            });
            return false;
        }
    };
    let target_identity_record = validated_route.identity.clone();
    let target_identity = target_identity_record.key.clone();
    let resolved_endpoint = validated_route.candidate.endpoint().base_url.clone();
    let route_limits = validated_route.candidate.limits();
    let context_window_source = validated_route.context_window.source;
    let new_model = validated_route.model.clone();
    *config = *validated_route.config;

    let new_base_url = resolved_endpoint;
    let new_endpoint = display_base_url_host(&new_base_url);
    let cache_scope_changed = previous_provider != target
        || previous_identity != target_identity
        || previous_model != new_model;
    app.set_provider_identity_record(target_identity_record);
    app.billing_presentation = crate::route_billing::for_route(config, target);
    app.max_subagents = config
        .max_subagents_for_provider(target)
        .clamp(1, crate::config::MAX_SUBAGENTS);
    app.provider_chain = target
        .kind()
        .map(|kind| codewhale_config::ProviderChain::new(kind, &config.fallback_providers))
        .filter(|chain| chain.providers().len() > 1);
    app.last_fallback_reason = None;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.apply_provider_switch_reasoning_effort(target, &new_base_url, model_override.as_deref());
    app.set_model_selection(new_model.clone());
    app.set_active_context_window_override(config.context_window_for_provider_config(target));
    app.set_active_route_resolution(new_base_url.clone(), route_limits, context_window_source);
    if model_override.is_some() {
        app.provider_models
            .insert(target_identity.clone(), new_model.clone());
    }
    app.update_model_compaction_budget();
    if cache_scope_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
        app.session.last_output_throughput = None;
    }

    let _ = engine_handle.send(Op::Shutdown).await;
    let engine_config = build_engine_config(app, config);
    *engine_handle = spawn_tui_engine(engine_config, config);
    // A successful in-session switch must refresh the same key-scoped live
    // catalog as startup. TelecomJS is currently the only provider using this
    // seam; failures preserve the existing/static rows.
    crate::client::DeepSeekClient::spawn_active_provider_catalog_refresh(config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
                mode: app.mode,
            })
            .await;
    }
    let _ = engine_handle
        .send(Op::SetCompaction {
            config: app.compaction_config(),
        })
        .await;

    let persist_warning = (|| -> anyhow::Result<()> {
        let provider_key = config.provider_identity_for(target);
        crate::config_persistence::persist_root_string_key(
            app.config_path.as_deref(),
            "provider",
            &provider_key,
        )?;

        let mut settings = crate::settings::Settings::load_persisted()?;
        settings.default_provider = Some(provider_key.clone());
        if model_override.is_some() {
            settings.set_model_for_provider(&provider_key, &new_model);
            if matches!(target, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
                settings.set("default_model", &new_model)?;
            }
        }
        settings.save()?;
        Ok(())
    })()
    .err()
    .map(|err| format!("Provider selection was not fully persisted: {err}"));

    let mut switch_summary = format!(
        "Provider switched: {} → {}",
        previous_identity, target_identity,
    );
    switch_summary.push(char::from(10));
    switch_summary.push_str(&format!("Model: {previous_model} → {new_model}"));
    switch_summary.push(char::from(10));
    switch_summary.push_str(&format!("Endpoint: {new_endpoint}"));
    if let Some(ref warning) = persist_warning {
        switch_summary.push(char::from(10));
        switch_summary.push_str(warning);
    }
    app.add_message(HistoryCell::System {
        content: switch_summary,
    });

    let mut status_message = format!("Provider: {target_identity} via {new_endpoint}");
    let persisted = persist_warning.is_none();
    if persist_warning.is_some() {
        status_message.push_str(" (not fully persisted)");
    }
    app.status_message = Some(status_message);
    if persisted {
        record_provider_model_setup_progress(app, config);
    }
    true
}

struct ProviderFallbackRollback {
    identity: ProviderIdentity,
    chain: Option<codewhale_config::ProviderChain>,
}

async fn apply_provider_fallback_switch(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    rollback: ProviderFallbackRollback,
) {
    let ProviderFallbackRollback {
        identity: previous_identity,
        chain: previous_chain,
    } = rollback;
    let previous_provider = previous_identity.provider;
    let target = app.api_provider;
    let previous_model = app.model.clone();

    let resolved_route = match resolve_runtime_route(config, target, None) {
        Ok(route) => route,
        Err(reason) => {
            app.set_provider_identity_record(previous_identity.clone());
            app.provider_chain = previous_chain.clone();
            app.last_fallback_reason = Some(format!(
                "Fallback provider {} route was rejected: {reason}",
                target.as_str()
            ));
            app.status_message = Some(format!(
                "Fallback provider {} rejected; provider remains {}.",
                target.as_str(),
                previous_provider.as_str()
            ));
            return;
        }
    };
    let target_identity = resolved_route.identity.clone();
    let resolved_endpoint = resolved_route.candidate.endpoint().base_url.clone();
    let next_config = resolved_route.config;
    let new_model = resolved_route.model;
    let context_window_source = resolved_route.context_window.source;

    if let Err(err) = DeepSeekClient::from_candidate(&next_config, &resolved_route.candidate) {
        app.set_provider_identity_record(previous_identity);
        app.provider_chain = previous_chain;
        app.last_fallback_reason = Some(format!(
            "Fallback provider {} was unavailable: {err}",
            target.as_str()
        ));
        app.status_message = Some(format!(
            "Fallback provider {} unavailable; provider remains {}.",
            target.as_str(),
            previous_provider.as_str()
        ));
        return;
    }
    *config = *next_config;
    app.set_provider_identity_record(target_identity);
    app.billing_presentation = crate::route_billing::for_route(config, target);

    let new_base_url = resolved_endpoint;
    let new_endpoint = display_base_url_host(&new_base_url);
    let cache_scope_changed = previous_provider != target || previous_model != new_model;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.reasoning_effort =
        app.reasoning_effort
            .normalize_for_route(target, &new_base_url, &new_model);
    app.set_model_selection(new_model.clone());
    app.set_active_context_window_override(config.context_window_for_provider_config(target));
    app.set_active_route_resolution(
        new_base_url.clone(),
        resolved_route.candidate.limits(),
        context_window_source,
    );
    app.update_model_compaction_budget();
    if cache_scope_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
        app.session.last_output_throughput = None;
    }

    let _ = engine_handle.send(Op::Shutdown).await;
    let engine_config = build_engine_config(app, config);
    *engine_handle = spawn_tui_engine(engine_config, config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
                mode: app.mode,
            })
            .await;
    }
    let _ = engine_handle
        .send(Op::SetCompaction {
            config: app.compaction_config(),
        })
        .await;

    app.add_message(HistoryCell::System {
        content: format!(
            "Provider fallback: {} -> {}\nModel: {} -> {}\nEndpoint: {}",
            previous_provider.as_str(),
            target.as_str(),
            previous_model,
            new_model,
            new_endpoint
        ),
    });
    app.status_message = Some(format!(
        "Fallback provider: {} via {}",
        target.as_str(),
        new_endpoint
    ));
}

fn display_base_url_host(base_url: &str) -> String {
    let without_scheme = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    without_scheme
        .split('/')
        .next()
        .filter(|host| !host.is_empty())
        .unwrap_or(base_url)
        .to_string()
}

fn sync_config_provider_from_app(config: &mut Config, app: &App) {
    config.provider = Some(app.provider_identity_for_persistence().to_string());
}

fn provider_picker_model_override(
    app: &App,
    config: &Config,
    provider: ApiProvider,
) -> Option<String> {
    (app.api_provider == provider
        && app.provider_identity_for_persistence() == config.provider_identity_for(provider))
    .then(|| app.model.clone())
}

async fn query_provider_runtime_status(
    engine_handle: &EngineHandle,
) -> Option<ProviderRuntimeStatus> {
    tokio::time::timeout(
        Duration::from_millis(100),
        engine_handle.get_provider_runtime_status(),
    )
    .await
    .ok()
    .and_then(|result| result.ok())
}

/// Open the one canonical provider setup surface for onboarding.  Fresh
/// onboarding starts at the full catalog; missing-key recovery focuses the
/// current route so an exact Kimi Code K3 configuration can expose its plan
/// route before a secret is entered.
async fn open_onboarding_provider_picker(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    focus_current_route: bool,
) {
    if app.onboarding != OnboardingState::Provider
        || app.view_stack.top_kind() == Some(ModalKind::ProviderPicker)
    {
        return;
    }
    let runtime_status = query_provider_runtime_status(engine_handle).await;
    app.view_stack.push(
        crate::tui::provider_picker::ProviderPickerView::new_for_setup(
            app.api_provider,
            focus_current_route.then_some(app.onboarding_provider),
            config,
            runtime_status,
        )
        .with_locale(app.ui_locale)
        .with_provider_health(&app.provider_health),
    );
    app.needs_redraw = true;
}

fn open_text_pager(app: &mut App, title: String, content: String) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    app.view_stack.push(PagerView::from_text(
        title,
        &content,
        width.saturating_sub(2),
    ));
}

fn launch_worktree_slug(requested: &str) -> String {
    let requested = requested.trim();
    if requested.is_empty() {
        return format!("session-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    }
    let mut slug = String::new();
    let mut separator = false;
    for ch in requested.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            separator = false;
        } else if matches!(ch, '-' | '_' | ' ' | '/' | '.') && !slug.is_empty() && !separator {
            slug.push('-');
            separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        format!("session-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
    } else {
        slug
    }
}

fn launch_worktree_spec(
    workspace: &std::path::Path,
    requested: &str,
) -> Result<codewhale_lane::WorktreeProvision> {
    let output = std::process::Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("inspect Git repository for new worktree")?;
    if !output.status.success() {
        anyhow::bail!("new worktree requires a Git repository");
    }
    let repo_root = PathBuf::from(String::from_utf8(output.stdout)?.trim());
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("workspace");
    let slug = launch_worktree_slug(requested);
    let parent = repo_root.parent().unwrap_or(repo_root.as_path());
    let path = parent
        .join(".codewhale-worktrees")
        .join(format!("{repo_name}-{slug}"));
    if path.exists() {
        anyhow::bail!("worktree path already exists: {}", path.display());
    }
    Ok(codewhale_lane::WorktreeProvision {
        repo_root,
        branch: format!("codex/{slug}"),
        path,
        base_ref: Some("HEAD".to_string()),
    })
}

async fn provision_launch_worktree(workspace: PathBuf, requested: String) -> Result<PathBuf> {
    let spec = launch_worktree_spec(&workspace, &requested)?;
    let provisioned =
        tokio::task::spawn_blocking(move || codewhale_lane::provision_worktree(&spec))
            .await
            .context("new worktree task failed")??;
    Ok(provisioned.path)
}

fn begin_launch_session(app: &mut App, workspace: Option<PathBuf>) -> commands::CommandResult {
    if let Some(workspace) = workspace {
        app.workspace = workspace;
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    app.current_session_id = Some(session_id.clone());
    app.current_session_metadata = None;
    app.session_title = Some(app.tr(MessageId::SessionsNewSessionTitle).into_owned());
    app.launch.visible = false;
    app.launch.status = None;
    app.status_message = None;
    commands::CommandResult::action(AppAction::SyncSession {
        session_id: Some(session_id),
        messages: Vec::new(),
        system_prompt: None,
        model: app.model.clone(),
        workspace: app.workspace.clone(),
        mode: app.mode,
    })
}

pub(crate) fn open_context_inspector(app: &mut App) {
    app.view_stack.push(ContextInspectorView::new(app));
}

// File-picker relevance scoring moved to `tui/file_picker_relevance.rs`.

async fn apply_command_result(
    terminal: &mut AppTerminal,
    app: &mut App,
    engine_handle: &mut EngineHandle,
    task_manager: &SharedTaskManager,
    config: &mut Config,
    #[cfg_attr(not(feature = "web"), allow(unused_variables))] web_config_session: &mut Option<
        WebConfigSession,
    >,
    result: commands::CommandResult,
) -> Result<bool> {
    if let Some(msg) = result.message {
        app.add_message(HistoryCell::System { content: msg });
    }

    if let Some(action) = result.action {
        match action {
            AppAction::Quit => {
                let _ = engine_handle.send(Op::Shutdown).await;
                return Ok(true);
            }
            AppAction::SaveSession(path) => {
                app.status_message = Some(format!("Session saved to {}", path.display()));
            }
            AppAction::LoadSession(path) => {
                let session: SavedSession = match std::fs::read_to_string(&path)
                    .map_err(|err| err.to_string())
                    .and_then(|raw| serde_json::from_str(&raw).map_err(|err| err.to_string()))
                {
                    Ok(session) => session,
                    Err(err) => {
                        app.status_message = Some(format!(
                            "Failed to load session from {}: {err}",
                            path.display()
                        ));
                        return Ok(false);
                    }
                };
                let fresh_config =
                    match Config::load(app.config_path.clone(), app.config_profile.as_deref()) {
                        Ok(config) => config,
                        Err(err) => {
                            app.status_message = Some(format!(
                                "Failed to load live config for session restore: {err}"
                            ));
                            return Ok(false);
                        }
                    };
                let (recovered, respawn) = match apply_loaded_session_config_snapshot(
                    app,
                    config,
                    &session,
                    fresh_config,
                    true,
                ) {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        app.status_message = Some(format!("Failed to restore session: {err}"));
                        return Ok(false);
                    }
                };
                sync_runtime_workspace_state(task_manager, app.workspace.clone()).await;
                if respawn {
                    let _ = engine_handle.send(Op::Shutdown).await;
                    *engine_handle = spawn_tui_engine(build_engine_config(app, config), config);
                } else {
                    let _ = engine_handle
                        .send(Op::SetModel {
                            model: app.model.clone(),
                            mode: app.mode,
                            route_limits: app.active_route_limits,
                        })
                        .await;
                }
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id: app.current_session_id.clone(),
                        messages: app.api_messages.clone(),
                        system_prompt: app.system_prompt.clone(),
                        system_prompt_override: false,
                        model: app.model.clone(),
                        workspace: app.workspace.clone(),
                        mode: app.mode,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SetCompaction {
                        config: app.compaction_config(),
                    })
                    .await;
                let success_message = format!(
                    "Session loaded from {} (ID: {}, {} messages)",
                    path.display(),
                    crate::session_manager::truncate_id(&session.metadata.id),
                    session.metadata.message_count
                );
                app.add_message(HistoryCell::System {
                    content: success_message.clone(),
                });
                if !recovered {
                    app.status_message = Some(success_message);
                }
            }
            AppAction::SyncSession {
                session_id,
                messages,
                system_prompt,
                model,
                workspace,
                mode,
            } => {
                let mut session_id = session_id;
                let is_full_reset = messages.is_empty() && system_prompt.is_none();
                if is_full_reset && session_id.is_none() {
                    let new_session_id = uuid::Uuid::new_v4().to_string();
                    app.current_session_id = Some(new_session_id.clone());
                    session_id = Some(new_session_id);
                }
                let workspace_changed = task_manager.default_workspace().await != workspace;
                if workspace_changed {
                    apply_workspace_runtime_state(app, config, workspace.clone());
                    sync_runtime_workspace_state(task_manager, workspace.clone()).await;
                }
                let provider_changed = config.api_provider() != app.api_provider
                    || config.provider_identity_for(config.api_provider())
                        != app.provider_identity_for_persistence();
                if provider_changed {
                    let identity = match config
                        .resolve_provider_identity(app.provider_identity_for_persistence())
                    {
                        Ok(identity) => identity,
                        Err(err) => {
                            app.status_message =
                                Some(format!("Failed to restore saved session provider: {err}"));
                            return Ok(false);
                        }
                    };
                    restore_loaded_session_provider(app, config, identity);
                    config.set_provider_model_override(app.api_provider, Some(model.clone()));
                }
                // Re-resolve from the live config even when the provider did
                // not change. The command layer intentionally has no Config
                // handle, so its provisional limits cannot include current
                // provider overrides.
                resolve_loaded_session_route(app, config);
                app.update_model_compaction_budget();
                if provider_changed || workspace_changed {
                    let _ = engine_handle.send(Op::Shutdown).await;
                    *engine_handle = spawn_tui_engine(build_engine_config(app, config), config);
                }
                // SyncSession carries the conversation but not resolved route
                // limits. Refresh the engine's model first so a loaded,
                // forked, or freshly reset session cannot retain the previous
                // route's context/output facts.
                let _ = engine_handle
                    .send(Op::SetModel {
                        model: model.clone(),
                        mode,
                        route_limits: app.active_route_limits,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id,
                        messages,
                        system_prompt,
                        system_prompt_override: false,
                        model,
                        workspace,
                        mode,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SetCompaction {
                        config: app.compaction_config(),
                    })
                    .await;
                if is_full_reset {
                    persist_full_reset_snapshot(app);
                }
            }
            AppAction::ModeChanged(_mode) => {
                sync_mode_update(app, engine_handle).await;
            }
            AppAction::ApprovalPolicyPersisted { policy } => {
                config.approval_policy = policy;
                sync_mode_update(app, engine_handle).await;
            }
            AppAction::PluginRegistryChanged => {
                let _ = engine_handle.send(Op::Shutdown).await;
                *engine_handle = spawn_tui_engine(build_engine_config(app, config), config);
                if !app.api_messages.is_empty() {
                    let _ = engine_handle
                        .send(Op::SyncSession {
                            session_id: app.current_session_id.clone(),
                            messages: app.api_messages.clone(),
                            system_prompt: app.system_prompt.clone(),
                            system_prompt_override: false,
                            model: app.model.clone(),
                            workspace: app.workspace.clone(),
                            mode: app.mode,
                        })
                        .await;
                }
            }
            AppAction::SendMessage(content) => {
                let queued = build_queued_message(app, content);
                submit_or_steer_message(app, config, engine_handle, queued).await?;
            }
            AppAction::SetGoalStatus { status, clear } => {
                let _ = engine_handle
                    .send(Op::SetGoalStatus { status, clear })
                    .await;
            }
            AppAction::VoiceCapture => {
                use commands::voice::VoiceCaptureOutcome;
                match commands::voice::capture_and_transcribe(app, config).await {
                    Ok(VoiceCaptureOutcome::Insert(text)) => {
                        app.insert_str(&text);
                        app.status_message = Some(format!(
                            "{}: {text}",
                            tr(app.ui_locale, MessageId::VoiceTranscribed)
                        ));
                    }
                    Ok(VoiceCaptureOutcome::Send(content)) => {
                        app.status_message =
                            Some(tr(app.ui_locale, MessageId::VoiceTranscribed).to_string());
                        let queued = build_queued_message(app, content);
                        submit_or_steer_message(app, config, engine_handle, queued).await?;
                    }
                    Err(err) => {
                        app.voice_enabled = false;
                        app.status_message = Some(err);
                    }
                }
            }
            AppAction::ListSubAgents => {
                // #3802: non-blocking send — refresh op, safe to drop.
                let _ = engine_handle.try_send(Op::ListSubAgents);
            }
            AppAction::CancelSubAgent { agent_id } => {
                app.status_message = Some(format!("Cancelling {agent_id}..."));
                if engine_handle
                    .send(Op::CancelSubAgent {
                        agent_id: agent_id.clone(),
                    })
                    .await
                    .is_err()
                {
                    app.status_message = Some(format!("Could not cancel {agent_id}"));
                }
            }
            AppAction::FetchModels => {
                app.status_message = Some("Fetching models...".to_string());
                match fetch_available_models(config).await {
                    Ok(models) => {
                        app.add_message(HistoryCell::System {
                            content: format_helpers::available_models_message(&app.model, &models),
                        });
                        app.status_message = Some(format!("Found {} model(s)", models.len()));
                    }
                    Err(error) => {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Failed to fetch models from {}: {error}",
                                config.api_provider().display_name()
                            ),
                        });
                    }
                }
            }
            AppAction::RefreshModelsDevCatalog => {
                app.status_message = Some("Refreshing Models.dev catalog...".to_string());
                let message = match crate::models_dev_live::refresh(true).await {
                    Ok(count) => {
                        let status = crate::models_dev_live::status();
                        let source = if status.source_label.is_empty() {
                            "unknown"
                        } else {
                            status.source_label.as_str()
                        };
                        format!(
                            "Models.dev catalog refreshed: {count} offerings ({:?}, source {source})",
                            status.freshness
                        )
                    }
                    Err(err) => {
                        let status = crate::models_dev_live::status();
                        format!(
                            "Models.dev refresh failed ({err}); keeping prior/bundled rows ({} offerings, {:?})",
                            status.offering_count, status.freshness
                        )
                    }
                };
                app.add_message(HistoryCell::System {
                    content: message.clone(),
                });
                app.status_message = Some(message);
            }
            AppAction::CacheWarmup => {
                app.status_message = Some("Warming prompt cache...".to_string());
                match run_cache_warmup(app, config).await {
                    Ok((usage, base_url, inspection)) => {
                        app.session.last_base_url = Some(base_url.clone());
                        app.session.last_warmup_key = Some(CacheWarmupKey::from_inspection(
                            &format!("{:?}", app.api_provider),
                            &app.model,
                            &base_url,
                            &inspection,
                        ));
                        let mut message = format_helpers::cache_warmup_result(&usage);
                        if let Some(key) = app.session.last_warmup_key.as_ref() {
                            message.push_str(&format!("\nWarmup key: {}", key.hash_short()));
                        }
                        // Append prefix-cache stability info.
                        if app.prefix_checks_total > 0 {
                            let changes = app.prefix_change_count;
                            let total = app.prefix_checks_total;
                            let stable = total.saturating_sub(changes);
                            let pct = app
                                .prefix_stability_pct
                                .map(|p| format!("{p}%"))
                                .unwrap_or_else(|| "--".to_string());
                            message.push_str(&format!(
                                "\n\nPrefix stability: {pct} ({stable}/{total} checks stable, {changes} change{})",
                                if changes == 1 { "" } else { "s" }
                            ));
                            if let Some(ref desc) = app.last_prefix_change_desc {
                                message.push_str(&format!("\nLast prefix change: {desc}"));
                            }
                        }
                        app.add_message(HistoryCell::System { content: message });
                        app.status_message = Some("Cache warmup complete".to_string());
                    }
                    Err(error) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Cache warmup failed: {error}"),
                        });
                        app.status_message = Some("Cache warmup failed".to_string());
                    }
                }
            }
            AppAction::SwitchProvider { provider, model } => {
                switch_provider(app, engine_handle, config, provider, model).await;
                // Refresh balance after provider switch.
                let balance_cooldown_expired = app
                    .last_balance_fetch
                    .is_none_or(|t| t.elapsed() >= BALANCE_FETCH_COOLDOWN);
                if balance_cooldown_expired && should_fetch_deepseek_balance(app) {
                    let cell = app.balance_cell.clone();
                    let api_key = config.deepseek_api_key().unwrap_or_default();
                    let base_url = config.deepseek_base_url();
                    if !api_key.is_empty() {
                        app.last_balance_fetch = Some(Instant::now());
                        tokio::spawn(async move {
                            if let Some(info) = fetch_deepseek_balance(&api_key, &base_url).await
                                && let Ok(mut guard) = cell.lock()
                            {
                                *guard = Some(info);
                            }
                        });
                    }
                } else {
                    // Clear balance when switching to a non-DeepSeek provider.
                    if let Ok(mut guard) = app.balance_cell.lock() {
                        *guard = None;
                    }
                }
            }
            AppAction::SwitchModelRoute { provider, model } => {
                let previous_model = if app.auto_model {
                    "auto".to_string()
                } else {
                    app.model.clone()
                };
                let previous_effort = app.reasoning_effort;
                apply_model_picker_choice(
                    app,
                    engine_handle,
                    config,
                    model,
                    Some(provider),
                    None,
                    previous_effort,
                    previous_model,
                    previous_effort,
                )
                .await;
            }
            AppAction::UpdateCompaction(compaction) => {
                apply_model_and_compaction_update(
                    engine_handle,
                    compaction,
                    app.mode,
                    app.active_route_limits,
                )
                .await;
            }
            AppAction::UpdateStreamChunkTimeout(timeout_secs) => {
                let _ = engine_handle
                    .send(Op::SetStreamChunkTimeout { timeout_secs })
                    .await;
            }
            AppAction::UpdateSubagentRuntimeConfig {
                enabled,
                max_subagents,
                launch_concurrency,
                max_spawn_depth,
                api_timeout_secs,
                heartbeat_timeout_secs,
            } => {
                let _ = engine_handle
                    .send(Op::SetSubagentRuntimeConfig {
                        enabled,
                        max_subagents,
                        launch_concurrency,
                        max_spawn_depth,
                        api_timeout_secs,
                        heartbeat_timeout_secs,
                    })
                    .await;
            }
            AppAction::OpenConfigEditor(mode) => match mode {
                ConfigUiMode::Native => {
                    if app.view_stack.top_kind() != Some(ModalKind::Config) {
                        app.view_stack.push(ConfigView::new_for_app(app));
                    }
                }
                ConfigUiMode::Tui => {
                    pause_terminal(
                        terminal,
                        app.use_alt_screen,
                        app.use_mouse_capture,
                        app.use_bracketed_paste,
                    )?;
                    let editor_result = config_ui::run_tui_editor(app, config)
                        .and_then(|doc| config_ui::apply_document(doc, app, config, true));
                    resume_terminal(
                        terminal,
                        app.use_alt_screen,
                        app.use_mouse_capture,
                        app.use_bracketed_paste,
                        app.synchronized_output_enabled,
                    )?;
                    match editor_result {
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
                                content: format!("Config UI failed: {err}"),
                            });
                        }
                    }
                }
                ConfigUiMode::Web => {
                    #[cfg(feature = "web")]
                    {
                        let session = config_ui::start_web_editor(app, config).await?;
                        let url = format!("http://{}", session.addr);
                        let open_err = config_ui::open_browser(&url).err();
                        if let Some(err) = open_err {
                            app.add_message(HistoryCell::System {
                                content: format!("Failed to open browser automatically: {err}"),
                            });
                        }
                        app.status_message = Some(format!("web ui listen on: {url}"));
                        *web_config_session = Some(session);
                    }
                    #[cfg(not(feature = "web"))]
                    {
                        app.add_message(HistoryCell::System {
                            content: "This build does not include the web config UI.".to_string(),
                        });
                    }
                }
            },
            AppAction::OpenConfigView => {
                if app.view_stack.top_kind() != Some(ModalKind::Config) {
                    app.view_stack.push(ConfigView::new_for_app(app));
                }
            }
            AppAction::OpenModelPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ModelPicker) {
                    app.view_stack
                        .push(crate::tui::model_picker::ModelPickerView::new(app, config));
                }
            }
            AppAction::OpenProviderPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_with_runtime_status_and_memory(
                            app.api_provider,
                            config,
                            runtime_status,
                            app.provider_picker_memory.as_ref(),
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                }
            }
            AppAction::OpenProviderSetup { provider } => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_for_setup(
                            app.api_provider,
                            provider,
                            config,
                            runtime_status,
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                    app.status_message = Some("Provider setup catalog opened.".to_string());
                }
            }
            AppAction::StartXaiDeviceLogin => {
                run_xai_device_login_from_tui(terminal, app, engine_handle, config).await?;
            }
            AppAction::OpenModePicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ModePicker) {
                    app.view_stack
                        .push(crate::tui::views::mode_picker::ModePickerView::new(
                            app.mode,
                            app.ui_locale,
                        ));
                }
            }
            AppAction::OpenStatusPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::StatusPicker) {
                    app.view_stack
                        .push(crate::tui::views::status_picker::StatusPickerView::new(
                            &app.status_items,
                            app.api_provider,
                            app.ui_locale,
                        ));
                }
            }
            AppAction::OpenFeedbackPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::FeedbackPicker) {
                    app.view_stack
                        .push(crate::tui::feedback_picker::FeedbackPickerView::new());
                }
            }
            AppAction::OpenThemePicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ThemePicker) {
                    // Capture the active theme name straight from `app` so
                    // Esc can revert through the same ConfigUpdated channel.
                    // Avoids re-reading settings.toml from disk on every
                    // `/theme` invocation.
                    let original = app.theme_id.name().to_string();
                    app.view_stack.push_boxed(
                        crate::tui::theme_picker::ThemePickerView::boxed_with_treatment(
                            original,
                            app.ocean_treatment,
                            app.ui_locale,
                            app.background_color_override,
                        ),
                    );
                }
            }
            AppAction::OpenSkillsManager => {
                if app.view_stack.top_kind() != Some(ModalKind::SkillsManager) {
                    app.view_stack
                        .push(crate::tui::views::skills_manager::SkillsManagerView::new(
                            app,
                        ));
                }
            }
            AppAction::OpenFleetRoster => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetRoster) {
                    app.view_stack
                        .push(crate::tui::views::fleet_roster::FleetRosterView::new(
                            app, config,
                        ));
                }
            }
            AppAction::OpenFleetSetup => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetSetup) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::views::fleet_setup::FleetSetupView::new(
                            app, config,
                        ));
                }
            }
            AppAction::OpenHotbarSetup => {
                if app.view_stack.top_kind() != Some(ModalKind::HotbarSetup) {
                    app.view_stack
                        .push(crate::tui::hotbar::setup::HotbarSetupView::new(app, config));
                }
            }
            AppAction::OpenSetupWizard => {
                if app.view_stack.top_kind() != Some(ModalKind::SetupWizard) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::setup::SetupWizardView::new_for_app(app, config));
                }
            }
            AppAction::OpenSetupWizardAt { step } => {
                if app.view_stack.top_kind() != Some(ModalKind::SetupWizard) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::setup::SetupWizardView::new_for_app_at(
                            app, config, step,
                        ));
                }
            }
            AppAction::UseBundledConstitution => use_bundled_constitution(app, config),
            AppAction::DisableHotbar => disable_hotbar(app, config),
            AppAction::RestoreHotbarDefaults => restore_hotbar_defaults(app, config),
            AppAction::OpenExternalUrl { url, label } => match open_external_url(&url) {
                Ok(()) => {
                    app.status_message = Some(format!("Opened {label} in your browser"));
                }
                Err(err) => {
                    app.add_message(HistoryCell::System {
                        content: format!(
                            "Could not open {label} automatically: {err}\n\nThe URL is printed above."
                        ),
                    });
                }
            },
            AppAction::OpenContextInspector => {
                open_context_inspector(app);
            }
            AppAction::OpenLiveTranscript => {
                open_live_transcript_overlay(app);
            }
            AppAction::CompactContext => {
                app.status_message = Some("Compacting context...".to_string());
                match validated_app_runtime_route(app, config) {
                    Ok(route) => {
                        let compaction = compaction_for_validated_route(app, &route);
                        let _ = engine_handle
                            .send(Op::CompactContext {
                                route: Box::new(route.into_resolved()),
                                compaction: Box::new(compaction),
                            })
                            .await;
                    }
                    Err(err) => {
                        app.status_message = Some(format!(
                            "Cannot compact because the active provider route is invalid: {err}"
                        ));
                    }
                }
            }
            AppAction::PurgeContext => {
                app.status_message = Some("Agent purging context...".to_string());
                let _ = engine_handle.send(Op::PurgeContext).await;
            }
            AppAction::TaskAdd { prompt } => {
                let request = NewTaskRequest {
                    prompt: prompt.clone(),
                    model: Some(app.model.clone()),
                    workspace: Some(app.workspace.clone()),
                    mode: Some(task_mode_label(app.mode).to_string()),
                    allow_shell: Some(app.allow_shell),
                    trust_mode: Some(app.trust_mode),
                    auto_approve: Some(app_auto_approve_enabled(app)),
                };
                match task_manager.add_task(request).await {
                    Ok(task) => {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Task queued: {} ({})",
                                task.id,
                                summarize_tool_output(&task.prompt)
                            ),
                        });
                        app.status_message = Some(format!("Queued {}", task.id));
                    }
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Failed to queue task: {err}"),
                        });
                    }
                }
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::TaskList => {
                let tasks = task_manager.list_tasks(Some(30)).await;
                refresh_active_task_panel(app, task_manager).await;
                app.add_message(HistoryCell::System {
                    content: format_task_list(&tasks),
                });
            }
            AppAction::TaskShow { id } => match task_manager.get_task(&id).await {
                Ok(task) => open_task_pager(app, &task),
                Err(err) => {
                    app.add_message(HistoryCell::System {
                        content: format!("Task lookup failed: {err}"),
                    });
                }
            },
            AppAction::TaskCancel { id } => {
                match task_manager.cancel_task(&id).await {
                    Ok(cancellation) => {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Task {} status: {:?}",
                                cancellation.task.id, cancellation.task.status
                            ),
                        });
                    }
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Task cancel failed: {err}"),
                        });
                    }
                }
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::ShellJob(action) => {
                handle_shell_job_action(app, action);
                // Immediately sync the task panel after cancel/poll so the
                // Activity sidebar stays accurate without waiting for the
                // next 2.5 s periodic refresh (#2937).
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::Mcp(action) => {
                handle_mcp_ui_action(app, engine_handle, config, action).await;
            }
            AppAction::SwitchWorkspace { workspace } => {
                switch_workspace(app, engine_handle, task_manager, config, workspace).await;
            }
            AppAction::SwitchProfile { profile } => {
                let previous_profile = app.config_profile.clone();
                match Config::load(app.config_path.clone(), Some(&profile)).and_then(|new_config| {
                    validated_profile_default_route(&new_config)
                        .map(|validated_route| (new_config, validated_route))
                }) {
                    Ok((new_config, validated_route)) => {
                        let new_model = validated_route.model.clone();
                        let provider_identity = validated_route.identity.clone();
                        let route_limits = crate::route_budget::known_route_limits(
                            validated_route.candidate.limits(),
                        );
                        app.config_profile = Some(profile.clone());
                        *config = new_config.clone();
                        app.set_provider_identity_record(provider_identity);
                        app.billing_presentation =
                            crate::route_billing::for_route(config, app.api_provider);
                        app.set_model_selection(new_model.clone());
                        app.set_active_context_window_override(
                            config.context_window_for_provider_config(app.api_provider),
                        );
                        app.active_route_limits = route_limits;
                        app.update_model_compaction_budget();
                        app.session.last_prompt_tokens = None;
                        app.session.last_completion_tokens = None;
                        app.session.last_output_throughput = None;
                        // Rebuild the engine with the new config so API key/model/base URL take effect.
                        let _ = engine_handle.send(Op::Shutdown).await;
                        let engine_config = build_engine_config(app, config);
                        *engine_handle = spawn_tui_engine(engine_config, config);
                        if !app.api_messages.is_empty() {
                            let _ = engine_handle
                                .send(Op::SyncSession {
                                    session_id: app.current_session_id.clone(),
                                    messages: app.api_messages.clone(),
                                    system_prompt: app.system_prompt.clone(),
                                    system_prompt_override: false,
                                    model: app.model.clone(),
                                    workspace: app.workspace.clone(),
                                    mode: app.mode,
                                })
                                .await;
                        }
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Switched to profile '{profile}'. Model: {new_model}, Provider: {}",
                                app.provider_identity_for_persistence()
                            ),
                        });
                        app.status_message = Some(format!("Profile: {profile}"));
                    }
                    Err(err) => {
                        app.config_profile = previous_profile;
                        app.status_message =
                            Some(format!("Failed to switch to profile '{profile}': {err}"));
                    }
                }
            }
            AppAction::ShareSession {
                history_len: _,
                model,
                mode,
            } => {
                let status = if app.api_messages.is_empty() {
                    "No session content to share.".to_string()
                } else {
                    let history_json = serde_json::to_string_pretty(&app.api_messages)
                        .unwrap_or_else(|_| "[]".to_string());
                    match crate::commands::share::perform_share(&history_json, &model, &mode).await
                    {
                        Ok(url) => format!("Session shared! URL: {url}"),
                        Err(err) => format!("Share failed: {err}"),
                    }
                };
                app.add_message(HistoryCell::System {
                    content: status.clone(),
                });
                app.status_message = Some(status);
            }
        }
    }

    Ok(false)
}

#[cfg(test)]
use std::process::{Command, Stdio};

fn open_external_url(url: &str) -> Result<()> {
    crate::utils::open_url(url)
}

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

fn apply_workspace_runtime_state(app: &mut App, config: &Config, workspace: PathBuf) {
    app.workspace = workspace.clone();
    app.coordination_detail = None;
    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
    app.active_skill = None;
    app.active_skill_provenance = None;
    app.hooks = HookExecutor::new(
        crate::hooks::HooksConfig::load_with_project(config.hooks_config(), &workspace),
        workspace.clone(),
    );
    app.skills_dir = crate::tui::app::resolve_skills_dir(&workspace, &config.skills_dir(), config);
    app.skills_scan_codewhale_only = config.skills_config().scan_codewhale_only();
    app.refresh_skill_cache();
    app.workspace_context = None;
    if let Ok(mut cell) = app.workspace_context_cell.lock() {
        *cell = None;
    }
    app.workspace_context_refreshed_at = None;
    app.file_tree = None;

    let shell_manager = crate::tools::shell::new_shared_shell_manager(workspace);
    app.runtime_services.shell_manager = Some(shell_manager);
    app.runtime_services.hook_executor = Some(std::sync::Arc::new(app.hooks.clone()));
}

async fn sync_runtime_workspace_state(task_manager: &SharedTaskManager, workspace: PathBuf) {
    task_manager.set_default_workspace(workspace).await;
}

async fn switch_workspace(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    task_manager: &SharedTaskManager,
    config: &Config,
    workspace: PathBuf,
) {
    if app.is_loading {
        app.status_message =
            Some("Cannot switch workspace while a request is running.".to_string());
        app.add_message(HistoryCell::System {
            content: "Cannot switch workspace while a request is running.".to_string(),
        });
        return;
    }

    if app.workspace == workspace {
        app.status_message = Some(format!("Workspace unchanged: {}", workspace.display()));
        return;
    }

    apply_workspace_runtime_state(app, config, workspace.clone());
    sync_runtime_workspace_state(task_manager, workspace.clone()).await;

    let _ = engine_handle.send(Op::Shutdown).await;
    let engine_config = build_engine_config(app, config);
    *engine_handle = spawn_tui_engine(engine_config, config);
    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: workspace.clone(),
                mode: app.mode,
            })
            .await;
    }

    app.add_message(HistoryCell::System {
        content: format!("Switched workspace to {}", workspace.display()),
    });
    app.status_message = Some(format!("Workspace: {}", workspace.display()));
}

async fn handle_mcp_ui_action(
    app: &mut App,
    engine_handle: &EngineHandle,
    config: &Config,
    action: crate::tui::app::McpUiAction,
) {
    use crate::mcp::{self, McpWriteStatus};

    let path = app.mcp_config_path.clone();
    let mut changed = false;
    let mut message = None;
    let is_reload = matches!(&action, crate::tui::app::McpUiAction::Reload);
    let discover = mcp_ui_action_refreshes_discovery(&action);

    let action_result = match action {
        crate::tui::app::McpUiAction::Show => Ok(()),
        crate::tui::app::McpUiAction::Init { force } => {
            changed = true;
            match mcp::init_config(&path, force) {
                Ok(McpWriteStatus::Created) => {
                    message = Some(format!("Created MCP config at {}", path.display()));
                    Ok(())
                }
                Ok(McpWriteStatus::Overwritten) => {
                    message = Some(format!("Overwrote MCP config at {}", path.display()));
                    Ok(())
                }
                Ok(McpWriteStatus::SkippedExists) => {
                    changed = false;
                    message = Some(format!(
                        "MCP config already exists at {} (use /mcp init --force to overwrite)",
                        path.display()
                    ));
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        crate::tui::app::McpUiAction::AddStdio {
            name,
            command,
            args,
        } => {
            changed = true;
            mcp::add_server_config(&path, name.clone(), Some(command), None, args, None)
                .map(|()| message = Some(format!("Added MCP stdio server '{name}'")))
        }
        crate::tui::app::McpUiAction::AddHttp {
            name,
            url,
            transport,
        } => {
            changed = true;
            mcp::add_server_config(&path, name.clone(), None, Some(url), Vec::new(), transport)
                .map(|()| message = Some(format!("Added MCP HTTP/SSE server '{name}'")))
        }
        crate::tui::app::McpUiAction::Enable { name } => {
            changed = true;
            mcp::set_server_enabled(&path, &name, true)
                .map(|()| message = Some(format!("Enabled MCP server '{name}'")))
        }
        crate::tui::app::McpUiAction::Disable { name } => {
            changed = true;
            mcp::set_server_enabled(&path, &name, false)
                .map(|()| message = Some(format!("Disabled MCP server '{name}'")))
        }
        crate::tui::app::McpUiAction::Remove { name } => {
            changed = true;
            mcp::remove_server_config(&path, &name)
                .map(|()| message = Some(format!("Removed MCP server '{name}'")))
        }
        crate::tui::app::McpUiAction::Login { name, scopes } => {
            let result = async {
                let cfg = mcp::load_config_with_workspace_and_plugins(
                    &path,
                    &app.workspace,
                    app.plugin_registry.as_ref(),
                )?;
                let server = cfg
                    .servers
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' not found"))?;
                let explicit_scopes = (!scopes.is_empty()).then_some(scopes);
                mcp::oauth::perform_oauth_login_for_server(
                    &name,
                    server,
                    explicit_scopes,
                    config.mcp_oauth_callback_port,
                    config.mcp_oauth_callback_url.as_deref(),
                )
                .await
            }
            .await;
            result.map(|()| {
                changed = true;
                message = Some(format!(
                    "Stored OAuth credentials for MCP server '{name}'. Run /mcp reload to reconnect it."
                ));
            })
        }
        crate::tui::app::McpUiAction::Logout { name } => {
            let result = (|| {
                let cfg = mcp::load_config_with_workspace_and_plugins(
                    &path,
                    &app.workspace,
                    app.plugin_registry.as_ref(),
                )?;
                let server = cfg
                    .servers
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' not found"))?;
                mcp::oauth::delete_oauth_tokens_for_server(&name, server)
            })();
            result.map(|deleted| {
                changed = deleted;
                message = Some(if deleted {
                    format!(
                        "Deleted stored OAuth credentials for MCP server '{name}'. Run /mcp reload to reconnect it."
                    )
                } else {
                    format!("No stored OAuth credentials found for MCP server '{name}'.")
                });
            })
        }
        crate::tui::app::McpUiAction::Validate | crate::tui::app::McpUiAction::Reload => Ok(()),
    };

    if let Err(err) = action_result {
        add_mcp_message(app, format!("MCP action failed: {err}"));
        return;
    }

    if changed {
        app.mcp_reload_required = true;
    }
    if let Some(message) = message {
        add_mcp_message(app, message);
    }

    let snapshot_result = if is_reload {
        match engine_handle.reload_mcp(path.clone()).await {
            Ok(snapshot) => {
                app.mcp_reload_required = false;
                add_mcp_message(app, mcp_reload_summary(&snapshot));
                Ok(snapshot)
            }
            Err(error) => {
                app.mcp_reload_required = true;
                Err(error)
            }
        }
    } else if discover {
        let network_policy = config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        });
        mcp::discover_manager_snapshot_with_workspace_and_plugins(
            &path,
            &app.workspace,
            network_policy,
            app.mcp_reload_required,
            std::sync::Arc::clone(&app.plugin_registry),
        )
        .await
    } else {
        mcp::manager_snapshot_from_config_with_workspace_and_plugins(
            &path,
            &app.workspace,
            app.mcp_reload_required,
            app.plugin_registry.as_ref(),
        )
    };

    match snapshot_result {
        Ok(snapshot) => {
            if discover {
                add_mcp_message(
                    app,
                    "MCP discovery refreshed for the UI. Run /mcp reload after config or credential edits to rebuild the live model-visible tool pool.".to_string(),
                );
            }
            // Keep the boot-time MCP-count chip in sync with the live
            // snapshot so footers and panels reflect post-/mcp edits
            // (#502).
            app.mcp_configured_count = snapshot.servers.len();
            app.mcp_snapshot = Some(snapshot.clone());
            // #2068: keep the hotbar's MCP-tool actions in sync with the tools
            // that are actually loaded; the hotbar never connects on its own.
            app.hotbar_actions.replace_mcp_tools(Some(&snapshot));
            open_mcp_manager_pager(app, &snapshot);
        }
        Err(err) if is_reload => add_mcp_message(
            app,
            format!("MCP reload failed; the live tool pool is unchanged: {err}"),
        ),
        Err(err) => add_mcp_message(app, format!("MCP snapshot failed: {err}")),
    }
}

fn mcp_reload_summary(snapshot: &crate::mcp::McpManagerSnapshot) -> String {
    let connected = snapshot
        .servers
        .iter()
        .filter(|server| server.connected)
        .count();
    let failed = snapshot
        .servers
        .iter()
        .filter(|server| server.enabled && server.error.is_some())
        .count();
    let disabled = snapshot
        .servers
        .iter()
        .filter(|server| !server.enabled)
        .count();
    format!(
        "MCP tool pool reloaded in process: {connected} connected, {failed} failed, {disabled} disabled. The next model turn uses this catalog."
    )
}

fn mcp_ui_action_refreshes_discovery(action: &crate::tui::app::McpUiAction) -> bool {
    matches!(
        action,
        crate::tui::app::McpUiAction::Show
            | crate::tui::app::McpUiAction::Validate
            | crate::tui::app::McpUiAction::Login { .. }
            | crate::tui::app::McpUiAction::Logout { .. }
    )
}

fn handle_shell_job_action(app: &mut App, action: crate::tui::app::ShellJobAction) {
    let Some(shell_manager) = app.runtime_services.shell_manager.clone() else {
        add_shell_job_message(app, "No shell session is active.".to_string());
        return;
    };

    let mut manager = match shell_manager.lock() {
        Ok(manager) => manager,
        Err(_) => {
            add_shell_job_message(
                app,
                "Shell tracking hit an internal error — restart Codewhale to recover.".to_string(),
            );
            return;
        }
    };

    match action {
        crate::tui::app::ShellJobAction::List => {
            let jobs = manager.list_jobs();
            add_shell_job_message(app, format_shell_job_list(&jobs));
        }
        crate::tui::app::ShellJobAction::Show { id } => match manager.inspect_job(&id) {
            Ok(detail) => open_shell_job_pager(app, &detail),
            Err(err) => add_shell_job_message(app, format!("Command lookup failed: {err}")),
        },
        crate::tui::app::ShellJobAction::Poll { id, wait } => {
            match manager.poll_delta(&id, wait, if wait { 5_000 } else { 1_000 }) {
                Ok(delta) => add_shell_job_message(app, format_shell_poll(&delta.result)),
                Err(err) => add_shell_job_message(app, format!("Command poll failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::SendStdin { id, input, close } => {
            match manager.write_stdin(&id, &input, close) {
                Ok(()) => match manager.poll_delta(&id, false, 1_000) {
                    Ok(delta) => add_shell_job_message(app, format_shell_poll(&delta.result)),
                    Err(err) => {
                        add_shell_job_message(
                            app,
                            format!("Command input sent; poll failed: {err}"),
                        );
                    }
                },
                Err(err) => add_shell_job_message(app, format!("Command input failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::Cancel { id } => match manager.kill(&id) {
            Ok(result) => add_shell_job_message(app, format_shell_poll(&result)),
            Err(err) => add_shell_job_message(app, format!("Command cancel failed: {err}")),
        },
        crate::tui::app::ShellJobAction::CancelAll => match manager.kill_running() {
            Ok(results) => {
                let count = results.len();
                if count == 0 {
                    add_shell_job_message(app, "No running commands to cancel.".to_string());
                } else {
                    let tasks: Vec<String> = results
                        .iter()
                        .filter_map(|result| result.task_id.clone())
                        .collect();
                    add_shell_job_message(
                        app,
                        format!("Canceled {count} command(s): {}", tasks.join(", ")),
                    );
                }
            }
            Err(err) => add_shell_job_message(app, format!("Command cancel-all failed: {err}")),
        },
    }
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

fn clear_active_provider_api_key_from_memory(app: &App, config: &mut Config) {
    let active_identity = app.provider_identity_for_persistence();
    let clears_legacy_root = matches!(
        app.api_provider,
        ApiProvider::Deepseek | ApiProvider::DeepseekCN
    ) || (app.api_provider == ApiProvider::Custom
        && active_identity == ApiProvider::Custom.as_str()
        && config.uses_legacy_literal_custom_route());
    if clears_legacy_root {
        config.api_key = None;
    }
    config.set_provider_api_key_override(app.api_provider, None);
    if app.api_provider == ApiProvider::Xai {
        let entry = config.provider_config_for_mut(ApiProvider::Xai);
        entry.auth_mode = None;
        entry.oauth_credential_generation = None;
        entry.external_credentials = None;
    }
}

fn parse_queue_send_command(input: &str) -> Option<Result<usize, String>> {
    let rest = strip_queue_command_prefix(input.trim())?;
    let mut parts = rest.split_whitespace();
    let action = parts.next()?;
    if !action.eq_ignore_ascii_case("send") && !action.eq_ignore_ascii_case("now") {
        return None;
    }
    let Some(raw_index) = parts.next() else {
        return Some(Err("Usage: /queue send <n>".to_string()));
    };
    if parts.next().is_some() {
        return Some(Err("Usage: /queue send <n>".to_string()));
    }
    let Ok(index) = raw_index.parse::<usize>() else {
        return Some(Err("Index must be a positive number".to_string()));
    };
    if index == 0 {
        return Some(Err("Index must be >= 1".to_string()));
    }
    Some(Ok(index - 1))
}

fn strip_queue_command_prefix(input: &str) -> Option<&str> {
    for prefix in ["/queue", "/queued"] {
        if let Some(rest) = input.strip_prefix(prefix)
            && (rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace))
        {
            return Some(rest);
        }
    }
    None
}

async fn steer_user_message(
    app: &mut App,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) -> Result<()> {
    let paused_snapshot = snapshot_steer_paused_state(app);
    let paused_dispatch = plan_paused_command_message(app, &message.display);
    let paused_note = paused_dispatch.note().map(str::to_string);
    paused_dispatch.apply(app, engine_handle);
    let cwd = std::env::current_dir().ok();
    let references = crate::tui::file_mention::context_references_from_input(
        &message.display,
        &app.workspace,
        cwd.clone(),
    );
    let mut content = queued_message_content_for_app(app, &message, cwd)?;
    if let Some(note) = paused_note.as_deref() {
        content.push_str(note);
    }
    let message_index = app.api_messages.len();

    if let Err(err) = engine_handle.steer(content.clone()).await {
        restore_steer_paused_state(app, &paused_snapshot);
        engine_handle.set_paused(paused_snapshot.paused);
        return Err(err);
    }
    app.last_submitted_prompt = Some(message.display.clone());

    // Flush any streaming thinking/tool content into history before
    // inserting the steer message, so the steer appears after (below)
    // the content that chronologically preceded it.
    app.flush_active_cell();

    // Mirror steer input in local transcript/session state.
    app.add_message(HistoryCell::User {
        content: format!("+ {}", message.display),
    });
    let history_cell = app.history.len().saturating_sub(1);
    app.record_context_references(history_cell, message_index, references);
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: content.clone(),
            cache_control: None,
        }],
    });

    app.status_message = Some("Steering current turn...".to_string());
    Ok(())
}

#[derive(Debug, Clone)]
struct SteerPausedSnapshot {
    paused: bool,
    pausable: bool,
    paused_quarry: Option<String>,
    quarry: Option<String>,
    tokens_used: u64,
    time_used_seconds: u64,
    continuation_count: u32,
}

fn snapshot_steer_paused_state(app: &App) -> SteerPausedSnapshot {
    SteerPausedSnapshot {
        paused: app.paused,
        pausable: app.pausable,
        paused_quarry: app.paused_quarry.clone(),
        quarry: app.hunt.quarry.clone(),
        tokens_used: app.hunt.tokens_used,
        time_used_seconds: app.hunt.time_used_seconds,
        continuation_count: app.hunt.continuation_count,
    }
}

fn restore_steer_paused_state(app: &mut App, snapshot: &SteerPausedSnapshot) {
    app.paused = snapshot.paused;
    app.pausable = snapshot.pausable;
    app.paused_quarry = snapshot.paused_quarry.clone();
    app.hunt.quarry = snapshot.quarry.clone();
    app.hunt.tokens_used = snapshot.tokens_used;
    app.hunt.time_used_seconds = snapshot.time_used_seconds;
    app.hunt.continuation_count = snapshot.continuation_count;
}

async fn attempt_steer_with_queue_fallback(
    app: &mut App,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) {
    match steer_user_message(app, engine_handle, message.clone()).await {
        Ok(()) => {
            app.push_status_toast(
                "Steering into current turn",
                StatusToastLevel::Info,
                Some(1_500),
            );
        }
        Err(err) => {
            enqueue_offline_message(app, message);
            let status = format!(
                "Steer failed ({err}); {} queued follow-up(s) — /queue send <n>",
                app.queued_message_count()
            );
            app.status_message = Some(status.clone());
            app.push_status_toast(status, StatusToastLevel::Warning, Some(4_000));
        }
    }
}

/// Park a draft on the queued-messages bucket for dispatch after TurnComplete.
/// Unlike a steer, the message is NOT forwarded immediately — it waits for
/// the current turn to finish, then dispatches as a normal user message.
async fn queue_follow_up(app: &mut App, message: QueuedMessage) -> Result<()> {
    let display = message.display.clone();
    enqueue_offline_message(app, message);
    let toast = if app.mode == AppMode::Operate {
        format!(
            "Queued task: {display} ({} total) — dispatches next while workers continue; ↑ to edit",
            app.queued_message_count()
        )
    } else {
        format!(
            "Queued: {display} ({} total) — sends after current output; ↑ to edit",
            app.queued_message_count()
        )
    };
    app.status_message = Some(toast.clone());
    app.push_status_toast(toast, StatusToastLevel::Info, Some(3_000));
    Ok(())
}

async fn submit_or_steer_message(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) -> Result<()> {
    match app
        .enter_with_double_tap()
        .unwrap_or(SubmitDisposition::Immediate)
    {
        SubmitDisposition::Immediate => {
            let _ = dispatch_user_message(app, config, engine_handle, message).await;
            Ok(())
        }
        SubmitDisposition::Queue => {
            let count = app.queued_message_count().saturating_add(1);
            enqueue_offline_message(app, message);
            let (status, toast) = if app.offline_mode {
                (
                    format!("Offline: {count} queued follow-up(s) — ↑ edit last, /queue send <n>"),
                    format!("Offline: queued follow-up ({count} total)"),
                )
            } else if app.mode == AppMode::Operate {
                (
                    format!(
                        "{count} queued task(s) — dispatches next while workers continue; ↑ edit last, /queue send <n>"
                    ),
                    format!("Queued task ({count} total) — dispatches next"),
                )
            } else {
                (
                    format!(
                        "{count} queued follow-up(s) — sends after current output; ↑ edit last, /queue send <n>"
                    ),
                    format!("Queued follow-up ({count} total) — sends after current output"),
                )
            };
            app.status_message = Some(status);
            app.push_status_toast(toast, StatusToastLevel::Info, Some(3_000));
            Ok(())
        }
        // Steer: reached via Enter when busy-but-waiting (v0.8.44), or
        // via Ctrl+Enter override in any busy state.
        SubmitDisposition::Steer => {
            attempt_steer_with_queue_fallback(app, engine_handle, message).await;
            Ok(())
        }
        SubmitDisposition::QueueFollowUp => queue_follow_up(app, message).await,
    }
}

fn restore_failed_immediate_submit(app: &mut App, message: QueuedMessage, error: &anyhow::Error) {
    tracing::warn!(
        error = %error,
        "immediate user message dispatch failed; restored composer"
    );
    app.input = message.display;
    app.cursor_position = app.input.chars().count();
    app.active_skill = message.skill_instruction;
    app.active_skill_provenance = message.skill_provenance;
    let status = tr(app.ui_locale, MessageId::ComposerDispatchFailedRestored)
        .replace("{error}", &error.to_string());
    app.status_message = Some(status.clone());
    app.set_sticky_status(status, StatusToastLevel::Error, None);
    app.needs_redraw = true;
}

/// Drain `app.pending_steers` into a single `QueuedMessage` ready for
/// `dispatch_user_message`. Returns `None` if the queue was empty (caller
/// then falls back to `app.queued_messages`). Skill instruction is taken
/// from the first message that supplies one — multiple steers shouldn't
/// double-up the system framing.
fn merge_pending_steers(app: &mut App) -> Option<QueuedMessage> {
    let drained = app.drain_pending_steers();
    if drained.is_empty() {
        return None;
    }
    if drained.len() == 1 {
        return drained.into_iter().next();
    }
    let mut skill_instruction: Option<String> = None;
    let mut skill_provenance = None;
    let mut bodies: Vec<String> = Vec::with_capacity(drained.len());
    for msg in drained {
        if skill_instruction.is_none() {
            skill_instruction = msg.skill_instruction;
            skill_provenance = msg.skill_provenance;
        }
        bodies.push(msg.display);
    }
    Some(
        QueuedMessage::new(bodies.join("\n\n"), skill_instruction)
            .with_skill_provenance(skill_provenance),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanChoice {
    AcceptAgent,
    AcceptYolo,
    RevisePlan,
    ExitPlan,
}

fn plan_next_step_prompt() -> String {
    [
        "Action required: choose the next step for this plan.",
        "  1) Accept + implement in Act mode",
        "  2) Accept + implement with Full Access (trusted workspace)",
        "  3) Revise the plan / ask follow-ups",
        "  4) Return to Act mode without implementing",
        "",
        "Use the plan confirmation popup, or type 1-4 and press Enter.",
    ]
    .join("\n")
}

fn plan_choice_from_option(option: usize) -> Option<PlanChoice> {
    match option {
        1 => Some(PlanChoice::AcceptAgent),
        2 => Some(PlanChoice::AcceptYolo),
        3 => Some(PlanChoice::RevisePlan),
        4 => Some(PlanChoice::ExitPlan),
        _ => None,
    }
}

fn parse_plan_choice(input: &str) -> Option<PlanChoice> {
    // Once the modal is dismissed, only the advertised 1-4 fallback remains active.
    // Letter shortcuts stay modal-only so normal messages like "yolo" are not captured.
    match input.trim() {
        "1" => Some(PlanChoice::AcceptAgent),
        "2" => Some(PlanChoice::AcceptYolo),
        "3" => Some(PlanChoice::RevisePlan),
        "4" => Some(PlanChoice::ExitPlan),
        _ => None,
    }
}

async fn apply_plan_choice(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    choice: PlanChoice,
) -> Result<()> {
    let acceptance = match choice {
        PlanChoice::AcceptAgent => PlanAcceptance::AcceptAct,
        PlanChoice::AcceptYolo => PlanAcceptance::AcceptFullAccess,
        PlanChoice::RevisePlan => PlanAcceptance::Revise,
        PlanChoice::ExitPlan => PlanAcceptance::Exit,
    };
    project_accepted_plan(
        app.runtime_services.work.as_ref(),
        app.current_session_id.as_deref(),
        acceptance,
        app.pending_plan_proposal_id.as_ref(),
    )
    .await
    .map_err(|err| anyhow::anyhow!("failed to project accepted plan: {err}"))?;
    app.pending_plan_proposal_id = None;
    persist_pending_work_checkpoint(app)
        .await
        .map_err(|err| anyhow::anyhow!("accepted plan was not checkpointed: {err}"))?;

    match choice {
        PlanChoice::AcceptAgent => {
            apply_mode_update(app, engine_handle, AppMode::Agent).await;
            app.add_message(HistoryCell::System {
                content: "Plan accepted. Switching to Act mode and starting implementation."
                    .to_string(),
            });
            let followup = QueuedMessage::new("Proceed with the accepted plan.".to_string(), None);
            if app.is_loading {
                app.queue_message(followup);
                app.status_message = Some("Queued accepted plan execution (Act mode).".to_string());
            } else {
                dispatch_user_message_with_recovery(
                    app,
                    config,
                    engine_handle,
                    followup,
                    DispatchRecovery::PlanFollowup,
                )
                .await?;
            }
        }
        PlanChoice::AcceptYolo => {
            apply_mode_update(app, engine_handle, AppMode::Yolo).await;
            app.add_message(HistoryCell::System {
                content:
                    "Plan accepted. Switching to Act + Full Access and starting implementation."
                        .to_string(),
            });
            let followup = QueuedMessage::new("Proceed with the accepted plan.".to_string(), None);
            if app.is_loading {
                app.queue_message(followup);
                app.status_message =
                    Some("Queued accepted plan execution (Act + Full Access).".to_string());
            } else {
                dispatch_user_message_with_recovery(
                    app,
                    config,
                    engine_handle,
                    followup,
                    DispatchRecovery::PlanFollowup,
                )
                .await?;
            }
        }
        PlanChoice::RevisePlan => {
            let prompt = "Revise the plan: ";
            app.input = prompt.to_string();
            app.cursor_position = prompt.chars().count();
            app.status_message = Some("Revise the plan and press Enter.".to_string());
        }
        PlanChoice::ExitPlan => {
            apply_mode_update(app, engine_handle, AppMode::Agent).await;
            app.add_message(HistoryCell::System {
                content: concat!(
                    "Exited Plan mode. Switched to Act mode.\n\n",
                    "The plan above is for reference only. ",
                    "Do NOT execute it until the user explicitly asks you to. ",
                    "Wait for the user's next instruction before taking any action.",
                )
                .to_string(),
            });
        }
    }

    Ok(())
}

async fn handle_plan_choice(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    input: &str,
) -> Result<bool> {
    if !app.plan_prompt_pending {
        return Ok(false);
    }

    let choice = parse_plan_choice(input);
    app.plan_prompt_pending = false;

    let Some(choice) = choice else {
        return Ok(false);
    };

    apply_plan_choice(app, config, engine_handle, choice).await?;
    Ok(true)
}

/// Build the pending-input preview widget from current `App` state.
///
/// v0.6.6 (#122) wires all three buckets:
/// - `pending_steers` — typed during a running turn + Esc; held until the
///   abort lands and gets resubmitted as a fresh merged turn.
/// - `rejected_steers` — engine declined a mid-turn steer (scaffolding;
///   no engine path produces these yet but the bucket renders with a distinct
///   rejected-steer label).
/// - `queued_messages` — Enter while busy; drained at end-of-turn. In Operate,
///   the foreground operator dispatches these as additional background tasks.
fn build_pending_input_preview(app: &App) -> PendingInputPreview {
    let mut preview = PendingInputPreview::new();
    let selected_attachment = app.selected_composer_attachment_index();
    let mut attachment_index = 0usize;
    preview.context_items = crate::tui::file_mention::pending_context_previews(&app.input)
        .into_iter()
        .map(|item| {
            let selected = if item.removable {
                let selected = selected_attachment == Some(attachment_index);
                attachment_index += 1;
                selected
            } else {
                false
            };
            ContextPreviewItem {
                kind: item.kind,
                label: item.label,
                detail: item.detail,
                included: item.included,
                removable: item.removable,
                selected,
            }
        })
        .collect();
    preview.pending_steers = app
        .pending_steers
        .iter()
        .map(|m| m.display.clone())
        .collect();
    preview.rejected_steers = app.rejected_steers.iter().cloned().collect();
    preview.queued_messages = app
        .queued_messages
        .iter()
        .map(|m| m.display.clone())
        .collect();
    preview.editing_queued_message = app.queued_draft.as_ref().map(|draft| {
        if app.input.trim().is_empty() {
            draft.display.clone()
        } else {
            app.input.clone()
        }
    });
    preview
}

fn classic_header_indicator_started_at(app: &App) -> Option<Instant> {
    app.motion_policy()
        .allows_status_spin()
        .then_some(app.turn_started_at)
        .flatten()
}

fn render_classic_header(area: Rect, buf: &mut Buffer, app: &App) {
    let context_usage = context_usage_snapshot(app);
    let context_window = context_usage.as_ref().map(|(_, max, _)| *max).or_else(|| {
        Some(crate::route_budget::route_context_window_tokens(
            app.api_provider,
            app.effective_model_for_budget(),
            app.active_route_limits,
        ))
    });
    let prompt_tokens = context_usage
        .as_ref()
        .and_then(|(used, _, _)| u32::try_from(*used).ok());
    let workspace = app
        .workspace
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("workspace");
    let model = app.model_display_label();
    let effort = app.reasoning_effort_display_label();
    let started_at = classic_header_indicator_started_at(app);
    let data = HeaderData::new(
        app.mode,
        &model,
        workspace,
        app.is_loading,
        app.ui_theme.header_bg,
    )
    .with_usage(
        app.session.total_conversation_tokens,
        context_window,
        app.session.session_cost,
        prompt_tokens,
    )
    .with_reasoning_effort(Some(&effort))
    .with_provider(None)
    .with_status_indicator(crate::tui::widgets::header_status_indicator_frame(
        started_at,
        &app.status_indicator,
    ));
    HeaderWidget::new(data).render(area, buf);
}

fn render(f: &mut Frame, app: &mut App, config: &Config) {
    let size = f.area();
    let classic_shell = app.ocean_treatment.is_classic();
    app.sidebar_hover = crate::tui::app::SidebarHoverState::default();
    app.viewport.last_approval_area = None;

    // Clear entire area with the configured app background.
    let background = Block::default().style(Style::default().bg(app.ui_theme.surface_bg));
    f.render_widget(background, size);

    // Show onboarding screen if needed
    if app.onboarding != OnboardingState::None {
        onboarding::render(f, size, app);
        // The provider step hosts the canonical setup picker as a modal on
        // top of the onboarding backdrop; without this the pushed view is
        // invisible and recovery appears to hang on an empty legacy screen.
        if app.onboarding == OnboardingState::Provider && !app.view_stack.is_empty() {
            let buf = f.buffer_mut();
            app.view_stack.render(size, buf);
        }
        return;
    }

    if app.launch.visible {
        crate::tui::underwater::render_launch_screen(size, f.buffer_mut(), app);
        crate::tui::underwater::record_launch_row_areas(size, &mut app.launch);
        if !app.view_stack.is_empty() {
            if app.view_stack.top_kind() == Some(ModalKind::Approval) {
                app.viewport.last_approval_area = app.view_stack.top_occupied_region(size);
            }
            let buf = f.buffer_mut();
            app.view_stack.render(size, buf);
        }
        return;
    }

    let header_height = if classic_shell || size.height < 16 {
        1
    } else {
        2
    };
    let footer_height = crate::tui::phase_strip::height();
    let slash_menu_entries = visible_slash_menu_entries(app, SLASH_MENU_LIMIT);
    let mention_menu_limit = app.mention_menu_limit;
    let mention_menu_entries =
        crate::tui::file_mention::visible_mention_menu_entries(app, mention_menu_limit);
    if !mention_menu_entries.is_empty() && app.mention_menu_selected >= mention_menu_entries.len() {
        app.mention_menu_selected = mention_menu_entries.len().saturating_sub(1);
    }
    let top_work_strip_height =
        super::work_surface::height(app, size.width, size.height, classic_shell);

    // Defensive two-pass layout: pin the header to the absolute top row,
    // then split the remaining body area for chat / preview / composer /
    // footer. This guarantees the header is never vertically centered
    // regardless of ratatui Flex defaults or terminal size.
    // Fixes #1834 — macOS terminal title centering.
    let (header_area, body_area) = {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .flex(ratatui::layout::Flex::Start)
            .constraints([Constraint::Length(header_height), Constraint::Min(1)])
            .split(size);
        (split[0], split[1])
    };

    let body_height = body_area.height;
    let composer_max_height = body_height
        .saturating_sub(MIN_CHAT_HEIGHT + footer_height + top_work_strip_height)
        .max(MIN_COMPOSER_HEIGHT);
    let composer_height = {
        let composer_widget = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        );
        composer_widget.desired_height(size.width)
    };

    // Pending-input preview (queued / steered messages). Empty when nothing's
    // queued, so zero height when idle. Phase 2 of #85 — solves the
    // "messages typed during a running turn vanish" complaint by giving the
    // user immediate visible feedback above the composer.
    let pending_preview = build_pending_input_preview(app);
    let desired_preview_height = pending_preview.desired_height(size.width);

    // WorkflowPanel unified activity surface (#4121). Collapsed to one row
    // while finished, expanded while running; zero height when no panel.
    let desired_workflow_panel_height = app
        .workflow_panel
        .as_ref()
        .map(|panel| panel.desired_height(size.width))
        .unwrap_or(0);
    let auxiliary_budget = body_height.saturating_sub(
        top_work_strip_height
            .saturating_add(MIN_CHAT_HEIGHT)
            .saturating_add(composer_height)
            .saturating_add(footer_height),
    );
    // Queued-only previews author the direct controls in row two (and fall
    // back to controls-only when just one row remains). Mixed previews retain
    // up to three compact rows at the release floor.
    let preview_cap = if size.height >= 20 { 4 } else { 3 };
    let preview_height = desired_preview_height.min(auxiliary_budget.min(preview_cap));
    let workflow_panel_height =
        desired_workflow_panel_height.min(auxiliary_budget.saturating_sub(preview_height));

    // Ocean live phases put the phase strip above the composer so activity
    // stays attached to the transcript and the prompt is the final bottom
    // object. Idle/typing keep a quiet phase under the prompt. Classic keeps
    // the legacy composer-then-footer stack.
    let phase = crate::tui::underwater::ShellPhase::from_app(app);
    let phase_above = !classic_shell
        && crate::tui::phase_strip::PhaseStripPlacement::for_phase(phase).is_above_composer();
    let (composer_slot, footer_slot, tail_constraints) = if phase_above {
        (
            5,
            4,
            [
                Constraint::Length(footer_height),
                Constraint::Length(composer_height),
            ],
        )
    } else {
        (
            4,
            5,
            [
                Constraint::Length(composer_height),
                Constraint::Length(footer_height),
            ],
        )
    };

    let body_chunks = Layout::default()
        .direction(Direction::Vertical)
        .flex(ratatui::layout::Flex::Start)
        .constraints([
            Constraint::Length(top_work_strip_height), // Tasks + To-do above transcript
            Constraint::Min(1),                        // Chat area
            Constraint::Length(workflow_panel_height), // Workflow panel (#4121)
            Constraint::Length(preview_height),        // Pending input preview (0 if empty)
            tail_constraints[0],
            tail_constraints[1],
        ])
        .split(body_area);

    let (work_chat_area, side_work_area) =
        super::work_surface::split_chat(app, body_chunks[1], classic_shell);

    if top_work_strip_height > 0 {
        super::work_surface::render(f, body_chunks[0], app);
    } else if let Some(work_area) = side_work_area {
        super::work_surface::render(f, work_area, app);
    }

    if classic_shell {
        render_classic_header(header_area, f.buffer_mut(), app);
    } else {
        crate::tui::underwater::render_header(header_area, f.buffer_mut(), app);
    }

    // Render the transcript and optional file-tree sidecar. The underwater
    // default deliberately has no legacy right sidebar: Tasks and To-do own
    // the strip above, Fleet owns `/fleet`, and dense context owns its
    // inspector. Keeping the sidebar here was the architectural reason the
    // rejected build still read as the old TUI under a gradient.
    let shell_ocean;
    {
        // Defensive backstop (#400): fill the entire body area with ink
        // background before any sub-widgets render, so cells that end up
        // uncovered by layout splits (e.g. after file-tree toggle or
        // resize) don't retain stale content from a previous frame.
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(work_chat_area, f.buffer_mut());

        // When the file-tree pane is visible and the terminal is wide
        // enough, reserve the left ~25% for the file tree.
        let mut chat_area =
            if app.file_tree.is_some() && work_chat_area.width >= SIDEBAR_VISIBLE_MIN_WIDTH {
                app.file_tree_visible = true;
                let split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                    .split(work_chat_area);
                let tree_area = split[0];
                let remaining = split[1];

                // Render the file-tree pane.
                if let Some(ref mut state) = app.file_tree {
                    super::file_tree::render_file_tree(f, tree_area, state, app.ui_theme.mode);
                }

                remaining
            } else {
                app.file_tree_visible = false;
                work_chat_area
            };
        app.last_sidebar_host_width = Some(chat_area.width);
        let sidebar_area = if classic_shell
            && !crate::tui::sidebar::sidebar_auto_idle(app)
            && let Some(sidebar_width) = sidebar_width_for_chat_area(app, chat_area.width)
        {
            app.sidebar_resize_total_width = chat_area.width;
            let split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(sidebar_width)])
                .split(chat_area);
            chat_area = split[0];
            Some(split[1])
        } else {
            None
        };
        app.viewport.last_sidebar_area = sidebar_area;
        if sidebar_area.is_none() {
            app.last_sidebar_area = None;
            app.last_sidebar_handle_area = None;
            app.sidebar_resizing = false;
            app.sidebar_hover_tooltip = None;
        }

        let chat_widget = ChatWidget::new(app, chat_area).with_ocean_viewport(size);
        shell_ocean = chat_widget.ocean_column();
        let buf = f.buffer_mut();
        chat_widget.render(chat_area, buf);

        // The rejected shell remains available only as an explicitly selected
        // compatibility treatment. It is never composed into the underwater
        // default path.
        if let Some(sidebar_area) = sidebar_area {
            app.last_sidebar_area = Some(sidebar_area);
            super::sidebar::render_sidebar(f, sidebar_area, app, config);
            let handle_area = Rect {
                x: sidebar_area.x,
                y: sidebar_area.y,
                width: 1,
                height: sidebar_area.height,
            };
            app.last_sidebar_handle_area = Some(handle_area);
            let handle =
                ratatui::widgets::Paragraph::new("│\n".repeat(usize::from(handle_area.height)))
                    .style(Style::default().fg(palette::TEXT_MUTED));
            f.render_widget(handle, handle_area);
        }
    }

    // Workflow panel between chat and pending-input preview (#4121).
    if workflow_panel_height > 0 {
        if let Some(panel) = app.workflow_panel.as_ref() {
            let area = body_chunks[2];
            app.viewport.last_workflow_panel_area = Some(area);
            app.viewport.last_workflow_cancel_area =
                panel.cancel_hint_span(area.width).map(|(start, end)| Rect {
                    x: area.x.saturating_add(start),
                    y: area.y,
                    width: end.saturating_sub(start),
                    height: 1,
                });
            let buf = f.buffer_mut();
            panel.render(area, buf);
        }
    } else {
        app.viewport.last_workflow_panel_area = None;
        app.viewport.last_workflow_cancel_area = None;
    }

    // Render pending-input preview (queued/steered messages, if any).
    if preview_height > 0 {
        let buf = f.buffer_mut();
        pending_preview.render(body_chunks[3], buf);
    }

    // Render composer
    let cursor_pos = {
        let composer_widget = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        );
        let buf = f.buffer_mut();
        composer_widget.render(body_chunks[composer_slot], buf);
        composer_widget.cursor_pos(body_chunks[composer_slot])
    };
    app.viewport.last_composer_area = Some(body_chunks[composer_slot]);
    {
        let area = body_chunks[composer_slot];
        let composer_widget = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        );
        let inner = if composer_widget.has_panel(area) {
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::TOP | ratatui::widgets::Borders::BOTTOM)
                .inner(area)
        } else if area.height >= 2 {
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::TOP)
                .inner(area)
        } else {
            area
        };
        app.viewport.last_composer_content = Some(inner);

        // Compute scroll offset and top padding for mouse coordinate mapping.
        let input_text = app.composer_display_input();
        let input_cursor = app.composer_display_cursor();
        let content_geometry =
            crate::tui::widgets::composer_content_geometry(inner, app.is_history_search_active());
        let content_width = content_geometry.text_width();
        let menu_lines = ComposerWidget::new(
            app,
            composer_max_height,
            &slash_menu_entries,
            &mention_menu_entries,
        )
        .active_menu_reserved_rows();
        let budget = crate::tui::widgets::composer_input_rows_budget(inner.height, menu_lines);
        let (_, _, _, scroll_offset) = crate::tui::widgets::layout_input_with_scroll(
            input_text,
            input_cursor,
            content_width,
            budget,
        );
        let visual_rows = if input_text.is_empty() {
            let hint: Option<std::borrow::Cow<'_, str>> = if let Some(ref suggestion) =
                app.prompt_suggestion
                && !app.is_history_search_active()
            {
                Some(std::borrow::Cow::Borrowed(suggestion.as_str()))
            } else {
                Some(crate::tui::widgets::composer_empty_hint_text(app))
            };
            crate::tui::widgets::empty_composer_visual_rows(hint.as_deref(), content_width, budget)
        } else {
            // Count wrapped lines (approximation matching the render path).
            crate::tui::widgets::wrap_input_lines_for_mouse(input_text, content_width).len()
        };
        let top_padding = budget.saturating_sub(visual_rows.clamp(1, budget));
        app.viewport.last_composer_scroll_offset = scroll_offset;
        app.viewport.last_composer_top_padding = top_padding;
    }
    if let Some(cursor_pos) = cursor_pos {
        f.set_cursor_position(cursor_pos);
    }

    if classic_shell {
        render_footer(f, body_chunks[footer_slot], app);
    } else {
        crate::tui::underwater::render_footer(body_chunks[footer_slot], f.buffer_mut(), app);
    }

    // The underwater shell is one water column, not a stack of independently
    // shaded panels. Continue the transcript's absolute-row ramp through each
    // ordinary shell surface after its foreground has rendered. Semantic
    // backgrounds such as selection, hover, errors, and code blocks do not
    // match these base colors and therefore remain intact.
    if let Some(column) = shell_ocean {
        column.paint_matching(header_area, f.buffer_mut(), app.ui_theme.header_bg);
        if top_work_strip_height > 0 {
            column.paint_matching(body_chunks[0], f.buffer_mut(), app.ui_theme.surface_bg);
        }
        if let Some(side_area) = side_work_area {
            column.paint_matching(side_area, f.buffer_mut(), app.ui_theme.surface_bg);
        }
        column.paint_matching(work_chat_area, f.buffer_mut(), app.ui_theme.surface_bg);
        column.paint_matching(body_chunks[2], f.buffer_mut(), app.ui_theme.surface_bg);
        column.paint_matching(body_chunks[3], f.buffer_mut(), app.ui_theme.surface_bg);
        column.paint_matching(
            body_chunks[composer_slot],
            f.buffer_mut(),
            app.ui_theme.composer_bg,
        );
        column.paint_matching(
            body_chunks[footer_slot],
            f.buffer_mut(),
            app.ui_theme.footer_bg,
        );
    }
    // Toast stack overlay (#439): when multiple status toasts are queued,
    // surface the older ones as a 1-2 line strip above the footer so a
    // burst of events isn't collapsed to a single visible message.
    if classic_shell {
        render_toast_stack_overlay(
            f,
            size,
            body_chunks[composer_slot],
            body_chunks[footer_slot],
            app,
        );
    }

    // Decision card overlay (v0.8.43 truth-surface). When a decision card is
    // active, render it centered on top of the transcript.
    if let Some(ref card) = app.decision_card {
        let card_width = size.width.clamp(30, 60);
        let card_height = card.desired_height(card_width);
        let card_area = ratatui::layout::Rect {
            x: size
                .x
                .saturating_add(size.width.saturating_sub(card_width) / 2),
            y: size
                .y
                .saturating_add(size.height.saturating_sub(card_height) / 2),
            width: card_width,
            height: card_height.min(size.height),
        };
        let buf = f.buffer_mut();
        card.render(card_area, buf);
    }

    if !app.view_stack.is_empty() {
        // The live transcript overlay snapshots the app's history + active
        // cell on each render so streaming mutations propagate. Other views
        // are static and skip this refresh.
        if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
            refresh_live_transcript_overlay(app);
        } else if app.view_stack.top_kind() == Some(ModalKind::ContextInspector) {
            refresh_context_inspector_overlay(app);
        }
        if app.view_stack.top_kind() == Some(ModalKind::Approval) {
            app.viewport.last_approval_area = app.view_stack.top_occupied_region(size);
        }
        let buf = f.buffer_mut();
        app.view_stack.render(size, buf);
    }
}

/// Draw a complete application frame, optionally with a full viewport reset.
///
/// When `full_repaint` is true, the terminal scroll margins and origin mode
/// are reset, the screen is cleared, ratatui's buffer is emptied, and then
/// the full UI is drawn — all within a single DEC 2026 synchronized-update
/// batch so GPU-accelerated terminals (Ghostty, VS Code, Kitty) render one
/// complete frame instead of a blank intermediate frame followed by the UI.
///
/// When `full_repaint` is false, only the diff from the previous draw is
/// written (normal incremental update path).
fn draw_app_frame_inner(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &Config,
    full_repaint: bool,
) -> Result<()> {
    terminal.backend_mut().set_palette_mode(app.ui_theme.mode);
    terminal.backend_mut().set_theme(app.theme_id, app.ui_theme);
    // DEC 2026 wrapping is on by default but can be turned off for
    // terminals that mishandle it (Ptyxis 50.x + VTE 0.84.x flashes the
    // whole viewport on every wrapped frame instead of deferring as the
    // standard requires). Settings::synchronized_output_enabled resolves
    // the user's setting against the Ptyxis env auto-detect.
    let wrap_in_sync_update = app.synchronized_output_enabled;
    if wrap_in_sync_update {
        let _ = terminal.backend_mut().write_all(BEGIN_SYNC_UPDATE);
    }

    // Run fallible draw operations in a closure so END_SYNC_UPDATE is
    // always sent even if an intermediate step fails. Without this, a
    // failing `?` would return early and leave the terminal stuck in
    // synchronized-update mode (screen frozen).
    let result = (|| -> Result<()> {
        if full_repaint {
            terminal.backend_mut().write_all(TERMINAL_ORIGIN_RESET)?;
            terminal.clear()?;
        }
        terminal.draw(|f| render(f, app, config))?;
        Ok(())
    })();

    // Always end the synchronized update, regardless of success or failure.
    if wrap_in_sync_update {
        let _ = terminal.backend_mut().write_all(END_SYNC_UPDATE);
    }
    let _ = terminal.backend_mut().flush();
    result
}

/// Pull the latest snapshot of cells / revisions / render options into the
/// live transcript overlay sitting on top of the view stack. No-op if the
/// top view isn't a `LiveTranscriptOverlay`.
fn refresh_live_transcript_overlay(app: &mut App) {
    // Pop+push lets us hold &mut to the overlay while also borrowing `app`
    // mutably for the snapshot — direct re-borrow through `view_stack`
    // would otherwise alias `app`.
    let Some(mut overlay) = app.view_stack.pop() else {
        return;
    };
    if let Some(typed) = overlay.as_any_mut().downcast_mut::<LiveTranscriptOverlay>() {
        typed.refresh_from_app(app);
    }
    app.view_stack.push_boxed(overlay);
}

fn refresh_context_inspector_overlay(app: &mut App) {
    let Some(mut overlay) = app.view_stack.pop() else {
        return;
    };
    if let Some(typed) = overlay.as_any_mut().downcast_mut::<ContextInspectorView>() {
        typed.refresh_from_app(app);
    }
    app.view_stack.push_boxed(overlay);
}

/// Open the live transcript overlay in backtrack-preview mode (#133).
/// The overlay starts highlighting the most recent user message
/// (`selected_idx = 0`) and routes Left/Right/Enter/Esc through
/// `ViewEvent::Backtrack*` so the main key dispatcher can advance the
/// `BacktrackState` and apply the rewind on confirm.
fn open_backtrack_overlay(app: &mut App) {
    let mut overlay = LiveTranscriptOverlay::new();
    overlay.refresh_from_app(app);
    overlay.set_backtrack_preview(0);
    app.view_stack.push(overlay);
    app.status_message =
        Some("Backtrack: \u{2190}/\u{2192} step  Enter rewind  Esc cancel".to_string());
    app.needs_redraw = true;
}

/// Open a fresh live transcript overlay in sticky-tail mode.
fn open_live_transcript_overlay(app: &mut App) {
    if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
        return;
    }
    let mut overlay = LiveTranscriptOverlay::new();
    overlay.refresh_from_app(app);
    app.view_stack.push(overlay);
    app.status_message = Some("Live transcript: tailing (Esc to close)".to_string());
    app.needs_redraw = true;
}

/// Toggle the live transcript overlay on `Ctrl+Shift+T`. Closes the overlay if it's
/// already on top; otherwise uses the same open path as `/transcript`.
fn toggle_live_transcript_overlay(app: &mut App) {
    if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
        app.view_stack.pop();
        app.needs_redraw = true;
        return;
    }
    open_live_transcript_overlay(app);
}

/// Open the `/model` picker pre-filtered to `provider` (#3083). The model
/// picker's search already scopes rows by provider display name, so we reuse
/// the standard "open model picker" path and seed its query by replaying the
/// provider's display name as character input through the public view-stack
/// key path — no model-picker internals are touched.
fn open_model_picker_for_provider(
    app: &mut App,
    config: &Config,
    provider: crate::config::ApiProvider,
) {
    if app.view_stack.top_kind() != Some(ModalKind::ModelPicker) {
        app.view_stack
            .push(crate::tui::model_picker::ModelPickerView::new(app, config));
    }
    for ch in provider.display_name().chars() {
        // Char input updates the query and never emits a ViewEvent, so the
        // returned (empty) event list is safe to drop.
        let _ = app.view_stack.handle_key(crossterm::event::KeyEvent::new(
            KeyCode::Char(ch),
            KeyModifiers::NONE,
        ));
    }
    app.needs_redraw = true;
}

fn apply_hotbar_setup_saved(
    app: &mut App,
    config: &mut Config,
    bindings: Vec<codewhale_config::HotbarBindingToml>,
) {
    match crate::config_persistence::persist_hotbar_bindings(app.config_path.as_deref(), &bindings)
    {
        Ok(path) => {
            config.hotbar = Some(bindings);
            app.status_message = Some(format!("Hotbar bindings saved to {}", path.display()));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to save Hotbar bindings: {err}"));
            app.add_message(HistoryCell::System {
                content: format!("Failed to save Hotbar bindings: {err}"),
            });
        }
    }
    app.needs_redraw = true;
}

fn record_provider_model_setup_progress(app: &mut App, config: &Config) {
    if let Err(err) = crate::tui::setup::record_provider_model_setup_state_for_app(app, config) {
        let note = format!("Setup provider/model state was not saved: {err}");
        if let Some(status) = app.status_message.as_mut() {
            status.push_str(" · ");
            status.push_str(&note);
        } else {
            app.status_message = Some(note.clone());
        }
        app.add_message(HistoryCell::System { content: note });
    }
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

/// Hide the Hotbar: persist `hotbar = []` (the canonical "disabled" state) and
/// clear the live in-memory slots so the panel disappears immediately. The
/// explicit empty array — not a missing key — is what disables defaults, so we
/// store `Some(vec![])` rather than `None`.
fn disable_hotbar(app: &mut App, config: &mut Config) {
    match crate::config_persistence::persist_hotbar_bindings(app.config_path.as_deref(), &[]) {
        Ok(path) => {
            config.hotbar = Some(Vec::new());
            app.status_message = Some(format!(
                "Hotbar hidden (hotbar = [] in {}). Bring it back with `/hotbar on`.",
                path.display()
            ));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to hide Hotbar: {err}"));
            app.add_message(HistoryCell::System {
                content: format!("Failed to hide Hotbar: {err}"),
            });
        }
    }
    app.needs_redraw = true;
}

/// Show the default recommended Hotbar slots. Since #3807 an absent `hotbar`
/// key means "hidden", so `/hotbar on` persists the explicit default bindings
/// rather than deleting the key. This is an explicit reset, so any custom
/// bindings are replaced with the recommended set.
fn restore_hotbar_defaults(app: &mut App, config: &mut Config) {
    let defaults = codewhale_config::default_hotbar_bindings_toml();
    match crate::config_persistence::persist_hotbar_bindings(app.config_path.as_deref(), &defaults)
    {
        Ok(path) => {
            config.hotbar = Some(defaults);
            app.status_message = Some(format!(
                "Hotbar enabled with the default slots ({}). Customize with `/hotbar`.",
                path.display()
            ));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to enable the Hotbar: {err}"));
            app.add_message(HistoryCell::System {
                content: format!("Failed to enable the Hotbar: {err}"),
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

fn refresh_config_view_if_open(app: &mut App, focus_key: &str) {
    if app.view_stack.top_kind() == Some(ModalKind::Config) {
        let filter = app.view_stack.pop().and_then(|mut view| {
            view.as_any_mut()
                .downcast_mut::<ConfigView>()
                .map(|config_view| config_view.filter_query().to_string())
        });
        let mut config_view = ConfigView::new_for_app(app);
        if let Some(filter) = filter {
            config_view.restore_filter(filter);
        }
        config_view.focus_key(focus_key);
        app.view_stack.push(config_view);
    }
}

fn refresh_skills_manager_if_open(
    app: &mut App,
    status: Option<String>,
    focus: Option<&crate::skills::audit::AuditedSkillId>,
) {
    if app.view_stack.top_kind() != Some(ModalKind::SkillsManager) {
        return;
    }
    let Some(mut boxed) = app.view_stack.pop() else {
        return;
    };
    let rebuilt = if let Some(prev) = boxed
        .as_any_mut()
        .downcast_mut::<crate::tui::views::skills_manager::SkillsManagerView>(
    ) {
        crate::tui::views::skills_manager::SkillsManagerView::rebuild_preserving(
            app, prev, status, focus,
        )
    } else {
        crate::tui::views::skills_manager::SkillsManagerView::new(app)
    };
    app.view_stack.push(rebuilt);
}

async fn handle_skill_mutation_requested(
    app: &mut App,
    request: crate::skills::mutation::SkillMutationRequest,
) {
    use crate::skills::install::{DEFAULT_MAX_SIZE_BYTES, DEFAULT_REGISTRY_URL};
    use crate::skills::mutation::{MutationContext, SkillMutationOutcome, SkillMutationRequest};

    let focus = match &request {
        SkillMutationRequest::ImportExternal { source_id, .. } => Some(source_id.clone()),
        SkillMutationRequest::Update { skill_id, .. }
        | SkillMutationRequest::Remove { skill_id, .. }
        | SkillMutationRequest::Trust { skill_id, .. } => Some(skill_id.clone()),
        SkillMutationRequest::InstallRemote { .. }
        | SkillMutationRequest::UpdateByName { .. }
        | SkillMutationRequest::RemoveByName { .. }
        | SkillMutationRequest::TrustByName { .. } => None,
    };

    let workspace = app.workspace.clone();
    let home = dirs::home_dir();
    let cfg = crate::config::Config::load(None, None).unwrap_or_default();
    let network = cfg
        .network
        .clone()
        .map(|policy| policy.into_runtime())
        .unwrap_or_default();
    let skills_cfg = cfg.skills.as_ref();
    let max_size = skills_cfg
        .and_then(|s| s.max_install_size_bytes)
        .unwrap_or(DEFAULT_MAX_SIZE_BYTES);
    let registry_url = skills_cfg
        .and_then(|s| s.registry_url.clone())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    let skills_dir = app.skills_dir.clone();
    let result = {
        let ctx = MutationContext {
            workspace: &workspace,
            home: home.as_deref(),
            configured_skills_dir: Some(skills_dir.as_path()),
            network: &network,
            max_size,
            registry_url: &registry_url,
        };
        crate::skills::mutation::execute(request, &ctx).await
    };

    let (status, refresh_skills) = match result {
        Ok(receipt) => {
            let msg = match &receipt.outcome {
                SkillMutationOutcome::Installed => {
                    format!(
                        "Installed '{}' → {}",
                        receipt.name, receipt.safe_target_path
                    )
                }
                SkillMutationOutcome::Updated => format!("Updated '{}'", receipt.name),
                SkillMutationOutcome::NoChange => {
                    format!("'{}': no upstream change", receipt.name)
                }
                SkillMutationOutcome::Removed => format!("Removed '{}'", receipt.name),
                SkillMutationOutcome::Trusted => format!("Trusted '{}'", receipt.name),
                SkillMutationOutcome::Imported => {
                    format!("Imported '{}' → {}", receipt.name, receipt.safe_target_path)
                }
                SkillMutationOutcome::AlreadyPresent => {
                    format!("'{}' already present (exact duplicate)", receipt.name)
                }
                SkillMutationOutcome::NeedsApproval(host) => {
                    format!("Needs network approval for {host}")
                }
                SkillMutationOutcome::NetworkDenied(host) => {
                    format!("Network denied for {host}")
                }
            };
            let refresh = !matches!(
                receipt.outcome,
                SkillMutationOutcome::NeedsApproval(_) | SkillMutationOutcome::NetworkDenied(_)
            );
            (msg, refresh)
        }
        Err(err) => (format!("Skill mutation failed: {err:#}"), false),
    };

    app.status_message = Some(status.clone());
    if refresh_skills {
        app.refresh_skill_cache();
    }
    refresh_skills_manager_if_open(app, Some(status), focus.as_ref());
    app.needs_redraw = true;
}

#[allow(clippy::too_many_arguments)]
async fn handle_config_updated(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    task_manager: &SharedTaskManager,
    engine_handle: &mut EngineHandle,
    web_config_session: &mut Option<WebConfigSession>,
    key: String,
    value: String,
    persist: bool,
) -> Result<bool> {
    let result = prepare_config_update_result(
        commands::set_config_value(app, &key, &value, persist),
        persist,
    );
    let normalized_value = value.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    let cleared_root_approval = !result.is_error
        && persist
        && key == "approval_policy"
        && matches!(
            normalized_value.as_str(),
            "default" | "tui-default" | "use-tui-default"
        );
    // Theme / background changes require a full terminal repaint because
    // ratatui's incremental diff cannot see colors remapped by the backend.
    if matches!(
        key.as_str(),
        "theme" | "ui_theme" | "background_color" | "background" | "bg"
    ) {
        app.force_next_full_repaint = true;
    }
    if apply_command_result(
        terminal,
        app,
        engine_handle,
        task_manager,
        config,
        web_config_session,
        result,
    )
    .await?
    {
        return Ok(true);
    }

    let focus_key = if cleared_root_approval {
        "permission_posture"
    } else {
        &key
    };
    refresh_config_view_if_open(app, focus_key);
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
async fn handle_view_events(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    task_manager: &SharedTaskManager,
    engine_handle: &mut EngineHandle,
    web_config_session: &mut Option<WebConfigSession>,
    events: Vec<ViewEvent>,
) -> Result<bool> {
    for event in events {
        match event {
            ViewEvent::CommandPaletteSelected { action } => match action {
                crate::tui::views::CommandPaletteAction::ExecuteCommand { command } => {
                    if execute_command_input(
                        terminal,
                        app,
                        engine_handle,
                        task_manager,
                        config,
                        &mut *web_config_session,
                        &command,
                    )
                    .await?
                    {
                        return Ok(true);
                    }
                }
                crate::tui::views::CommandPaletteAction::InsertText { text } => {
                    app.input = text;
                    app.cursor_position = app.input.chars().count();
                    app.status_message = Some(
                        "Inserted into composer. Finish the input or press Enter.".to_string(),
                    );
                }
                crate::tui::views::CommandPaletteAction::OpenTextPager { title, content } => {
                    open_text_pager(app, title, content);
                }
            },
            ViewEvent::OpenTextPager { title, content } => {
                open_text_pager(app, title, content);
            }
            ViewEvent::CopyToClipboard { text, label } => {
                if text.is_empty() {
                    app.status_message = Some(format!("{label} is empty"));
                } else if app.clipboard.write_text(&text).is_ok() {
                    app.status_message = Some(format!("{label} copied"));
                } else {
                    app.status_message = Some(format!("Copy failed ({label})"));
                }
            }
            ViewEvent::ApprovalDecision {
                tool_id,
                tool_name,
                decision,
                timed_out,
                approval_key,
                approval_grouping_key,
                persistent_ask_rules,
            } => {
                apply_approval_decision(
                    app,
                    engine_handle,
                    config,
                    ApprovalDecisionEvent {
                        tool_id,
                        tool_name,
                        decision,
                        timed_out,
                        approval_key,
                        approval_grouping_key,
                        persistent_ask_rules,
                    },
                )
                .await;

                if timed_out {
                    app.add_message(HistoryCell::System {
                        content: "Approval request timed out - denied".to_string(),
                    });
                }
            }
            ViewEvent::ElevationDecision {
                tool_id,
                tool_name,
                option,
            } => {
                use crate::tui::approval::ElevationOption;
                match option {
                    ElevationOption::Abort => {
                        let _ = engine_handle.deny_tool_call(tool_id).await;
                        app.add_message(HistoryCell::System {
                            content: format!("Sandbox elevation aborted for {tool_name}"),
                        });
                    }
                    ElevationOption::WithNetwork => {
                        app.add_message(HistoryCell::System {
                            content: format!("Retrying {tool_name} with network access enabled"),
                        });
                        let policy = option.to_policy(&app.workspace);
                        let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                    }
                    ElevationOption::WithWriteAccess(_) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Retrying {tool_name} with write access enabled"),
                        });
                        let policy = option.to_policy(&app.workspace);
                        let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                    }
                    ElevationOption::FullAccess => {
                        app.add_message(HistoryCell::System {
                            content: format!("Retrying {tool_name} with full access (no sandbox)"),
                        });
                        let policy = option.to_policy(&app.workspace);
                        let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                    }
                }
            }
            ViewEvent::UserInputSubmitted { tool_id, response } => {
                match engine_handle
                    .submit_user_input(tool_id.clone(), response)
                    .await
                {
                    Ok(()) => {
                        app.pending_user_input_prompt = None;
                    }
                    Err(err) => {
                        tracing::warn!(tool_id = %tool_id, error = %err, "user input submit failed");
                        if let Some((id, request)) = app.pending_user_input_prompt.clone() {
                            app.view_stack.push(UserInputView::new(id, request));
                        }
                        app.push_status_toast(
                            format!("Failed to submit response: {err}"),
                            StatusToastLevel::Error,
                            None,
                        );
                        app.status_message =
                            Some(format!("Failed to submit response: {err} — try again"));
                    }
                }
            }
            ViewEvent::UserInputCancelled { tool_id } => {
                let _ = engine_handle.cancel_user_input(tool_id).await;
                app.add_message(HistoryCell::System {
                    content: "User input cancelled".to_string(),
                });
            }
            ViewEvent::PlanPromptSelected { option } => {
                if app.plan_prompt_pending {
                    app.plan_prompt_pending = false;
                    if let Some(choice) = plan_choice_from_option(option)
                        && let Err(err) =
                            apply_plan_choice(app, config, engine_handle, choice).await
                    {
                        app.status_message = Some(format!("Failed to apply plan selection: {err}"));
                    }
                }
            }
            ViewEvent::PlanPromptDismissed => {
                app.plan_prompt_pending = true;
                app.status_message =
                    Some("Plan prompt closed. Type 1-4 and press Enter to choose.".to_string());
            }
            ViewEvent::SessionSelected { session_id } => {
                let manager = match SessionManager::default_location() {
                    Ok(manager) => manager,
                    Err(err) => {
                        app.status_message =
                            Some(format!("Failed to open sessions directory: {err}"));
                        continue;
                    }
                };

                match manager.load_session(&session_id) {
                    Ok(session) => {
                        let next_config = config.clone();
                        let (recovered, respawn) = match apply_loaded_session_config_snapshot(
                            app,
                            config,
                            &session,
                            next_config,
                            false,
                        ) {
                            Ok(outcome) => outcome,
                            Err(err) => {
                                app.status_message =
                                    Some(format!("Failed to restore session: {err}"));
                                continue;
                            }
                        };
                        sync_runtime_workspace_state(task_manager, app.workspace.clone()).await;
                        if respawn {
                            let _ = engine_handle.send(Op::Shutdown).await;
                            *engine_handle =
                                spawn_tui_engine(build_engine_config(app, config), config);
                        } else {
                            let _ = engine_handle
                                .send(Op::SetModel {
                                    model: app.model.clone(),
                                    mode: app.mode,
                                    route_limits: app.active_route_limits,
                                })
                                .await;
                        }
                        let _ = engine_handle
                            .send(Op::SyncSession {
                                session_id: app.current_session_id.clone(),
                                messages: app.api_messages.clone(),
                                system_prompt: app.system_prompt.clone(),
                                system_prompt_override: false,
                                model: app.model.clone(),
                                workspace: app.workspace.clone(),
                                mode: app.mode,
                            })
                            .await;
                        let _ = engine_handle
                            .send(Op::SetCompaction {
                                config: app.compaction_config(),
                            })
                            .await;
                        if !recovered {
                            app.status_message = Some(format!(
                                "Session loaded (ID: {})",
                                crate::session_manager::truncate_id(&session_id)
                            ));
                        }
                        app.launch.visible = false;
                        app.launch.status = None;
                    }
                    Err(err) => {
                        app.status_message = Some(format!(
                            "Failed to load session {}: {err}",
                            crate::session_manager::truncate_id(&session_id)
                        ));
                    }
                }
            }
            ViewEvent::SessionRenamed { metadata } => {
                let session_id = metadata.id.clone();
                let title = metadata.title.clone();
                let mut work_snapshot_warning = None;
                if apply_picker_session_rename_to_active_app(app, metadata)
                    && let Ok(manager) = SessionManager::default_location()
                {
                    match build_session_snapshot(app, &manager) {
                        Ok(session) => {
                            if let Err(err) = persist_with_pending_work_boundary(
                                app,
                                PersistRequest::SessionSnapshot(session),
                            ) {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %err,
                                    "Could not queue active session rename Work snapshot"
                                );
                                work_snapshot_warning = Some(format!(
                                    "Session renamed, but Work snapshot is pending ({err})"
                                ));
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %err,
                                "Could not queue active session rename snapshot"
                            );
                        }
                    }
                }
                app.status_message = Some(work_snapshot_warning.unwrap_or_else(|| {
                    format!(
                        "Renamed session {} to \"{}\"",
                        crate::session_manager::truncate_id(&session_id),
                        title
                    )
                }));
            }
            ViewEvent::SessionDeleted { session_id, title } => {
                app.status_message = Some(format!(
                    "Deleted session {} ({})",
                    crate::session_manager::truncate_id(&session_id),
                    title
                ));
            }
            ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            } => {
                if handle_config_updated(
                    terminal,
                    app,
                    config,
                    task_manager,
                    engine_handle,
                    web_config_session,
                    key,
                    value,
                    persist,
                )
                .await?
                {
                    return Ok(true);
                }
            }
            ViewEvent::StatusItemsUpdated { items, final_save } => {
                // Apply to the live App immediately so the footer reflects
                // every keystroke (live preview).
                app.status_items = items.clone();
                app.needs_redraw = true;
                if final_save {
                    match crate::config_persistence::persist_status_items(&items) {
                        Ok(path) => {
                            app.status_message =
                                Some(format!("Status line saved to {}", path.display()));
                        }
                        Err(err) => {
                            app.add_message(HistoryCell::System {
                                content: format!("Failed to save status line: {err}"),
                            });
                        }
                    }
                }
            }
            ViewEvent::HotbarSetupSaved { bindings } => {
                apply_hotbar_setup_saved(app, config, bindings);
            }
            ViewEvent::SetupStateCommitRequested { state, message } => match state.save() {
                Ok(()) => {
                    app.status_message = Some(message);
                }
                Err(err) => {
                    app.status_message = Some(format!("Setup state could not be saved: {err}"));
                }
            },
            ViewEvent::SetupConstitutionCommitRequested {
                constitution,
                state,
                message,
            } => match crate::tui::setup::persist_user_constitution_choice(&constitution, &state) {
                Ok(()) => {
                    app.status_message = Some(message);
                }
                Err(err) => {
                    app.status_message =
                        Some(format!("User constitution could not be saved: {err}"));
                }
            },
            ViewEvent::SetupConstitutionModelDraftRequested {
                draft,
                freeform_note,
                locale,
            } => {
                handle_setup_constitution_model_draft(app, config, draft, freeform_note, locale)
                    .await;
            }
            ViewEvent::FleetProfileModelDraftRequested {
                role,
                model,
                provider,
                reasoning_effort,
                locale,
            } => {
                handle_fleet_profile_model_draft(
                    app,
                    config,
                    role,
                    model,
                    provider,
                    reasoning_effort,
                    locale,
                )
                .await;
            }
            ViewEvent::FleetRosterOpenSetupRequested => {
                // The roster view hands off to the authoring wizard (same
                // path as AppAction::OpenFleetSetup).
                if app.view_stack.top_kind() != Some(ModalKind::FleetSetup) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::views::fleet_setup::FleetSetupView::new(
                            app, config,
                        ));
                }
            }
            ViewEvent::FleetRosterOpenWorkersRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::SubAgents) {
                    let agents = subagent_view_agents(app, &app.subagent_cache);
                    app.view_stack
                        .push(super::views::SubAgentsView::new(agents));
                }
                app.status_message =
                    Some(tr(app.ui_locale, MessageId::SubagentsFetching).to_string());
                let _ = engine_handle.try_send(Op::ListSubAgents);
            }
            ViewEvent::FleetProfileDraftCommitRequested { draft, scope } => {
                // The TOML is rendered deterministically from the validated
                // draft and written atomically; the target path is derived
                // from the sanitized id, never model-chosen.
                let profile_dir =
                    match crate::fleet::profile::agent_profile_dir_for_scope(scope, &app.workspace)
                    {
                        Ok(dir) => dir,
                        Err(err) => {
                            app.set_sticky_status(
                                format!("Fleet {} scope is unavailable: {err:#}", scope.label()),
                                StatusToastLevel::Error,
                                None,
                            );
                            app.needs_redraw = true;
                            continue;
                        }
                    };
                let target = profile_dir.join(draft.file_name());
                // A ratified profile must not silently clobber a differently
                // named existing profile that shares this id (which would also
                // make the whole agents dir fail to load on the duplicate).
                // Overwriting the SAME file is fine — that is an intentional
                // re-draft of this profile.
                // The collision gate only needs file identities. Accept
                // otherwise legacy profile fields here so an old, unrelated
                // profile cannot block saving a current one. Malformed TOML,
                // unreadable files, and invalid ids still fail closed because
                // then we cannot prove there is no collision.
                let existing_profiles =
                    crate::fleet::profile::load_agent_profile_identities_from_dir(&profile_dir);
                if let Err(err) = &existing_profiles {
                    let message = tr(app.ui_locale, MessageId::FleetProfileIdentityVerifyFailed)
                        .replace("{error}", &format!("{err:#}"));
                    app.set_sticky_status(message, StatusToastLevel::Error, None);
                    app.needs_redraw = true;
                    continue;
                }
                let id_conflict = existing_profiles
                    .into_iter()
                    .flatten()
                    .find(|p| p.id.eq_ignore_ascii_case(&draft.id) && p.source != target);
                if let Some(existing) = id_conflict {
                    let message = tr(app.ui_locale, MessageId::FleetProfileIdConflict)
                        .replace("{id}", &draft.id)
                        .replace("{path}", &existing.source.display().to_string());
                    app.set_sticky_status(message, StatusToastLevel::Error, None);
                    app.needs_redraw = true;
                    continue;
                }
                // #4093 AC #5: a profile may only pin a provider the operator
                // has actually configured/credentialed. The picker already
                // offers models only from configured providers, but a
                // model-drafted or hand-edited route (or credentials removed
                // after the pick) could still name an unconfigured one — which
                // would fail loudly at launch. Catch it at save time with a
                // clear message, reusing the SAME predicate the picker uses.
                if let Some(provider_id) = draft.provider.as_deref()
                    && let Some(provider) = crate::config::ApiProvider::parse(provider_id)
                    && !crate::config::provider_is_configured_for_active(
                        config,
                        provider,
                        app.api_provider,
                    )
                {
                    let message = tr(app.ui_locale, MessageId::FleetProfileProviderUnconfigured)
                        .replace("{provider}", provider_id)
                        .replace("{env}", &provider.env_vars_label());
                    app.set_sticky_status(message, StatusToastLevel::Error, None);
                    app.needs_redraw = true;
                    continue;
                }
                let mut txn = codewhale_config::persistence::SetupTransaction::new();
                txn.stage(target.clone(), draft.render_toml().into_bytes());
                match txn.commit() {
                    Ok(()) => {
                        let roster = std::sync::Arc::new(crate::fleet::roster::FleetRoster::load(
                            &config.fleet_config(),
                            &app.workspace,
                        ));
                        let roster_refresh_failed = engine_handle
                            .try_send(Op::SetFleetRoster { roster })
                            .is_err();
                        let zh = app.ui_locale == crate::localization::Locale::ZhHans;
                        app.add_message(HistoryCell::System {
                            content: if zh {
                                format!("已保存 Fleet 配置：{}", target.display())
                            } else {
                                format!(
                                    "Fleet {} profile saved: {}",
                                    scope.label(),
                                    target.display()
                                )
                            },
                        });
                        app.status_message = Some(if zh {
                            format!("已保存 Fleet 配置：{}", draft.file_name())
                        } else if roster_refresh_failed {
                            format!(
                                "Fleet {} profile saved, but the live roster could not refresh; restart before dispatching {}",
                                scope.label(),
                                draft.id
                            )
                        } else {
                            format!(
                                "Fleet {} profile saved: {}",
                                scope.label(),
                                draft.file_name()
                            )
                        });
                    }
                    Err(err) => {
                        app.status_message =
                            Some(if app.ui_locale == crate::localization::Locale::ZhHans {
                                format!("无法保存 Fleet 配置：{err:#}")
                            } else {
                                format!("Fleet profile could not be saved: {err:#}")
                            });
                    }
                }
                app.needs_redraw = true;
            }
            ViewEvent::SetupRuntimePresetApplyRequested {
                preset,
                state,
                message,
            } => match apply_setup_runtime_preset(app, config, preset, state) {
                Ok(summary) => {
                    app.status_message = Some(format!("{message} {summary}"));
                }
                Err(err) => {
                    app.status_message =
                        Some(format!("Runtime preset could not be applied: {err:#}"));
                }
            },
            ViewEvent::SetupOpenProviderRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_for_setup(
                            app.api_provider,
                            Some(app.api_provider),
                            config,
                            runtime_status,
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                    app.status_message =
                        Some("Provider setup opened from /setup readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenModelRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::ModelPicker) {
                    open_model_picker_for_provider(app, config, app.api_provider);
                    app.status_message =
                        Some("Model route picker opened from /setup readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenFleetRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetSetup) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::views::fleet_setup::FleetSetupView::new(
                            app, config,
                        ));
                    app.status_message =
                        Some("Fleet setup opened from /setup Operate/Fleet readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenHotbarRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::HotbarSetup) {
                    app.view_stack
                        .push(crate::tui::hotbar::setup::HotbarSetupView::new(app, config));
                    app.status_message =
                        Some("Hotbar setup opened from /setup Hotbar readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenModeRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::ModePicker) {
                    app.view_stack
                        .push(crate::tui::views::mode_picker::ModePickerView::new(
                            app.mode,
                            app.ui_locale,
                        ));
                    app.status_message =
                        Some("Work mode picker opened from /setup runtime posture.".to_string());
                }
            }
            ViewEvent::SetupOpenConfigRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::Config) {
                    app.view_stack.push(ConfigView::new_for_app(app));
                    app.status_message =
                        Some("Config view opened from /setup runtime posture.".to_string());
                }
            }
            ViewEvent::HotbarDisableRequested => {
                disable_hotbar(app, config);
            }
            ViewEvent::SubAgentsRefresh => {
                app.status_message = Some("Refreshing sub-agents...".to_string());
                // #3802: non-blocking send — refresh op, safe to drop.
                let _ = engine_handle.try_send(Op::ListSubAgents);
            }
            ViewEvent::SidebarAgentCancel { agent_id } => {
                app.status_message = Some(format!("Cancelling {agent_id}..."));
                if engine_handle
                    .send(Op::CancelSubAgent {
                        agent_id: agent_id.clone(),
                    })
                    .await
                    .is_err()
                {
                    app.status_message = Some(format!("Could not cancel {agent_id}"));
                }
            }
            ViewEvent::OpenAgentTranscript { agent_id } => {
                if !crate::tui::mouse_ui::open_agent_chat_pager(app, &agent_id) {
                    app.status_message = Some("Exact agent transcript is unavailable".to_string());
                }
                app.needs_redraw = true;
            }
            ViewEvent::AgentDetailsClosed { agent_id } => {
                crate::tui::work_surface::agent_details_closed(app, &agent_id);
            }
            ViewEvent::FilePickerSelected { path } => {
                // Insert `@<path>` at the composer's cursor with surrounding
                // whitespace so the existing `@`-mention parser picks it up.
                let cursor = app.cursor_position;
                let needs_leading_space = cursor > 0
                    && !app
                        .input
                        .chars()
                        .nth(cursor.saturating_sub(1))
                        .is_some_and(|c| c.is_whitespace());
                let mut insertion = String::new();
                if needs_leading_space {
                    insertion.push(' ');
                }
                insertion.push('@');
                insertion.push_str(&path);
                insertion.push(' ');
                app.insert_str(&insertion);
                app.status_message = Some(format!("Attached @{path}"));
            }
            ViewEvent::ModelPickerApplied {
                model,
                provider,
                provider_id,
                effort,
                previous_model,
                previous_effort,
            } => {
                apply_model_picker_choice(
                    app,
                    engine_handle,
                    config,
                    model,
                    provider,
                    provider_id,
                    effort,
                    previous_model,
                    previous_effort,
                )
                .await;
            }
            ViewEvent::ModelPickerDismissed {
                catalog_view,
                view,
                selected_row_id,
            } => {
                sync_config_provider_from_app(config, app);
                app.model_picker_memory = Some(crate::tui::app::ModelPickerMemory {
                    catalog_view,
                    view: Some(view),
                    selected_row_id,
                });
            }
            ViewEvent::ProviderPickerDismissed {
                catalog_view,
                selected_provider_id,
            } => {
                let onboarding_provider_picker = app.onboarding == OnboardingState::Provider;
                // A picker preview must never become route authority. During
                // onboarding Esc is deliberately non-mutating: it returns to
                // Language without touching config or the onboarding marker.
                if !onboarding_provider_picker {
                    sync_config_provider_from_app(config, app);
                }
                app.provider_picker_memory = Some(crate::tui::app::ProviderPickerMemory {
                    catalog_view,
                    selected_provider_id,
                });
                if onboarding_provider_picker {
                    back_from_provider_onboarding(app);
                }
            }
            ViewEvent::ProviderPickerApplied {
                provider,
                provider_id,
            } => {
                if let Some(provider_id) = provider_id {
                    set_active_custom_provider_in_memory(config, &provider_id);
                }
                let model_override = provider_picker_model_override(app, config, provider);
                let switched =
                    switch_provider(app, engine_handle, config, provider, model_override).await;
                if switched && app.onboarding == OnboardingState::Provider {
                    complete_provider_picker_onboarding(app, provider);
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerApiKeySubmitted {
                provider,
                provider_id,
                api_key,
            } => {
                let identity = picker_provider_identity(config, provider, provider_id.as_deref())
                    .map_err(anyhow::Error::msg)?;
                apply_provider_picker_api_key(app, engine_handle, config, identity, api_key).await;
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerSetupConfirmed {
                provider,
                provider_id,
                api_key,
                model,
            } => {
                let identity = picker_provider_identity(config, provider, provider_id.as_deref())
                    .map_err(anyhow::Error::msg)?;
                let completed = apply_provider_picker_setup_confirmed(
                    app,
                    engine_handle,
                    config,
                    identity,
                    api_key,
                    model,
                )
                .await;
                if completed && app.onboarding == OnboardingState::Provider {
                    complete_provider_picker_onboarding(app, provider);
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerCustomProviderSubmitted {
                provider_id,
                base_url,
                model,
                api_key_env,
            } => {
                apply_provider_picker_custom_provider(
                    app,
                    engine_handle,
                    config,
                    provider_id,
                    base_url,
                    model,
                    api_key_env,
                )
                .await;
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerXaiOAuthRequested => {
                run_xai_device_login_from_tui(terminal, app, engine_handle, config).await?;
            }
            ViewEvent::ProviderPickerExternalConsentConfirmed {
                provider,
                consent_provider,
                source,
                path,
            } => match persist_external_credential_consent_for_at(
                app.config_path.as_deref(),
                config,
                provider,
                consent_provider,
                source,
                &path,
            ) {
                Ok(_) => {
                    let toast = app
                        .tr(MessageId::ProviderExternalGrantedToast)
                        .replace("{owner}", source.owner_label())
                        .replace("{provider}", provider.as_str());
                    app.push_status_toast(toast, StatusToastLevel::Success, Some(8_000));
                    let model_override = provider_picker_model_override(app, config, provider);
                    switch_provider(app, engine_handle, config, provider, model_override).await;
                    refresh_config_view_if_open(app, "provider");
                }
                Err(error) => app.push_status_toast(
                    app.tr(MessageId::ProviderExternalSaveFailedToast)
                        .replace("{error}", &error.to_string()),
                    StatusToastLevel::Error,
                    None,
                ),
            },
            ViewEvent::ProviderPickerExternalConsentRevoked { provider } => {
                match revoke_external_credential_consent_for_at(
                    app.config_path.as_deref(),
                    config,
                    provider,
                ) {
                    Ok(_) => app.push_status_toast(
                        app.tr(MessageId::ProviderExternalRevokedToast)
                            .replace("{provider}", provider.as_str()),
                        StatusToastLevel::Success,
                        Some(5_000),
                    ),
                    Err(error) => app.push_status_toast(
                        app.tr(MessageId::ProviderExternalRevokeFailedToast)
                            .replace("{error}", &error.to_string()),
                        StatusToastLevel::Error,
                        None,
                    ),
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerOpenModels {
                provider,
                provider_id,
            } => {
                if let Some(provider_id) = provider_id {
                    set_active_custom_provider_in_memory(config, &provider_id);
                }
                open_model_picker_for_provider(app, config, provider);
            }
            ViewEvent::ModeSelected { mode } => {
                let prior_mode = app.mode;
                let msg = commands::switch_mode(app, mode);
                if app.mode != prior_mode {
                    sync_mode_update(app, engine_handle).await;
                }
                app.add_message(HistoryCell::System { content: msg });
            }
            ViewEvent::BacktrackStep { direction } => {
                app.backtrack.step(direction);
                if let Some(idx) = app.backtrack.selected_idx() {
                    update_backtrack_overlay_selection(app, idx);
                }
            }
            ViewEvent::BacktrackConfirm => {
                if let Some(depth) = app.backtrack.confirm() {
                    apply_backtrack(app, depth);
                    let _ = engine_handle
                        .send(Op::SyncSession {
                            session_id: app.current_session_id.clone(),
                            messages: app.api_messages.clone(),
                            system_prompt: app.system_prompt.clone(),
                            system_prompt_override: false,
                            model: app.model.clone(),
                            workspace: app.workspace.clone(),
                            mode: app.mode,
                        })
                        .await;
                }
            }
            ViewEvent::BacktrackCancel => {
                app.backtrack.reset();
                app.status_message = Some("Backtrack canceled".to_string());
                app.needs_redraw = true;
            }
            ViewEvent::ContextMenuSelected {
                action: ContextMenuAction::ExecuteCommand { command },
            } => {
                if execute_command_input(
                    terminal,
                    app,
                    engine_handle,
                    task_manager,
                    config,
                    &mut *web_config_session,
                    &command,
                )
                .await?
                {
                    return Ok(true);
                }
            }
            ViewEvent::ContextMenuSelected { action } => handle_context_menu_action(app, action),
            ViewEvent::SkillMutationRequested { request } => {
                handle_skill_mutation_requested(app, request).await;
            }
            ViewEvent::SkillsManagerToggleCompatible => {
                if app.view_stack.top_kind() == Some(ModalKind::SkillsManager)
                    && let Some(mut boxed) = app.view_stack.pop()
                {
                    if let Some(view) = boxed
                        .as_any_mut()
                        .downcast_mut::<crate::tui::views::skills_manager::SkillsManagerView>(
                    ) {
                        crate::tui::views::skills_manager::apply_toggle_compatible(view, app);
                    }
                    app.view_stack.push_boxed(boxed);
                }
            }
        }
    }

    Ok(false)
}

/// Keep the very large modal-event dispatcher out of the already-large TUI
/// loop future. Config previews take a dedicated small path: polling the full
/// dispatcher on top of the event loop exceeds the macOS main-thread stack in
/// debug builds before a theme preview can reach its next frame.
#[allow(clippy::too_many_arguments)]
fn handle_view_events_boxed<'a>(
    terminal: &'a mut AppTerminal,
    app: &'a mut App,
    config: &'a mut Config,
    task_manager: &'a SharedTaskManager,
    engine_handle: &'a mut EngineHandle,
    web_config_session: &'a mut Option<WebConfigSession>,
    events: Vec<ViewEvent>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool>> + 'a>> {
    Box::pin(async move {
        for event in events {
            match event {
                ViewEvent::ConfigUpdated {
                    key,
                    value,
                    persist,
                } => {
                    if handle_config_updated(
                        terminal,
                        app,
                        config,
                        task_manager,
                        engine_handle,
                        web_config_session,
                        key,
                        value,
                        persist,
                    )
                    .await?
                    {
                        return Ok(true);
                    }
                }
                other => {
                    if Box::pin(handle_view_events(
                        terminal,
                        app,
                        config,
                        task_manager,
                        engine_handle,
                        web_config_session,
                        vec![other],
                    ))
                    .await?
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    })
}

fn push_approval_request_view(
    app: &mut App,
    id: &str,
    tool_name: &str,
    description: &str,
    tool_input: &serde_json::Value,
    approval_key: &str,
    intent_summary: Option<&str>,
) {
    let request = ApprovalRequest::new_with_intent(
        id,
        tool_name,
        description,
        tool_input,
        approval_key,
        intent_summary,
        &app.workspace,
    );
    app.view_stack
        .push(ApprovalView::new_for_locale(request, app.ui_locale));
}

struct ApprovalDecisionEvent {
    tool_id: String,
    tool_name: String,
    decision: ReviewDecision,
    timed_out: bool,
    approval_key: String,
    approval_grouping_key: String,
    persistent_ask_rules: Vec<codewhale_config::ToolAskRule>,
}

async fn apply_approval_decision(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    event: ApprovalDecisionEvent,
) {
    if event.decision == ReviewDecision::ApprovedForSession {
        // Store the tool name (backward compat) and the lossy grouping key so
        // later flag variants of the same command family are also auto-approved
        // (v0.8.37).
        app.approval_session_approved
            .insert(event.tool_name.clone());
        app.approval_session_approved
            .insert(event.approval_grouping_key.clone());
    }

    if matches!(
        event.decision,
        ReviewDecision::Approved | ReviewDecision::ApprovedForSession
    ) && !event.persistent_ask_rules.is_empty()
        && !event.timed_out
    {
        persist_ask_rules_from_approval(app, config, &event.persistent_ask_rules);
    }

    match event.decision {
        ReviewDecision::Approved | ReviewDecision::ApprovedForSession => {
            let _ = engine_handle.approve_tool_call(event.tool_id).await;
        }
        ReviewDecision::Denied => {
            // Cache the denial so the model retry-loop doesn't re-prompt for
            // the exact same approval_key (#360). Only the key (per-call
            // unique) is stored — NOT the tool_name, which would block all
            // future invocations of the same tool type (#1377).
            if !event.timed_out {
                app.approval_session_denied.insert(event.approval_key);
            }
            let _ = engine_handle.deny_tool_call(event.tool_id).await;
        }
        ReviewDecision::Abort => {
            engine_handle.cancel();
            mark_active_turn_cancelled_locally(app);
            app.status_message = Some(parent_stop_status(app, "Request cancelled"));
        }
    }
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

fn apply_setup_runtime_preset(
    app: &mut App,
    config: &mut Config,
    preset: crate::tui::setup::SetupRuntimePreset,
    state: codewhale_config::SetupState,
) -> Result<String> {
    if let Some(source) = config.runtime_preset_blocker(
        app.config_path.as_deref(),
        app.config_profile.as_deref(),
        &app.workspace,
    ) {
        anyhow::bail!(
            "Runtime presets cannot override {source}; change that controlling source first"
        );
    }
    if preset == crate::tui::setup::SetupRuntimePreset::HighTrustLocal {
        let approval = config.approval_policy_control(
            app.config_path.as_deref(),
            app.config_profile.as_deref(),
            &app.workspace,
        );
        if !approval.editable_root() {
            anyhow::bail!(
                "Full Access cannot override {}; change that controlling source first",
                approval.label()
            );
        }
    }

    let settings_path = Settings::path().context("failed to resolve settings path")?;
    let settings_snapshot = RuntimePresetFileSnapshot::capture(settings_path)?;
    let mut settings = Settings::load_persisted().context("failed to load settings")?;
    settings.default_mode = preset.default_mode().to_string();
    settings.permission_posture = Some(preset.permission_posture().to_string());

    // Persist into the same file Config::load actually selected. When an env
    // override names a missing file, reads intentionally fall back to an
    // existing home config; writing to the missing override would otherwise
    // leave the controlling key untouched and shadow the home config on the
    // next launch.
    let selected_config_path = crate::config::resolve_load_config_path(app.config_path.clone())
        .or_else(|| app.config_path.clone());
    let config_path = crate::config_persistence::config_toml_path(selected_config_path.as_deref())
        .context("failed to resolve config path")?;
    let config_snapshot = RuntimePresetFileSnapshot::capture(config_path.clone())?;
    if let Err(error) =
        crate::config_persistence::mutate_config_document(&config_path, |document| {
            if let Some(policy) = preset.approval_policy() {
                crate::config_persistence::set_document_value(
                    document,
                    &["approval_policy"],
                    policy,
                )?;
            } else {
                crate::config_persistence::unset_document_value(document, &["approval_policy"])?;
            }
            crate::config_persistence::set_document_value(
                document,
                &["allow_shell"],
                preset.allow_shell(),
            )?;
            crate::config_persistence::set_document_value(
                document,
                &["sandbox_mode"],
                preset.sandbox_mode(),
            )
        })
        .context("failed to persist runtime posture")
    {
        return Err(runtime_preset_error_with_rollback(
            error,
            &[&settings_snapshot, &config_snapshot],
        ));
    }
    if let Err(error) = settings.save().context("failed to save settings") {
        return Err(runtime_preset_error_with_rollback(
            error,
            &[&settings_snapshot, &config_snapshot],
        ));
    }
    if let Err(error) = state
        .save()
        .context("failed to persist setup runtime posture state")
    {
        return Err(runtime_preset_error_with_rollback(
            error,
            &[&settings_snapshot, &config_snapshot],
        ));
    }

    // Durable writes succeeded as one transaction. Only now may live state
    // move to the new posture.
    if let Some(policy) = preset.approval_policy() {
        config.approval_policy = Some(policy.to_string());
        app.mark_approval_policy_locked();
    } else {
        config.approval_policy = None;
        app.clear_saved_approval_policy_lock();
    }
    config.allow_shell = Some(preset.allow_shell());
    config.sandbox_mode = Some(preset.sandbox_mode().to_string());

    let approval_mode = ApprovalMode::from_config_value(
        preset
            .approval_policy()
            .unwrap_or(preset.permission_posture()),
    )
    .unwrap_or(ApprovalMode::Suggest);
    let trust_mode = match preset {
        crate::tui::setup::SetupRuntimePreset::AskFirst => false,
        crate::tui::setup::SetupRuntimePreset::NormalAgent => app.agent_trust_baseline(),
        crate::tui::setup::SetupRuntimePreset::HighTrustLocal => true,
    };
    app.set_agent_runtime_baseline(preset.allow_shell(), trust_mode, approval_mode);
    let mode = AppMode::from_setting(preset.default_mode());
    app.set_mode(mode);
    app.needs_redraw = true;

    Ok(format!("Applied {}.", preset.result_summary()))
}

fn persist_ask_rules_from_approval(
    app: &mut App,
    config: &mut Config,
    rules: &[codewhale_config::ToolAskRule],
) {
    match codewhale_config::ConfigStore::load(app.config_path.clone()).and_then(|mut store| {
        let added = store.append_ask_rules(rules)?;
        let permissions_path = store.permissions_path();
        config.exec_policy_engine = store.exec_policy_engine();
        Ok((added, permissions_path))
    }) {
        Ok((added, path)) if added > 0 => {
            app.status_message = Some(format!(
                "Saved {added} ask permission rule(s) to {}",
                path.display()
            ));
        }
        Ok((_added, path)) => {
            app.status_message = Some(format!(
                "Ask permission rule already saved in {}",
                path.display()
            ));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to save ask permission rule: {err:#}"));
        }
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

/// Push the new `selected_idx` into the live transcript overlay so the
/// highlight follows the user's Left/Right input. No-op if the overlay is
/// no longer on top (e.g. it was closed underneath us).
fn update_backtrack_overlay_selection(app: &mut App, selected_idx: usize) {
    if app.view_stack.top_kind() != Some(ModalKind::LiveTranscript) {
        return;
    }
    let Some(mut overlay) = app.view_stack.pop() else {
        return;
    };
    if let Some(typed) = overlay.as_any_mut().downcast_mut::<LiveTranscriptOverlay>() {
        typed.set_backtrack_preview(selected_idx);
    }
    app.view_stack.push_boxed(overlay);
    app.needs_redraw = true;
}

/// Count how many `HistoryCell::User` entries currently live in the
/// transcript. Used by the backtrack state machine to decide whether
/// there's anything to rewind to. Walks `app.history` directly so it
/// stays accurate even mid-stream (the streaming Assistant cell never
/// counts as a user turn).
fn count_user_history_cells(app: &App) -> usize {
    app.history
        .iter()
        .filter(|cell| matches!(cell, HistoryCell::User { .. }))
        .count()
}

/// Find the absolute index of the Nth-from-tail `HistoryCell::User` in
/// `app.history`. `depth` of 0 selects the most recent user cell.
/// Returns `None` if `depth` is out of range.
fn find_user_cell_index_from_tail(app: &App, depth: usize) -> Option<usize> {
    let mut count = 0usize;
    for (idx, cell) in app.history.iter().enumerate().rev() {
        if matches!(cell, HistoryCell::User { .. }) {
            if count == depth {
                return Some(idx);
            }
            count += 1;
        }
    }
    None
}

/// Apply the user's backtrack selection: trim `app.history` and
/// `app.api_messages` so everything from the chosen user message onward
/// is dropped, populate the composer with the dropped user text, close
/// the overlay, and surface a status hint. The cycle counter is bumped
/// so any persistent indices clear; the engine's in-flight context is
/// re-synced via `Op::SyncSession` so the next turn starts fresh.
/// Index in `api_messages` to truncate to for a backtrack of `depth` visible
/// user prompts from the tail. Counts only messages that yield a
/// `HistoryCell::User` (a real prompt), NOT tool-result messages which are
/// also stored with `role == "user"`. Returns `None` if fewer than `depth`
/// user prompts exist.
fn backtrack_api_cut_index(api_messages: &[Message], depth: usize) -> Option<usize> {
    let mut user_seen = 0usize;
    for (idx, msg) in api_messages.iter().enumerate().rev() {
        let yields_user = history_cells_from_message(msg)
            .iter()
            .any(|cell| matches!(cell, HistoryCell::User { .. }));
        if yields_user {
            if user_seen == depth {
                return Some(idx);
            }
            user_seen += 1;
        }
    }
    None
}

fn apply_backtrack(app: &mut App, depth: usize) {
    let Some(history_idx) = find_user_cell_index_from_tail(app, depth) else {
        app.status_message = Some("Backtrack target no longer present".to_string());
        return;
    };

    // Snapshot the user text before truncating so we can refill the
    // composer.
    let user_text = match app.history.get(history_idx) {
        Some(HistoryCell::User { content }) => content.clone(),
        _ => String::new(),
    };

    // Trim the visible transcript at the chosen user cell. Per-cell
    // revisions and tool-cell maps are kept consistent through
    // `App::truncate_history_to`.
    app.truncate_history_to(history_idx);

    // Trim the API-message log at the matching user PROMPT. `depth` counts
    // visible `HistoryCell::User` cells (real prompts), but a naive
    // `role == "user"` walk over `api_messages` over-counts: tool results are
    // stored as `role == "user"` messages too, so in any turn with tool calls
    // the cut would land mid-turn on a tool_result — leaving a dangling
    // assistant tool_use with no matching result and a transcript the provider
    // rejects. Count only messages that actually yield a User cell, the same
    // predicate `apply_loaded_session` uses.
    if let Some(idx) = backtrack_api_cut_index(&app.api_messages, depth) {
        app.api_messages.truncate(idx);
    }

    // Hand the dropped text back to the user so they can edit + resend.
    app.input = user_text;
    app.cursor_position = app.input.chars().count();

    // Close the overlay, refresh sticky-tail flag, and surface a hint.
    if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
        app.view_stack.pop();
    }
    app.status_message =
        Some("Rewound to previous user message — edit and Enter to resend".to_string());
    app.scroll_to_bottom();
    app.mark_history_updated();
    app.needs_redraw = true;
}

/// Persist the typed API key to `~/.codewhale/config.toml`, refresh the
/// in-memory config so the engine can see it, then switch to the provider.
fn set_active_custom_provider_in_memory(config: &mut Config, provider_id: &str) {
    let provider_id = provider_id.trim();
    if provider_id.is_empty() {
        return;
    }
    config.provider = Some(provider_id.to_string());
    config
        .providers
        .get_or_insert_with(ProvidersConfig::default)
        .custom
        .entry(provider_id.to_string())
        .or_default();
}

fn picker_provider_identity(
    config: &Config,
    provider: ApiProvider,
    provider_id: Option<&str>,
) -> Result<crate::config::ProviderIdentity, String> {
    let identity = match provider_id {
        Some(provider_id) => config
            .resolve_persisted_provider_identity(Some(provider.as_str()), Some(provider_id))?,
        None if provider == ApiProvider::Custom => config.active_provider_identity(provider)?,
        None => config.resolve_persisted_provider_identity(
            Some(provider.as_str()),
            Some(provider.as_str()),
        )?,
    };
    if identity.provider != provider {
        return Err(format!(
            "provider picker identity '{}' resolved as {}, not {}",
            identity.key,
            identity.provider.as_str(),
            provider.as_str()
        ));
    }
    Ok(identity)
}

async fn apply_provider_picker_custom_provider(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    provider_id: String,
    base_url: String,
    model: Option<String>,
    api_key_env: Option<String>,
) {
    let written = match crate::config_persistence::persist_custom_provider(
        app.config_path.as_deref(),
        &provider_id,
        &base_url,
        model.as_deref(),
        api_key_env.as_deref(),
    ) {
        Ok(path) => path,
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!("Failed to save custom provider {provider_id}: {err}"),
            });
            app.status_message = Some("Custom provider was not saved.".to_string());
            return;
        }
    };

    config.provider = Some(provider_id.clone());
    let entry = config
        .providers
        .get_or_insert_with(ProvidersConfig::default)
        .custom
        .entry(provider_id.clone())
        .or_default();
    entry.kind = Some("openai-compatible".to_string());
    entry.base_url = Some(base_url.trim().trim_end_matches('/').to_string());
    entry.model = model.clone().and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    });
    entry.api_key_env = api_key_env.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    });

    app.status_message = Some(format!(
        "Custom provider {provider_id} saved to {}",
        written.display()
    ));
    switch_provider(app, engine_handle, config, ApiProvider::Custom, model).await;
}

async fn apply_provider_picker_api_key(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    api_key: String,
) {
    apply_provider_picker_api_key_with_verifier(
        app,
        engine_handle,
        config,
        identity,
        api_key,
        &LiveProviderKeyVerifier,
    )
    .await;
}

type ProviderKeyVerification<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

trait ProviderKeyVerifier {
    fn verify<'a>(
        &'a self,
        provider: ApiProvider,
        api_key: &'a str,
        base_url: &'a str,
    ) -> ProviderKeyVerification<'a>;
}

struct LiveProviderKeyVerifier;

#[cfg(test)]
fn provider_verification_error_category(reason: &str) -> crate::error_taxonomy::ErrorCategory {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("http 401") || lower.contains("status 401") {
        crate::error_taxonomy::ErrorCategory::Authentication
    } else if lower.contains("http 403") || lower.contains("status 403") {
        crate::error_taxonomy::ErrorCategory::Authorization
    } else if ["500", "502", "503", "504"]
        .iter()
        .any(|status| lower.contains(&format!("http {status}")))
    {
        crate::error_taxonomy::ErrorCategory::Network
    } else {
        crate::error_taxonomy::classify_error_message(reason)
    }
}

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

async fn apply_provider_picker_api_key_with_verifier(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    api_key: String,
    verifier: &dyn ProviderKeyVerifier,
) {
    let provider = identity.provider;
    let mut scoped_config = config.clone();
    scoped_config.provider = Some(identity.key.clone());
    // #3875: verify the key against the provider before opening the rest of
    // the guided flow. Nothing is persisted until the confirm stage.
    // Resolve the effective route, including compatibility routes whose
    // endpoint is selected by auth mode (notably a legacy Kimi CLI import).
    // This prevents a replacement Kimi Code API key from being probed against
    // the ordinary Moonshot endpoint.
    let base_url = scoped_config.deepseek_base_url();
    match verifier.verify(provider, &api_key, &base_url).await {
        Ok(()) => {
            // Key is valid — continue the guided flow at model pick without
            // writing the secret yet.
            let runtime_status = query_provider_runtime_status(engine_handle).await;
            if let Some(picker) =
                crate::tui::provider_picker::ProviderPickerView::new_for_model_pick_after_validation(
                    app.api_provider,
                    provider,
                    &scoped_config,
                    runtime_status,
                    api_key,
                )
                .map(|picker| {
                    picker
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health)
                })
            {
                app.view_stack.push(picker);
                app.status_message = Some(format!(
                    "{} API key verified — pick a default model.",
                    provider.as_str()
                ));
            } else {
                app.status_message = Some(format!(
                    "{} API key verified, but the guided setup could not be re-opened.",
                    provider.as_str()
                ));
            }
            app.needs_redraw = true;
        }
        Err(reason) => {
            // Verification failed - keep the picker open at the key-entry
            // stage with the provider's actual error so the user can fix
            // the key instead of dead-ending with a status toast.
            let runtime_status = query_provider_runtime_status(engine_handle).await;
            if let Some(picker) =
                crate::tui::provider_picker::ProviderPickerView::new_for_key_entry_with_error(
                    app.api_provider,
                    provider,
                    &scoped_config,
                    runtime_status,
                    reason,
                )
                .map(|picker| {
                    picker
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health)
                })
            {
                app.view_stack.push(picker);
                app.status_message = Some(format!(
                    "{} API key verification failed - check the key and try again.",
                    provider.as_str()
                ));
            } else {
                app.status_message = Some(format!(
                    "{} API key verification failed, but the provider could not be re-opened.",
                    provider.as_str()
                ));
            }
            app.needs_redraw = true;
        }
    }
}

async fn apply_provider_picker_setup_confirmed(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    api_key: String,
    model: String,
) -> bool {
    use crate::config::{save_api_key_for_identity, save_provider_model_for_identity};

    let provider = identity.provider;

    let model = model.trim().to_string();
    if model.is_empty() {
        app.add_message(HistoryCell::System {
            content: format!(
                "Cannot finish {} setup: default model is empty.\nProvider unchanged.",
                provider.as_str()
            ),
        });
        return false;
    }

    // Persist key first via the existing comment-preserving path, then pin the
    // chosen default model on the same document when the provider uses a
    // `[providers.<name>]` table.
    match save_api_key_for_identity(&identity, config, &api_key) {
        Ok(path) => {
            if let Err(err) = save_provider_model_for_identity(&identity, config, &model) {
                app.add_message(HistoryCell::System {
                    content: format!(
                        "Saved {} API key to {}, but failed to pin model `{model}`: {err}",
                        provider.as_str(),
                        path.display()
                    ),
                });
            } else {
                app.status_message = Some(format!(
                    "Saved {} API key and model to {}",
                    provider.as_str(),
                    path.display()
                ));
            }
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to save {} API key: {err}\nProvider unchanged.",
                    provider.as_str()
                ),
            });
            return false;
        }
    }

    config.provider = Some(identity.key);
    mirror_saved_api_key_in_config(config, provider, api_key);
    mirror_saved_model_in_config(config, provider, model.clone());
    switch_provider(app, engine_handle, config, provider, Some(model)).await
}

fn mirror_saved_model_in_config(config: &mut Config, provider: ApiProvider, model: String) {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        config.default_text_model = Some(model);
        return;
    }
    config.set_provider_model_override(provider, Some(model));
}

fn mirror_saved_api_key_in_config(config: &mut Config, provider: ApiProvider, api_key: String) {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        config.api_key = Some(api_key);
        config.auth_mode = Some("api_key".to_string());
        return;
    }
    if provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route() {
        config.api_key = Some(api_key);
        config.auth_mode = Some("api_key".to_string());
        return;
    }
    let pin_kimi_code_base_url = provider == ApiProvider::Moonshot
        && config.provider_config_for(provider).is_some_and(|entry| {
            crate::config::provider_config_uses_kimi_imported_token(entry)
                && entry
                    .base_url
                    .as_deref()
                    .is_none_or(|base_url| base_url.trim().is_empty())
        });
    let custom_key = (provider == ApiProvider::Custom).then(|| {
        config
            .provider
            .clone()
            .unwrap_or_else(|| "__custom__".to_string())
    });
    let providers = config
        .providers
        .get_or_insert_with(ProvidersConfig::default);
    let entry: &mut ProviderConfig = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => return,
        ApiProvider::Custom => providers
            .custom
            .entry(custom_key.expect("custom key captured for custom provider"))
            .or_default(),
        ApiProvider::DeepseekAnthropic => &mut providers.deepseek_anthropic,
        ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
        ApiProvider::Openai => &mut providers.openai,
        ApiProvider::Atlascloud => &mut providers.atlascloud,
        ApiProvider::WanjieArk => &mut providers.wanjie_ark,
        ApiProvider::Volcengine => &mut providers.volcengine,
        ApiProvider::Openrouter => &mut providers.openrouter,
        ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
        ApiProvider::Novita => &mut providers.novita,
        ApiProvider::Fireworks => &mut providers.fireworks,
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => &mut providers.siliconflow,
        ApiProvider::Arcee => &mut providers.arcee,
        ApiProvider::Moonshot => &mut providers.moonshot,
        ApiProvider::Sglang => &mut providers.sglang,
        ApiProvider::Vllm => &mut providers.vllm,
        ApiProvider::Ollama => &mut providers.ollama,
        ApiProvider::Huggingface => &mut providers.huggingface,
        ApiProvider::Deepinfra => &mut providers.deepinfra,
        ApiProvider::Together => &mut providers.together,
        ApiProvider::Qianfan => &mut providers.qianfan,
        ApiProvider::OpenaiCodex => &mut providers.openai_codex,
        ApiProvider::Anthropic => &mut providers.anthropic,
        ApiProvider::Openmodel => &mut providers.openmodel,
        ApiProvider::Zai => &mut providers.zai,
        ApiProvider::Stepfun => &mut providers.stepfun,
        ApiProvider::Minimax => &mut providers.minimax,
        ApiProvider::MinimaxAnthropic => &mut providers.minimax_anthropic,
        ApiProvider::Sakana => &mut providers.sakana,
        ApiProvider::LongCat => &mut providers.longcat,
        ApiProvider::OpencodeGo => &mut providers.opencode_go,
        ApiProvider::Meta => &mut providers.meta,
        ApiProvider::Xai => &mut providers.xai,
        ApiProvider::Telecomjs => &mut providers.telecomjs,
    };
    if pin_kimi_code_base_url {
        entry.base_url = Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string());
    }
    entry.auth_mode = Some("api_key".to_string());
    entry.api_key = Some(api_key);
    entry.external_credentials = None;
    if provider == ApiProvider::Xai {
        entry.oauth_credential_generation = None;
    }
}

async fn apply_codewhale_owned_xai_login(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    pending: crate::xai_oauth::PendingXaiDeviceLogin,
    status_prefix: &str,
) {
    match crate::xai_oauth::activate_device_login(
        pending,
        app.config_path.as_deref(),
        Some(&mut *config),
    ) {
        Ok(activation) => {
            app.status_message = Some(format!(
                "{status_prefix}; activated {} via {}",
                codewhale_config::quote_os_path(&activation.auth_path),
                codewhale_config::quote_os_path(&activation.config_path)
            ));
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to finalize {} device login: {err}\nProvider unchanged.",
                    ApiProvider::Xai.as_str()
                ),
            });
            return;
        }
    }

    switch_provider(app, engine_handle, config, ApiProvider::Xai, None).await;
}

async fn run_xai_device_login_from_tui(
    terminal: &mut AppTerminal,
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
) -> Result<()> {
    pause_terminal(
        terminal,
        app.use_alt_screen,
        app.use_mouse_capture,
        app.use_bracketed_paste,
    )?;
    let login_result = crate::xai_oauth::device_code_login().await;
    resume_terminal(
        terminal,
        app.use_alt_screen,
        app.use_mouse_capture,
        app.use_bracketed_paste,
        app.synchronized_output_enabled,
    )?;

    match login_result {
        Ok(pending) => {
            apply_codewhale_owned_xai_login(
                app,
                engine_handle,
                config,
                pending,
                "xAI device login complete",
            )
            .await;
        }
        Err(err) => {
            let message = format!("xAI device login failed: {err}");
            app.add_message(HistoryCell::System {
                content: message.clone(),
            });
            app.status_message = Some(message);
        }
    }
    app.needs_redraw = true;
    Ok(())
}

fn apply_loaded_session(
    app: &mut App,
    config: &mut Config,
    session: &SavedSession,
) -> Result<bool, String> {
    if app.session_transition_blocked() {
        return Err(
            "runtime work is active; wait for the current turn, maintenance, and background tasks to finish, or cancel that specific work before switching sessions".to_string(),
        );
    }
    let provider_identity = config.resolve_persisted_provider_identity(
        Some(&session.metadata.model_provider),
        session.metadata.model_provider_id.as_deref(),
    )?;
    let restored_route = resolve_runtime_route_for_identity(
        config,
        &provider_identity,
        Some(&session.metadata.model),
    )
    .map_err(|reason| {
        format!(
            "saved session provider '{}' could not be resolved from the live config: {reason}. Codewhale will not fall back",
            provider_identity.key
        )
    })?;
    // Restore/validate the contended state before mutating conversation or
    // workspace fields. A failed session switch must leave the current session
    // wholly intact.
    app.restore_work_state(
        &session.metadata.id,
        &session.metadata.workspace,
        session.work_state.as_ref(),
    )?;
    *config = *restored_route.config;
    let projected_messages =
        crate::runtime_handoff::project_messages_for_restore(&session.messages);
    let (messages, recovered_draft) = recover_interrupted_user_tail(&projected_messages);
    app.api_messages = messages;
    app.clear_history();
    app.tool_cells.clear();
    app.tool_details_by_cell.clear();
    app.active_cell = None;
    app.active_tool_details.clear();
    app.active_tool_entry_completed_at.clear();
    app.active_cell_revision = app.active_cell_revision.wrapping_add(1);
    app.exploring_cell = None;
    app.exploring_entries.clear();
    app.ignored_tool_calls.clear();
    app.pending_tool_uses.clear();
    app.last_exec_wait_command = None;
    let messages = app.api_messages.clone();
    let mut message_to_cell = std::collections::HashMap::new();
    for (message_index, msg) in messages.iter().enumerate() {
        let mut cells = history_cells_from_message(msg);
        if msg.role == "user"
            && session
                .context_references
                .iter()
                .any(|record| record.message_index == message_index)
        {
            for cell in &mut cells {
                if let HistoryCell::User { content } = cell {
                    *content = compact_user_context_display(content);
                }
            }
        }
        let base = app.history.len();
        if msg.role == "user"
            && let Some(offset) = cells
                .iter()
                .position(|cell| matches!(cell, HistoryCell::User { .. }))
        {
            message_to_cell.insert(message_index, base + offset);
        }
        app.extend_history(cells);
    }
    app.sync_context_references_from_session(&session.context_references, &message_to_cell);
    app.mark_history_updated();
    app.viewport.transcript_selection.clear();
    restore_loaded_session_provider(app, config, provider_identity);
    app.set_model_selection(session.metadata.model.clone());
    if app.auto_model
        && let Some(saved) = session.last_auto_route.as_ref()
        && !saved.provider_identity.trim().is_empty()
        && !saved.model.trim().is_empty()
    {
        app.last_effective_provider = Some(saved.provider);
        app.last_effective_provider_identity = Some(saved.provider_identity.clone());
        app.last_effective_model = Some(saved.model.clone());
        app.last_auto_route_receipt = Some(saved.receipt.clone());
    }
    resolve_loaded_session_route(app, config);
    app.provider_models.insert(
        app.provider_identity_for_persistence().to_string(),
        app.model_selection_for_persistence(),
    );
    app.update_model_compaction_budget();
    apply_workspace_runtime_state(app, config, session.metadata.workspace.clone());
    if let Some(mode) = session.metadata.mode.as_deref().and_then(AppMode::parse) {
        app.set_mode(mode);
    }
    app.session.total_tokens = u32::try_from(session.metadata.total_tokens).unwrap_or(u32::MAX);
    app.session.total_conversation_tokens = app.session.total_tokens;
    app.session.session_cost = session.metadata.cost.session_cost_usd;
    app.session.session_cost_cny = session.metadata.cost.session_cost_cny;
    app.session.subagent_cost = session.metadata.cost.subagent_cost_usd;
    app.session.subagent_cost_cny = session.metadata.cost.subagent_cost_cny;
    app.session.subagent_cost_event_seqs.clear();
    // Restore the high-water marks from persisted metadata so the
    // monotonic cost guarantee (#244) survives session restarts.
    // Take the max with the current totals — old sessions without
    // persisted high-water fields deserialise to 0.0 and fall back to
    // the restored total with no regression.
    let total_restored_usd = session.metadata.cost.total_usd();
    let total_restored_cny = session.metadata.cost.total_cny();
    app.session.displayed_cost_high_water = session
        .metadata
        .cost
        .displayed_cost_high_water_usd
        .max(total_restored_usd);
    app.session.displayed_cost_high_water_cny = session
        .metadata
        .cost
        .displayed_cost_high_water_cny
        .max(total_restored_cny);
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_output_throughput = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    // Accumulated token breakdown is per-runtime-session; reset on load.
    app.session.reset_token_breakdown();
    app.session.turn_cache_history.clear();
    // Restore cumulative turn duration so the footer "worked" chip
    // persists across session restarts (#2038).
    app.cumulative_turn_duration =
        std::time::Duration::from_secs(session.metadata.cumulative_turn_secs);
    app.current_session_id = Some(session.metadata.id.clone());
    app.current_session_metadata = Some(session.metadata.clone());
    app.session_artifacts = session.artifacts.clone();
    app.session_title = Some(session.metadata.title.clone());
    app.workspace_context = None;
    app.workspace_context_refreshed_at = None;
    if let Some(sp) = session.system_prompt.as_ref() {
        app.system_prompt = Some(SystemPrompt::Text(sp.clone()));
    } else {
        app.system_prompt = None;
    }
    let recovered = if let Some(draft) = recovered_draft {
        restore_recovered_retry_draft(app, draft);
        true
    } else {
        false
    };
    app.scroll_to_bottom();
    Ok(recovered)
}

fn loaded_session_requires_engine_respawn(
    app: &App,
    previous_provider: ApiProvider,
    previous_provider_identity: &str,
    previous_workspace: &Path,
) -> bool {
    app.api_provider != previous_provider
        || app.provider_identity_for_persistence() != previous_provider_identity
        || app.workspace != previous_workspace
}

fn apply_loaded_session_config_snapshot(
    app: &mut App,
    config: &mut Config,
    session: &SavedSession,
    mut next_config: Config,
    force_engine_respawn: bool,
) -> Result<(bool, bool), String> {
    if force_engine_respawn {
        // File `/load` supplies a freshly loaded disk snapshot, but the live
        // Config also contains CLI and workspace/project overlays that are not
        // represented by that file. Refresh the provider registry atomically
        // over the effective Config instead of dropping permission controls.
        let mut effective_config = config.clone();
        effective_config.refresh_provider_routes_from(&next_config);
        next_config = effective_config;
    }
    let previous_provider = app.api_provider;
    let previous_provider_identity = app.provider_identity_for_persistence().to_string();
    let previous_workspace = app.workspace.clone();
    let recovered = apply_loaded_session(app, &mut next_config, session)?;
    // A file load reads a fresh disk snapshot. Even when the route's enum and
    // exact identity are unchanged, endpoint, key, headers, TLS, or retry
    // settings may have changed. Rebuild from that same validated snapshot so
    // compaction and other pre-turn engine work cannot retain the old client.
    let respawn = force_engine_respawn
        || loaded_session_requires_engine_respawn(
            app,
            previous_provider,
            &previous_provider_identity,
            &previous_workspace,
        );
    *config = next_config;
    Ok((recovered, respawn))
}

fn restore_loaded_session_provider(app: &mut App, config: &mut Config, identity: ProviderIdentity) {
    let provider = identity.provider;
    config.provider = Some(identity.key.clone());
    app.set_provider_identity_record(identity);
    app.billing_presentation = crate::route_billing::for_route(config, provider);
    app.max_subagents = config
        .max_subagents_for_provider(provider)
        .clamp(1, crate::config::MAX_SUBAGENTS);
    app.provider_chain = provider
        .kind()
        .map(|kind| codewhale_config::ProviderChain::new(kind, &config.fallback_providers))
        .filter(|chain| chain.providers().len() > 1);
    app.last_fallback_reason = None;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.reasoning_effort =
        app.reasoning_effort
            .normalize_for_route(provider, &config.deepseek_base_url(), &app.model);
    app.set_active_context_window_override(config.context_window_for_provider_config(provider));
    app.active_route_limits = app.context_window_override_limits();
    app.active_route_base_url = config.deepseek_base_url();
    app.active_context_window_source = if app.active_context_window_override.is_some() {
        crate::route_runtime::ContextWindowSource::Configured
    } else {
        crate::route_runtime::ContextWindowSource::Fallback
    };
}

fn resolve_loaded_session_route(app: &mut App, config: &Config) {
    let context_override = config.context_window_for_provider_config(app.api_provider);
    app.set_active_context_window_override(context_override);
    if app.auto_model {
        app.active_route_limits = app.context_window_override_limits();
        app.active_route_base_url = config.deepseek_base_url();
        app.active_context_window_source = if context_override.is_some() {
            crate::route_runtime::ContextWindowSource::Configured
        } else {
            crate::route_runtime::ContextWindowSource::Fallback
        };
        return;
    }

    let saved_provider_model = config
        .provider_config_for(app.api_provider)
        .and_then(|provider| provider.model.as_deref());
    match crate::route_runtime::resolve_route_candidate_with_context_metadata(
        app.api_provider,
        Some(&app.model),
        saved_provider_model,
        Some(config.deepseek_base_url()),
        context_override,
        None,
    ) {
        Ok(resolution) => app.set_active_route_resolution(
            resolution.candidate.endpoint().base_url.clone(),
            resolution.candidate.limits(),
            resolution.context_window.source,
        ),
        Err(_) => {
            app.active_route_limits = app.context_window_override_limits();
            app.active_route_base_url = config.deepseek_base_url();
            app.active_context_window_source = if context_override.is_some() {
                crate::route_runtime::ContextWindowSource::Configured
            } else {
                crate::route_runtime::ContextWindowSource::Fallback
            };
        }
    }
}

/// Derive a short display title from the API message list.
///
/// Tries several strategies in order:
/// 1. If the first user message starts with a known slash command (`/goal`,
///    `/fleet`, `/workflow`, etc.), use the command + first argument.
/// 2. Otherwise, take the first meaningful line and cut it at a natural
///    phrase boundary (period, comma, colon, or word boundary) within
///    `SESSION_TITLE_MAX_CHARS`, never splitting mid-word.
///
/// Never leaks raw prompt text — the result is always a concise label.
fn derive_session_title(messages: &[Message]) -> Option<String> {
    let text = messages.iter().find(|m| m.role == "user").and_then(|m| {
        m.content.iter().find_map(|block| match block {
            ContentBlock::Text { text, .. } if !text.starts_with(TURN_META_PREFIX) => {
                Some(text.trim().to_string())
            }
            _ => None,
        })
    })?;

    let first_line = text.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return None;
    }

    // Slash command: extract command name + first reasonable argument.
    if let Some(rest) = first_line.strip_prefix('/') {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        return match parts.as_slice() {
            [] => None,
            [cmd] => Some(format!("/{cmd}")),
            [cmd, arg, ..] => {
                let arg_short = short_title_truncate(arg, 24);
                Some(format!("/{cmd} {arg_short}"))
            }
        };
    }

    Some(short_title_truncate(first_line, SESSION_TITLE_MAX_CHARS))
}

/// Truncate `text` to at most `max_chars` characters, cutting at the last
/// natural phrase boundary (`.`, `,`, `:`, `;`, `—`, `-`, or whitespace)
/// so words are never split. Appends `…` only when text was actually cut.
pub(crate) fn short_title_truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    // Look for a natural boundary within the allowed range.
    let candidate: String = text.chars().take(max_chars).collect();
    let boundary = candidate
        .rfind(['.', ',', ':', ';', '—', '-'])
        .or_else(|| candidate.rfind(' '))
        .unwrap_or(max_chars.min(candidate.len()).saturating_sub(1));
    let cut: String = text.chars().take(boundary.max(1)).collect();
    format!("{cut}…")
}

fn recover_interrupted_user_tail(messages: &[Message]) -> (Vec<Message>, Option<QueuedMessage>) {
    let mut recovered = messages.to_vec();
    let Some(last) = recovered.last() else {
        return (recovered, None);
    };
    if last.role != "user" {
        return (recovered, None);
    }
    if crate::runtime_handoff::restored_subagent_checkpoint_display(last).is_some() {
        return (recovered, None);
    }
    let Some(display) = retry_display_from_user_message(last) else {
        return (recovered, None);
    };
    if looks_like_slash_command_input(&display) {
        return (recovered, None);
    }
    recovered.pop();
    (recovered, Some(QueuedMessage::new(display, None)))
}

fn retry_display_from_user_message(message: &Message) -> Option<String> {
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let display = compact_user_context_display(&text).trim().to_string();
    if display.is_empty() {
        None
    } else {
        Some(display)
    }
}

fn restore_recovered_retry_draft(app: &mut App, draft: QueuedMessage) {
    app.input.clone_from(&draft.display);
    app.cursor_position = app.input.chars().count();
    app.queued_draft = Some(draft);
    app.status_message = Some(
        "Recovered interrupted prompt as an editable draft; press Enter to retry.".to_string(),
    );
    app.needs_redraw = true;
}

fn compact_user_context_display(content: &str) -> String {
    content
        .split("\n\n---\n\nLocal context from @mentions:")
        .next()
        .unwrap_or(content)
        .to_string()
}

fn pause_terminal(
    terminal: &mut AppTerminal,
    use_alt_screen: bool,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
) -> Result<()> {
    // #443: pop keyboard enhancement flags before handing the terminal
    // to a child process so it doesn't inherit a half-configured input
    // mode. Best-effort — terminals that didn't accept the flags
    // silently ignore the pop. Matches the shutdown and panic paths.
    pop_keyboard_enhancement_flags(terminal.backend_mut());
    disable_alternate_scroll_mode(terminal.backend_mut());
    execute!(terminal.backend_mut(), DisableFocusChange)?;
    disable_raw_mode()?;
    if use_alt_screen {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        #[cfg(windows)]
        crate::logging::restore_verbose_state();
    }
    if use_mouse_capture {
        execute!(terminal.backend_mut(), DisableMouseCapture)?;
    }
    if use_bracketed_paste {
        disable_bracketed_paste_mode(terminal.backend_mut());
    }
    Ok(())
}

fn resume_terminal(
    terminal: &mut AppTerminal,
    use_alt_screen: bool,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
    sync_output_enabled: bool,
) -> Result<()> {
    enable_raw_mode()?;
    if use_alt_screen {
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
        // Re-entering alt-screen after mode recovery — suppress verbose
        // CLI logging again so eprintln! doesn't leak into the TUI.
        #[cfg(windows)]
        crate::logging::set_verbose(false);
    }
    recover_terminal_modes(
        terminal.backend_mut(),
        use_mouse_capture,
        use_bracketed_paste,
    );
    // Cache the real terminal size *before* resetting the viewport, so that
    // reset_terminal_viewport → terminal.clear() → autoresize() → backend.size()
    // picks up the cached size instead of falling through to
    // crossterm::terminal::size() which may return stale buffer metadata
    // (especially on Windows after a secondary EnterAlternateScreen).
    if let Ok((cols, rows)) = crossterm::terminal::size() {
        terminal
            .backend_mut()
            .set_terminal_size(Size::new(cols, rows));
    }
    reset_terminal_viewport(terminal, sync_output_enabled)?;
    Ok(())
}

fn reset_terminal_viewport(terminal: &mut AppTerminal, sync_output_enabled: bool) -> Result<()> {
    // Reset scroll margins and origin mode before clearing. Some interactive
    // child processes leave DECSTBM/DECOM behind; if ratatui's diff renderer
    // then writes "row 0", terminals can place it relative to the leaked
    // scroll region and the whole viewport appears shifted down. We
    // deliberately do *not* emit CSI 2J/3J here — see TERMINAL_ORIGIN_RESET
    // for why; the immediately-following ratatui `terminal.clear()` flushes a
    // single clear via the diff renderer, which the alt-screen buffer absorbs
    // without visible flicker on the affected terminals.
    //
    // Wrap the reset+clear sequence in DEC 2026 synchronized-output mode
    // (`\x1b[?2026h` … `\x1b[?2026l`) so GPU-accelerated terminals
    // (Ghostty, VSCode, Kitty, WezTerm) defer rendering until the whole
    // frame is staged. Terminals that don't support it silently ignore.
    // The wrap is opt-out via `synchronized_output = "off"` for terminals
    // that mishandle the sequence (Ptyxis 50.x on VTE 0.84.x flashes the
    // whole viewport on each wrapped frame).
    if sync_output_enabled {
        let _ = terminal.backend_mut().write_all(BEGIN_SYNC_UPDATE);
    }

    let result = (|| -> Result<()> {
        terminal.backend_mut().write_all(TERMINAL_ORIGIN_RESET)?;
        terminal.clear()?;
        Ok(())
    })();

    // Always end the synchronized update, regardless of success or failure.
    if sync_output_enabled {
        let _ = terminal.backend_mut().write_all(END_SYNC_UPDATE);
    }
    let _ = terminal.backend_mut().flush();
    result
}

fn push_keyboard_enhancement_flags<W: Write>(writer: &mut W) {
    // crossterm's PushKeyboardEnhancementFlags command unconditionally
    // returns Unsupported on Windows (is_ansi_code_supported() == false), so
    // the ANSI escape is written directly on that platform. Modern Windows
    // terminals (VSCode integrated terminal, Windows Terminal ≥1.17) honour
    // the kitty keyboard protocol but crossterm's event reader does not
    // decode CSI u sequences on Windows (issue #1599). Write \033[>0u to
    // probe the protocol without enabling any flags — Enter stays as \n.
    #[cfg(windows)]
    {
        if let Err(err) = write!(writer, "\x1b[>0u").and_then(|()| writer.flush()) {
            tracing::debug!(
                target: "kitty_keyboard",
                ?err,
                "PushKeyboardEnhancementFlags direct write failed on Windows"
            );
        }
    }
    #[cfg(not(windows))]
    if let Err(err) = execute!(
        writer,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    ) {
        tracing::debug!(
            target: "kitty_keyboard",
            ?err,
            "PushKeyboardEnhancementFlags ignored (terminal lacks support)"
        );
    }
}

pub(crate) fn pop_keyboard_enhancement_flags<W: Write>(writer: &mut W) {
    // Mirror of push_keyboard_enhancement_flags: crossterm's
    // PopKeyboardEnhancementFlags also has is_ansi_code_supported() == false
    // on Windows, so write the pop escape directly to restore the terminal to
    // its pre-launch keyboard mode.
    // pub(crate) so the panic hook in main.rs and external_editor.rs can
    // also call the Windows-aware path instead of using the raw crossterm
    // execute!() macro which silently no-ops on Windows.
    #[cfg(windows)]
    {
        if let Err(err) = write!(writer, "\x1b[<1u").and_then(|()| writer.flush()) {
            tracing::debug!(
                target: "kitty_keyboard",
                ?err,
                "PopKeyboardEnhancementFlags direct write failed on Windows"
            );
        }
    }
    #[cfg(not(windows))]
    let _ = execute!(writer, PopKeyboardEnhancementFlags);
}

fn set_alternate_scroll_mode<W: Write>(writer: &mut W, enabled: bool) {
    let sequence = if enabled {
        ENABLE_ALT_SCROLL_MODE
    } else {
        DISABLE_ALT_SCROLL_MODE
    };
    if let Err(err) = writer.write_all(sequence).and_then(|()| writer.flush()) {
        tracing::debug!(
            ?err,
            enabled,
            "alternate-scroll terminal mode change ignored"
        );
    }
}

fn enable_alternate_scroll_mode<W: Write>(writer: &mut W) {
    set_alternate_scroll_mode(writer, true);
}

pub(crate) fn disable_alternate_scroll_mode<W: Write>(writer: &mut W) {
    set_alternate_scroll_mode(writer, false);
}

/// Best-effort terminal restoration for emergency exit paths
/// (panic hook, signal handlers). Mirrors the normal teardown in
/// `run_event_loop` but tolerates any subset of modes not actually being
/// active — every step is discarded on failure so a half-initialized TUI
/// (e.g. SIGINT during startup before `EnterAlternateScreen`) still gets
/// raw mode + kitty keyboard flags cleared, which is what causes the
/// `^[[>5u` shell pollution reported in #1583.
pub fn emergency_restore_terminal() {
    let mut stdout = std::io::stdout();
    pop_keyboard_enhancement_flags(&mut stdout);
    disable_alternate_scroll_mode(&mut stdout);
    let _ = execute!(stdout, DisableFocusChange);
    disable_bracketed_paste_mode(&mut stdout);
    let _ = execute!(stdout, DisableMouseCapture);
    let _ = disable_raw_mode();
    let _ = execute!(stdout, LeaveAlternateScreen);
}

/// On Windows, ensure the console input handle has `ENABLE_WINDOW_INPUT`
/// (0x0008) set. crossterm's `enable_raw_mode()` removes this flag, which
/// breaks IME composition (Chinese/Japanese/Korean input methods cannot
/// commit characters) on some Windows configurations (e.g. Windows Terminal
/// in conhost compatibility mode, or the legacy console with VT input).
///
/// Best-effort and idempotent. Silently ignored if the console handle or
/// mode query fails.
#[cfg(target_os = "windows")]
fn enable_windows_ime_console_mode() {
    use windows::Win32::System::Console::CONSOLE_MODE;
    const ENABLE_WINDOW_INPUT: CONSOLE_MODE = CONSOLE_MODE(0x0008);

    // SAFETY: Win32 console API is safe to call from any thread.
    // Failures (console handle invalid, mode query fails) are silently
    // ignored — this is a best-effort IME compatibility tweak.
    unsafe {
        let Ok(handle) = GetStdHandle(windows::Win32::System::Console::STD_INPUT_HANDLE) else {
            return;
        };
        let mut mode = CONSOLE_MODE(0);
        if GetConsoleMode(handle, &mut mode).is_err() {
            return;
        }
        if mode.0 & ENABLE_WINDOW_INPUT.0 == 0 {
            let _ = SetConsoleMode(handle, mode | ENABLE_WINDOW_INPUT);
        }
    }
}

/// Re-establish terminal mode flags. Idempotent and best-effort: each
/// underlying flag is silently discarded by terminals that don't support
/// it, and a single flag's failure doesn't prevent later flags from being
/// attempted.
///
/// **Canonical location for terminal-mode setup.** If you add a new mode
/// flag at startup or in `resume_terminal`, add it here too — `FocusGained`
/// recovery calls this and will silently fall behind otherwise.
///
/// Excluded by design: raw mode and the alternate screen — those persist
/// across focus events and are only re-established by `resume_terminal`
/// after a suspension, which always runs a separate path.
///
pub(crate) fn recover_terminal_modes<W: Write>(
    writer: &mut W,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
) {
    #[cfg(target_os = "windows")]
    enable_windows_ime_console_mode();

    pop_keyboard_enhancement_flags(writer);
    push_keyboard_enhancement_flags(writer);
    if use_mouse_capture {
        enable_alternate_scroll_mode(writer);
        if let Err(err) = execute!(writer, EnableMouseCapture) {
            tracing::debug!(?err, "EnableMouseCapture ignored");
        }
    } else {
        disable_alternate_scroll_mode(writer);
    }
    if use_bracketed_paste {
        try_enable_bracketed_paste_mode(writer);
    }
    if let Err(err) = execute!(writer, EnableFocusChange) {
        tracing::debug!(?err, "EnableFocusChange ignored");
    }
}

fn try_enable_bracketed_paste_mode<W: Write>(writer: &mut W) -> bool {
    match execute!(writer, EnableBracketedPaste) {
        Ok(()) => true,
        Err(err) => {
            tracing::debug!(?err, "EnableBracketedPaste ignored");
            false
        }
    }
}

pub(crate) fn disable_bracketed_paste_mode<W: Write>(writer: &mut W) {
    if let Err(err) = execute!(writer, DisableBracketedPaste) {
        tracing::debug!(?err, "DisableBracketedPaste ignored");
    }
}

fn terminal_event_needs_viewport_recapture(evt: &Event) -> bool {
    matches!(evt, Event::FocusGained)
}

pub(crate) fn status_color(level: StatusToastLevel) -> ratatui::style::Color {
    match level {
        StatusToastLevel::Info => palette::WHALE_INFO,
        StatusToastLevel::Success => palette::STATUS_SUCCESS,
        StatusToastLevel::Warning => palette::STATUS_WARNING,
        StatusToastLevel::Error => palette::STATUS_ERROR,
    }
}

/// Maximum stacked toasts rendered above the footer (#439). The footer line
/// itself stays the most-recent; this overlay surfaces up to two older
/// queued toasts so a burst of status events isn't dropped silently.
const TOAST_STACK_MAX_VISIBLE: usize = 3;

/// Render up to `TOAST_STACK_MAX_VISIBLE - 1` *additional* toasts as an
/// overlay just above the footer when multiple are active. The most recent
/// toast continues to render in the footer line itself; this strip is for
/// the older entries the user would otherwise miss when statuses arrive in
/// bursts.
fn render_toast_stack_overlay(
    f: &mut Frame,
    full_area: Rect,
    composer_area: Rect,
    footer_area: Rect,
    app: &mut App,
) {
    let toasts = app.active_status_toasts(TOAST_STACK_MAX_VISIBLE);
    if toasts.len() < 2 || footer_area.y == 0 {
        return;
    }
    // Drop the most recent (rendered inline by the footer), keep the rest.
    let extra = toasts.len() - 1;
    let stack_height = extra.min(TOAST_STACK_MAX_VISIBLE - 1) as u16;
    // Toast stack can only use space between composer and footer.
    // Composer occupies rows [composer_area.y, composer_area.y + composer_area.height).
    // Toast must start at or after row (composer_area.y + composer_area.height).
    let composer_end = composer_area.y + composer_area.height;
    let max_above = footer_area.y.saturating_sub(composer_end);
    if stack_height == 0 || max_above == 0 {
        return;
    }
    let height = stack_height.min(max_above);
    let stack_area = Rect {
        x: full_area.x,
        y: footer_area.y.saturating_sub(height),
        width: full_area.width,
        height,
    };
    // Iterate oldest-first so the freshest *non-inline* toast is closest to
    // the footer (visually nearest the most-recent message in the line below).
    let visible = &toasts[..extra];
    for (i, toast) in visible.iter().take(height as usize).enumerate() {
        let row_y = stack_area.y + i as u16;
        let row = Rect {
            x: stack_area.x,
            y: row_y,
            width: stack_area.width,
            height: 1,
        };
        let style = ratatui::style::Style::default()
            .fg(status_color(toast.level))
            .add_modifier(ratatui::style::Modifier::DIM);
        let line = ratatui::text::Line::styled(format!(" {} ", toast.text), style);
        f.render_widget(ratatui::widgets::Paragraph::new(line), row);
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

    let Some(shell_manager) = app.runtime_services.shell_manager.clone() else {
        app.status_message = Some("No shell session is active.".to_string());
        return;
    };

    match shell_manager.lock() {
        Ok(mut manager) => {
            manager.request_foreground_background();
            app.status_message = Some("Moving current shell command to /jobs...".to_string());
        }
        Err(_) => {
            app.status_message = Some(
                "Shell tracking hit an internal error — restart Codewhale to recover.".to_string(),
            );
        }
    }
}

pub(crate) fn prefill_jobs_cancel_all_if_tasks_sidebar(app: &mut App) -> bool {
    if !app.view_stack.is_empty()
        || app.sidebar_focus != SidebarFocus::Tasks
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
                    if exec.status == ToolStatus::Running && exec.interaction.is_none()
            )
        })
    })
}

pub(crate) fn terminal_pause_has_live_owner(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|cell| {
            matches!(
                cell,
                HistoryCell::Tool(ToolCell::Exec(exec)) if exec.status == ToolStatus::Running
            )
        })
    })
}

#[allow(dead_code)]
fn transcript_scroll_percent(top: usize, visible: usize, total: usize) -> Option<u16> {
    if total <= visible {
        return None;
    }

    let max_top = total.saturating_sub(visible);
    if max_top == 0 {
        return None;
    }

    let clamped_top = top.min(max_top);
    let percent = ((clamped_top as f64 / max_top as f64) * 100.0).round() as u16;
    Some(percent.min(100))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchDirection {
    Forward,
    Backward,
}

fn jump_to_adjacent_tool_cell(app: &mut App, direction: SearchDirection) -> bool {
    let line_meta = app.viewport.transcript_cache.line_meta();
    if line_meta.is_empty() {
        return false;
    }

    let top = app
        .viewport
        .last_transcript_top
        .min(line_meta.len().saturating_sub(1));
    let current_cell = line_meta
        .get(top)
        .and_then(crate::tui::scrolling::TranscriptLineMeta::cell_line)
        .map(|(cell_index, _)| app.original_cell_index_for_rendered(cell_index));

    let mut scan_indices = Vec::new();
    match direction {
        SearchDirection::Forward => {
            scan_indices.extend((top.saturating_add(1))..line_meta.len());
        }
        SearchDirection::Backward => {
            scan_indices.extend((0..top).rev());
        }
    }

    for idx in scan_indices {
        let Some((cell_index, _)) = line_meta[idx].cell_line() else {
            continue;
        };
        let cell_index = app.original_cell_index_for_rendered(cell_index);
        if current_cell.is_some_and(|current| current == cell_index) {
            continue;
        }
        if !matches!(app.history.get(cell_index), Some(HistoryCell::Tool(_))) {
            continue;
        }
        if let Some(anchor) = TranscriptScroll::anchor_for(line_meta, idx) {
            app.viewport.transcript_scroll = anchor;
            app.viewport.pending_scroll_delta = 0;
            app.needs_redraw = true;
            return true;
        }
    }

    false
}

fn estimated_context_tokens(app: &App) -> Option<i64> {
    i64::try_from(estimate_input_tokens_conservative(
        &app.api_messages,
        app.system_prompt.as_ref(),
    ))
    .ok()
}

pub(crate) fn context_usage_snapshot(app: &App) -> Option<(i64, u32, f64)> {
    let max = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
    context_usage_snapshot_for_window(app, max)
}

fn context_usage_snapshot_for_window(app: &App, max: u32) -> Option<(i64, u32, f64)> {
    let max_i64 = i64::from(max);
    let reported = app
        .session
        .last_prompt_tokens
        .map(i64::from)
        .map(|tokens| tokens.max(0));
    let estimated = estimated_context_tokens(app).map(|tokens| tokens.max(0));

    // Always prefer the estimated current-context size (computed from
    // `app.api_messages`) when we have it. Reported `last_prompt_tokens`
    // comes from `Event::TurnComplete.usage`, which the engine builds with
    // `turn.add_usage` — that SUMS input_tokens across every round in the
    // turn, so a multi-round tool-call turn reports a value much larger
    // than the actual context window state, then the next single-round
    // turn drops back to a single round's input_tokens. User-visible %
    // was bouncing 31% → 9% (#115) because of this. The estimate is
    // monotonic wrt conversation growth, which is what a "context filling
    // up" indicator should show. We still consult `reported` only as a
    // fallback when no estimate is available (e.g., immediately after a
    // session restore before the api_messages are populated).
    let used = match (estimated, reported) {
        (Some(estimated), _) => estimated.min(max_i64),
        (None, Some(reported)) => reported.min(max_i64),
        (None, None) => return None,
    };

    let max_f64 = f64::from(max);
    let used_f64 = used as f64;
    let percent = ((used_f64 / max_f64) * 100.0).clamp(0.0, 100.0);
    Some((used, max, percent))
}

/// Retained as a callable utility — `context_usage_snapshot` no longer uses
/// it directly (#115 makes the estimate the primary signal), but tests in
/// `ui/tests.rs` still exercise it and a future heuristic may want to
/// distinguish "obviously inflated reported tokens" from healthy reports.
#[allow(dead_code)]
fn is_reported_context_inflated(reported: i64, estimated: i64) -> bool {
    const MIN_ABSOLUTE_GAP: i64 = 4_096;
    if estimated <= 0 || reported <= estimated {
        return false;
    }

    reported.saturating_sub(estimated) >= MIN_ABSOLUTE_GAP
        && reported >= estimated.saturating_mul(4)
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

fn status_animation_interval_ms(app: &App) -> u64 {
    if app.effective_low_motion_for_status() {
        2_400
    } else {
        UI_STATUS_ANIMATION_MS
    }
}

/// Whether any underwater motion owner is actually visible in the transcript
/// host. This keeps the scheduler honest: ombre needs a non-empty viewport,
/// fish need their collision-safe water budget, and the smaller idle whale may
/// independently earn its caustic. Obscured surfaces never request frames.
#[must_use]
fn underwater_motion_surface_visible(
    area: Option<Rect>,
    ombre_field_breathes: bool,
    empty_water_visible: bool,
    obscured: bool,
) -> bool {
    if obscured {
        return false;
    }
    area.is_some_and(|area| {
        area.width > 0
            && area.height > 0
            && (ombre_field_breathes
                || (area.width >= crate::tui::ocean::AMBIENT_MIN_WIDTH
                    && area.height >= crate::tui::ocean::AMBIENT_MIN_HEIGHT)
                || (empty_water_visible && crate::tui::underwater::empty_state_mark_visible(area)))
    })
}

fn animation_interval_ms(app: &App, status_motion: bool, underwater_motion: bool) -> u64 {
    match (status_motion, underwater_motion) {
        (true, true) => status_animation_interval_ms(app).min(UI_UNDERWATER_ANIMATION_MS),
        (true, false) => status_animation_interval_ms(app),
        (false, true) => UI_UNDERWATER_ANIMATION_MS,
        (false, false) => UI_UNDERWATER_ANIMATION_MS,
    }
}

fn active_poll_ms(app: &App) -> u64 {
    if app.low_motion {
        96
    } else {
        UI_ACTIVE_POLL_MS
    }
}

fn idle_poll_ms(app: &App) -> u64 {
    if app.low_motion { 120 } else { UI_IDLE_POLL_MS }
}

fn clamp_event_poll_timeout(timeout: Duration) -> Duration {
    const MIN_EVENT_POLL_TIMEOUT: Duration = Duration::from_millis(1);
    timeout.max(MIN_EVENT_POLL_TIMEOUT)
}

/// True while a `workflow` tool is executing in the foreground (active cell)
/// or still shown as running in history. Used to keep per-subagent completion
/// notifications quiet during a workflow run under `final-only`.
fn workflow_tool_is_running(app: &App) -> bool {
    fn is_running_workflow(cell: &HistoryCell) -> bool {
        matches!(
            cell,
            HistoryCell::Tool(ToolCell::Generic(tool))
                if tool.name == "workflow" && tool.status == ToolStatus::Running
        )
    }
    app.history.iter().any(is_running_workflow)
        || app
            .active_cell
            .as_ref()
            .is_some_and(|active| active.entries().iter().any(is_running_workflow))
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

fn should_tick_status_animation(
    app: &App,
    has_running_agents: bool,
    history_has_live_motion: bool,
    active_cell_has_live_motion: bool,
    translation_placeholder_has_live_motion: bool,
) -> bool {
    !matches!(app.motion_policy().mode(), MotionMode::Still)
        && (app.is_loading
            || has_running_agents
            || app.is_compacting
            || app.is_purging
            || history_has_live_motion
            || active_cell_has_live_motion
            || translation_placeholder_has_live_motion
            || visible_background_task_has_live_motion(app))
}

fn visible_background_task_has_live_motion(app: &App) -> bool {
    matches!(
        app.sidebar_focus,
        SidebarFocus::Auto | SidebarFocus::Pinned | SidebarFocus::Tasks
    ) && app
        .last_sidebar_area
        .or(app.viewport.last_sidebar_area)
        .is_some()
        && app.task_panel.iter().any(|task| task.status == "running")
}

fn active_cell_has_live_motion(app: &App) -> bool {
    app.active_cell
        .as_ref()
        .is_some_and(|active| active.entries().iter().any(HistoryCell::has_live_motion))
}

fn history_has_live_motion(history: &[HistoryCell]) -> bool {
    history.iter().any(HistoryCell::has_live_motion)
}

pub(crate) fn open_pager_for_selection(app: &mut App) -> bool {
    let Some(text) = selection_to_text(app) else {
        return false;
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let pager = PagerView::from_text("Selection", &text, width.saturating_sub(2));
    app.view_stack.push(pager);
    true
}

fn open_pager_for_last_message(app: &mut App) -> bool {
    let Some(cell) = app.history.last() else {
        return false;
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let text = history_cell_to_text(cell, width);
    let pager = PagerView::from_text("Message", &text, width.saturating_sub(2));
    app.view_stack.push(pager);
    true
}

/// Compatibility wrapper for the old test name. Exercises the single-cell
/// Activity Detail helper (still used by `v`-adjacent detail paths); the
/// user-facing Ctrl+O surface is now the whole-turn Turn Inspector (#4104).
#[cfg(test)]
fn open_thinking_pager(app: &mut App) -> bool {
    open_activity_detail_pager(app)
}

// Keyboard-shortcut predicates moved to `tui/key_shortcuts.rs`.

#[derive(Debug, Clone, PartialEq, Eq)]
enum StartupVersionCheckSource {
    Disabled,
    ConfiguredUrl(String),
    ReleaseResolver,
}

fn startup_version_check_source(config: &UpdateConfig) -> StartupVersionCheckSource {
    if !config.check_for_updates {
        return StartupVersionCheckSource::Disabled;
    }
    if let Some(update_uri) = config.update_uri() {
        return StartupVersionCheckSource::ConfiguredUrl(update_uri.to_string());
    }
    StartupVersionCheckSource::ReleaseResolver
}

fn spawn_startup_version_check(
    config: UpdateConfig,
) -> Option<tokio::task::JoinHandle<Option<String>>> {
    let source = startup_version_check_source(&config);
    if source == StartupVersionCheckSource::Disabled {
        return None;
    }

    let current = env!("CARGO_PKG_VERSION").to_string();
    Some(tokio::spawn(async move {
        version_hint_from_startup_source(source, &current).await
    }))
}

async fn version_hint_from_startup_source(
    source: StartupVersionCheckSource,
    current: &str,
) -> Option<String> {
    match source {
        StartupVersionCheckSource::Disabled => None,
        StartupVersionCheckSource::ConfiguredUrl(url) => {
            match version_hint_from_configured_update_uri(&url, current).await {
                Ok(hint) => hint,
                Err(_) => version_hint_from_release_mirror_env(current).await,
            }
        }
        StartupVersionCheckSource::ReleaseResolver => {
            if release_mirror_env_configured() {
                return version_hint_from_release_mirror_env(current).await;
            }

            let body = codewhale_release::fetch_release_json_async(
                codewhale_release::LATEST_RELEASE_URL,
                "latest release",
            )
            .await
            .ok()?;
            let json: serde_json::Value = serde_json::from_str(&body).ok()?;
            version_hint_from_release_json(&json, current)
        }
    }
}

async fn version_hint_from_release_mirror_env(current: &str) -> Option<String> {
    if !release_mirror_env_configured() {
        return None;
    }
    let tag =
        codewhale_release::latest_release_tag_async(codewhale_release::ReleaseChannel::Stable)
            .await
            .ok()?;
    version_hint_from_latest_tag(&tag, current)
}

fn release_mirror_env_configured() -> bool {
    let version = codewhale_release::update_version_from_env()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    codewhale_release::release_base_url_from_env(&version).is_some()
}

async fn version_hint_from_configured_update_uri(
    update_uri: &str,
    current: &str,
) -> Result<Option<String>> {
    let body = codewhale_release::fetch_release_json_async(update_uri, "configured latest release")
        .await?;
    let json: serde_json::Value = serde_json::from_str(&body).with_context(|| {
        format!("failed to parse release JSON from configured URI {update_uri}")
    })?;
    Ok(version_hint_from_custom_release_json(&json, current))
}

fn version_hint_from_release_json(json: &serde_json::Value, current: &str) -> Option<String> {
    if !release_has_required_assets(json) {
        return None;
    }

    let tag = json["tag_name"].as_str()?;
    version_hint_from_latest_tag(tag, current)
}

fn version_hint_from_custom_release_json(
    json: &serde_json::Value,
    current: &str,
) -> Option<String> {
    if !release_is_publishable(json) {
        return None;
    }
    if json.get("assets").is_some() && !release_has_required_assets(json) {
        return None;
    }
    let tag = json["tag_name"].as_str()?;
    version_hint_from_latest_tag(tag, current)
}

fn version_hint_from_latest_tag(tag: &str, current: &str) -> Option<String> {
    let latest = tag.trim_start_matches('v');
    if !is_newer_version(latest, current) {
        return None;
    }

    Some(format!(
        "v{latest} available - run `codewhale update` and restart, then /change for what's new"
    ))
}

fn release_has_required_assets(json: &serde_json::Value) -> bool {
    if !release_is_publishable(json) {
        return false;
    }

    REQUIRED_RELEASE_ASSETS
        .iter()
        .all(|required| release_has_uploaded_asset(json, required))
}

fn release_is_publishable(json: &serde_json::Value) -> bool {
    !json
        .get("draft")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && !json
            .get("prerelease")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
}

fn release_has_uploaded_asset(json: &serde_json::Value, required: &str) -> bool {
    let Some(assets) = json.get("assets").and_then(serde_json::Value::as_array) else {
        return false;
    };
    assets.iter().any(|asset| {
        asset.get("name").and_then(serde_json::Value::as_str) == Some(required)
            && asset.get("state").and_then(serde_json::Value::as_str) == Some("uploaded")
    })
}

fn is_newer_version(latest: &str, current: &str) -> bool {
    // Compare semver so dev builds (e.g. "0.8.46-pre") don't trigger false
    // hints. Falls back to string compare on unparseable versions.
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => latest != current,
    }
}

/// Parse a `major.minor.patch` version string into a comparable tuple.
/// Returns `None` on any parse failure (non-semver, dev suffixes, etc.).
fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    Some((major, minor, patch))
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
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: false,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
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
"#,
        )
        .expect("seed config");

        let mut app = create_test_app();
        let mut engine = mock_engine_handle();
        let mut config = openrouter_config("https://mock.openrouter.test/v1");
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

#[cfg(test)]
mod tests;
