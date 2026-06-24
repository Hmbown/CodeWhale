//! TUI event loop and rendering logic for `DeepSeek` CLI.

use std::cell::Cell;
use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
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
        EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
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
    CacheWarmupKey, DeepSeekClient, PromptInspection, build_cache_warmup_request,
    inspect_prompt_for_request,
};
use crate::commands;
use crate::compaction::estimate_input_tokens_conservative;
use crate::config::{
    ApiProvider, Config, ProviderConfig, ProvidersConfig, StatusItem, UpdateConfig,
    provider_capability, save_provider_auth_mode_for,
};
use crate::config_ui::{self, ConfigUiMode, WebConfigSession, WebConfigSessionEvent};
use crate::core::engine::{EngineConfig, EngineHandle, spawn_engine};
use crate::core::events::Event as EngineEvent;
use crate::core::ops::{Op, USER_SHELL_TOOL_ID_PREFIX};
use crate::hooks::{HookEvent, HookExecutor, TurnEndPayloadInput, TurnEndTotals};
use crate::llm_client::LlmClient;
use crate::localization::{MessageId, tr};
use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt, Usage};
use crate::palette;
use crate::prompts;
use crate::route_runtime::{resolve_route_candidate, resolve_runtime_route};
use crate::session_manager::{
    OfflineQueueState, QueuedSessionMessage, SavedSession, SessionManager,
    create_saved_session_with_id_and_mode, create_saved_session_with_mode, update_session,
};
use crate::settings::Settings;
use crate::task_manager::{
    NewTaskRequest, SharedTaskManager, TaskManager, TaskManagerConfig, TaskStatus, TaskSummary,
};
use crate::tools::goal::{GoalSnapshot, GoalStatus};
use crate::tools::shell::{ShellJobSnapshot, ShellStatus};
use crate::tools::spec::{RuntimeToolServices, ToolResult};
use crate::tools::subagent::SubAgentStatus;
use crate::tui::auto_router;
use crate::tui::color_compat::ColorCompatBackend;
use crate::tui::command_palette::{
    CommandPaletteView, build_entries as build_command_palette_entries,
};
use crate::tui::composer_ui::*;
use crate::tui::context_inspector::build_context_inspector_text;
use crate::tui::event_broker::EventBroker;
use crate::tui::file_picker_relevance;
use crate::tui::footer_ui::{
    friendly_subagent_progress, is_noisy_subagent_progress, one_line_summary, render_footer,
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
use crate::tui::scrolling::TranscriptScroll;
// SelectionAutoscroll unused
use crate::tui::session_picker::SessionPickerView;
use crate::tui::shell_job_routing::{
    add_shell_job_message, format_shell_job_list, format_shell_poll, open_shell_job_pager,
};
use crate::tui::streaming_thinking;
#[cfg(test)]
use crate::tui::subagent_routing::reconcile_subagent_activity_state_at;
use crate::tui::subagent_routing::{
    format_task_list, handle_subagent_mailbox, open_task_pager, reconcile_subagent_activity_state,
    running_agent_count, sort_subagents_in_place, subagent_message_refreshes_workspace_context,
    task_mode_label, task_summary_to_panel_entry,
};
#[cfg(test)]
use crate::tui::tool_routing::exploring_label;
use crate::tui::tool_routing::{
    handle_tool_call_complete, handle_tool_call_started, maybe_add_patch_preview,
};
use crate::tui::ui_text::{
    history_cell_to_text, line_to_plain, text_display_width, truncate_line_to_width,
};
use crate::tui::user_input::UserInputView;
use crate::tui::views::subagent_view_agents;
use crate::tui::vim_mode;
use crate::tui::workspace_context;

use super::key_actions;

use super::app::{
    App, AppAction, AppMode, HuntVerdict, OnboardingState, PendingProviderSwitch, QueuedMessage,
    ReasoningEffort, SidebarFocus, StatusToastLevel, SubmitDisposition, TaskPanelEntry,
    TaskPanelEntryKind, TuiOptions, looks_like_slash_command_input, shell_command_from_bang_input,
};
use super::approval::{
    ApprovalMode, ApprovalRequest, ApprovalView, ElevationRequest, ElevationView, ReviewDecision,
};
use super::history::{
    HistoryCell, ToolCell, ToolStatus, TranscriptRenderOptions, history_cells_from_message,
    summarize_tool_output,
};
use super::slash_menu::{
    apply_slash_menu_selection, partial_inline_skill_mention_at_cursor,
    try_autocomplete_slash_command, visible_slash_menu_entries,
};
use super::views::{ConfigView, ContextMenuAction, HelpView, ModalKind, ViewEvent};
use super::widgets::pending_input_preview::{ContextPreviewItem, PendingInputPreview};
use super::widgets::{ChatWidget, ComposerWidget, HeaderData, HeaderWidget, Renderable};

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
/// Maximum wall-clock time a turn may stay in `"in_progress"` before the UI
/// assumes the engine stalled (e.g. sub-agent hang, lost completion event,
/// engine panic).  Matched to [`DEFAULT_STREAM_IDLE_TIMEOUT`] so legitimate
/// long-running tool chains are not interrupted prematurely.
const TURN_STALL_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(300);
/// Running tools can legitimately exceed the silent-turn timeout, but a tool
/// with no progress heartbeat or output beyond this ceiling is treated as hung.
const TOOL_HANG_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(900);
// Forced repaint cadence while a turn is live (model loading, compacting,
// sub-agents running). Drives the footer water-spout animation as well as
// the per-tool spinner pulse — keep this fast enough that the spout reads as
// motion (~12 fps) instead of teleport-frames.
const UI_STATUS_ANIMATION_MS: u64 = 80;
pub(crate) const SIDEBAR_VISIBLE_MIN_WIDTH: u16 = 64;
const DEFAULT_TERMINAL_PROBE_TIMEOUT_MS: u64 = 500;
const PERIODIC_FULL_REPAINT_EVERY_N: u64 = 50;
const TURN_META_PREFIX: &str = "<turn_meta>";
const SESSION_TITLE_MAX_CHARS: usize = 32;
const VERSION_HINT_TOAST_TTL_MS: u64 = 12_000;

const REQUIRED_RELEASE_ASSETS: &[&str] = &[
    "codewhale-artifacts-sha256.txt",
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
];

fn is_session_approved_for_tool(app: &App, tool_name: &str, grouping_key: &str) -> bool {
    app.approval_session_approved.contains(grouping_key)
        || app.approval_session_approved.contains(tool_name)
}

fn is_session_denied_for_key(app: &App, approval_key: &str) -> bool {
    app.approval_session_denied.contains(approval_key)
}

fn should_auto_approve_approval_request(
    app: &App,
    tool_name: &str,
    grouping_key: &str,
    approval_force_prompt: bool,
) -> bool {
    !approval_force_prompt
        && (is_session_approved_for_tool(app, tool_name, grouping_key)
            || app.approval_mode == ApprovalMode::Auto)
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
// viewport. Crossterm's mouse-capture command does not enable this DEC private
// mode, so terminals can still scroll the host scrollback if mouse capture is
// disabled, dropped during focus changes, or unavailable in the host.
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
const MAX_ENGINE_EVENTS_PER_DRAIN: usize = 128;

enum TerminalInputMessage {
    Event(Event),
    Heartbeat,
    Error(io::Error),
}

struct TerminalInputPump {
    rx: std::sync::mpsc::Receiver<TerminalInputMessage>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    last_alive_at: Cell<Instant>,
}

impl TerminalInputPump {
    fn spawn() -> io::Result<Self> {
        let (rx, stop, handle) = Self::spawn_parts()?;
        Ok(Self {
            rx,
            stop,
            handle: Some(handle),
            last_alive_at: Cell::new(Instant::now()),
        })
    }

    fn spawn_parts() -> io::Result<(
        std::sync::mpsc::Receiver<TerminalInputMessage>,
        Arc<AtomicBool>,
        JoinHandle<()>,
    )> {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("codewhale-terminal-input".to_string())
            .spawn(move || {
                let mut last_heartbeat = Instant::now();
                while !thread_stop.load(Ordering::Acquire) {
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
        Ok((rx, stop, handle))
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

    #[cfg(target_os = "windows")]
    fn restart_detached(&mut self) -> io::Result<()> {
        self.stop.store(true, Ordering::Release);
        let _ = self.handle.take();
        let (rx, stop, handle) = Self::spawn_parts()?;
        self.rx = rx;
        self.stop = stop;
        self.handle = Some(handle);
        self.last_alive_at.set(Instant::now());
        Ok(())
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
pub async fn run_tui(config: &Config, options: TuiOptions) -> Result<()> {
    let use_alt_screen = options.use_alt_screen;
    let use_mouse_capture = options.use_mouse_capture;
    let use_bracketed_paste = options.use_bracketed_paste;

    // Apply OSC 8 hyperlink toggle from config.
    //
    // #3029: OSC 8 hyperlinks are now emitted out-of-band. The transcript
    // carries the link payloads in-band inside `Span` content, but each render
    // seam calls `osc8::extract_buffer_link_regions`, which blanks the payload
    // cells (so no buffer cell ever holds `\x1b` or `]8;;` — the old column-
    // drift corruption is gone by construction) and publishes `LinkRegion`s.
    // `ColorCompatBackend::draw` then re-emits the OSC 8 escapes through the
    // backend's `Write` impl, interleaved with the cell stream — never inside a
    // buffer cell. So the corruption that previously forced this default off is
    // fixed, and hyperlinks are on by default for terminals that handle the OSC
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

    // Terminal probe with timeout to prevent hanging on unresponsive terminals
    let probe_timeout = terminal_probe_timeout(config);
    let enable_raw = tokio::task::spawn_blocking(move || {
        enable_raw_mode().map_err(|e| anyhow::anyhow!("Failed to enable raw mode: {e}"))
    });

    match tokio::time::timeout(probe_timeout, enable_raw).await {
        Ok(inner_result) => {
            inner_result??; // propagate both join and raw-mode errors
        }
        Err(_) => {
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
        // Windows also suppresses CodeWhale's own verbose CLI logger while
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
    let mut app = App::new(options.clone(), config);
    sync_config_provider_from_app(config, &app);

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
            Ok(Some(saved)) => {
                let recovered = apply_loaded_session(&mut app, config, &saved);
                if !recovered {
                    app.status_message = Some(format!(
                        "Resumed session: {}",
                        crate::session_manager::truncate_id(&saved.metadata.id)
                    ));
                }
            }
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
        // #456: plumb the App's HookExecutor so `exec_shell` can surface
        // the configured `shell_env` hooks. Clone the shared Arc.
        hook_executor: app.runtime_services.hook_executor.clone(),
        handle_store: app.runtime_services.handle_store.clone(),
        rlm_sessions: app.runtime_services.rlm_sessions.clone(),
    };
    refresh_active_task_panel(&mut app, &task_manager).await;

    let engine_config = build_engine_config(&app, config);

    // Spawn the Engine - it will handle all API communication
    let engine_handle = spawn_engine(engine_config, config);
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
            })
            .await;
    }

    // Fire session start hook
    {
        let context = app.base_hook_context();
        let _ = app.execute_hooks(HookEvent::SessionStart, &context);
    }

    // Spawn the persistence actor so checkpoint/session-save I/O stays off
    // the UI thread.  The actor serialises + writes to disk in a dedicated
    // task; the UI just `try_send`s a request and returns immediately.
    if let Ok(persist_manager) = SessionManager::default_location() {
        let handle = persistence_actor::spawn_persistence_actor(persist_manager);
        persistence_actor::init_actor(handle);
    }

    submit_initial_input_if_ready(&mut app, config, &engine_handle).await?;

    let result = run_event_loop(
        &mut terminal,
        &mut app,
        config,
        engine_handle,
        task_manager,
        &event_broker,
        translation_client,
    )
    .await;
    automation_cancel.cancel();
    automation_scheduler.abort();

    // Fire session end hook
    {
        let context = app.base_hook_context();
        let _ = app.execute_hooks(HookEvent::SessionEnd, &context);
    }

    // Flush the persistence actor: clear checkpoint + graceful shutdown.
    persistence_actor::persist(PersistRequest::ClearCheckpoint);
    persistence_actor::persist(PersistRequest::Shutdown);

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
        execute!(terminal.backend_mut(), DisableBracketedPaste)?;
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
    usage: &Usage,
    duration: Duration,
    error: Option<&str>,
) {
    if !app.hooks.has_hooks_for_event(HookEvent::TurnEnd) {
        return;
    }

    let context = app.base_hook_context();
    let payload = crate::hooks::turn_end_payload(TurnEndPayloadInput {
        context: &context,
        turn_id: app.runtime_turn_id.as_deref(),
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
            let _ = execute!(stdout, DisableBracketedPaste);
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

fn build_engine_config(app: &App, config: &Config) -> EngineConfig {
    let provider = app.api_provider;
    let max_subagents = app.max_subagents.clamp(1, crate::config::MAX_SUBAGENTS);
    EngineConfig {
        model: app.model.clone(),
        workspace: app.workspace.clone(),
        allow_shell: app.allow_shell,
        trust_mode: app.trust_mode,
        notes_path: config.notes_path(),
        mcp_config_path: config.mcp_config_path(),
        skills_dir: app.skills_dir.clone(),
        skills_scan_codewhale_only: app.skills_scan_codewhale_only,
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
        launch_concurrency: config.launch_concurrency_for_provider(provider),
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
        subagent_api_timeout: Duration::from_secs(
            config.subagent_api_timeout_secs_for_provider(provider),
        ),
        stream_chunk_timeout: Duration::from_secs(app.stream_chunk_timeout_secs),
        subagent_heartbeat_timeout: Duration::from_secs(
            config.subagent_heartbeat_timeout_secs_for_provider(provider),
        ),
        prefer_bwrap: config.prefer_bwrap.unwrap_or(false),
        memory_enabled: config.memory_enabled(),
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
    }
}

fn configured_instruction_sources(config: &Config) -> Vec<prompts::InstructionSource> {
    config
        .instructions_paths()
        .into_iter()
        .map(Into::into)
        .collect()
}

fn build_app_system_prompt(app: &App, config: &Config) -> SystemPrompt {
    let instructions = configured_instruction_sources(config);
    prompts::system_prompt_for_mode_with_context_skills_and_session(
        &app.workspace,
        None,
        None,
        Some(&instructions),
        prompts::PromptSessionContext {
            user_memory_block: None,
            goal_objective: app.hunt.quarry.as_deref(),
            project_context_pack_enabled: config.project_context_pack_enabled(),
            locale_tag: app.ui_locale.tag(),
            translation_enabled: app.translation_enabled,
            model_id: &app.model,
            context_window_override: Some(
                provider_capability(app.api_provider, &app.model).context_window,
            ),
            show_thinking: app.show_thinking,
            verbosity: app.verbosity.as_deref(),
            skills_scan_codewhale_only: app.skills_scan_codewhale_only,
        },
    )
}

/// How long after a task finishes it should still appear in the Work
/// sidebar even if its `ended_at` predates the current TUI session.
///
/// Tasks completing during the current session always show (until the
/// next session boundary). Tasks that completed shortly before the
/// session also show, so users coming back to a terminal see "you just
/// finished X". Anything older than this window is hidden — preventing
/// the sidebar from accumulating indefinitely (bug #1913).
const WORK_SIDEBAR_RECENT_COMPLETED_TTL: chrono::Duration = chrono::Duration::hours(2);

/// Choose which durable-task summaries should appear in the Work
/// sidebar's Tasks panel.
///
/// Active tasks (`Queued`/`Running`) are always included. Terminal
/// tasks (`Completed`/`Failed`/`Canceled`) are kept only if their
/// `ended_at` falls within the "recent" window — defined as either:
///
/// - within the current TUI session (`ended_at >= session_started_at`), or
/// - within `recent_ttl` of `now` (so a task that finished a few
///   minutes before the session started still shows).
///
/// Anything older than that — including the multi-day-old completed
/// tasks reported in bug #1913 — is excluded so the sidebar does not
/// accumulate indefinitely across sessions.
///
/// A terminal task missing `ended_at` is treated as not-recent and
/// dropped: durable tasks always stamp `ended_at` when they reach a
/// terminal state, so absence of it indicates a record from a much
/// older schema and isn't worth surfacing.
pub(crate) fn select_work_sidebar_tasks(
    tasks: Vec<TaskSummary>,
    session_started_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    recent_ttl: chrono::Duration,
) -> Vec<TaskSummary> {
    let recent_cutoff = now - recent_ttl;
    tasks
        .into_iter()
        .filter(|task| match task.status {
            TaskStatus::Queued | TaskStatus::Running => true,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled => {
                match task.ended_at {
                    Some(ended_at) => ended_at >= session_started_at || ended_at >= recent_cutoff,
                    None => false,
                }
            }
        })
        .collect()
}

async fn refresh_active_task_panel(app: &mut App, task_manager: &SharedTaskManager) {
    let tasks = task_manager.list_tasks(None).await;
    let session_started_at = app.session_started_at;
    let now = chrono::Utc::now();
    let mut entries: Vec<TaskPanelEntry> = select_work_sidebar_tasks(
        tasks,
        session_started_at,
        now,
        WORK_SIDEBAR_RECENT_COMPLETED_TTL,
    )
    .into_iter()
    .map(task_summary_to_panel_entry)
    .collect();

    entries.extend(active_reasoning_task_entries(app));
    entries.extend(active_rlm_task_entries(app));

    if let Some(shell_mgr) = app.runtime_services.shell_manager.as_ref()
        && let Ok(mut mgr) = shell_mgr.lock()
    {
        for job in mgr.list_jobs() {
            if !matches!(job.status, crate::tools::shell::ShellStatus::Running) {
                continue;
            }
            entries.push(TaskPanelEntry {
                id: job.id,
                status: "running".to_string(),
                prompt_summary: format!("shell: {}", job.command),
                duration_ms: Some(job.elapsed_ms),
                kind: TaskPanelEntryKind::Background,
                stale: job.stale,
                elapsed_since_output_ms: job.elapsed_since_output_ms,
                owner_agent_id: job.owner_agent_id,
                owner_agent_name: job.owner_agent_name,
            });
        }
    }

    app.task_panel = entries;
}

fn refresh_shell_exec_live_output(app: &mut App) -> bool {
    let Some(shell_mgr) = app.runtime_services.shell_manager.as_ref().cloned() else {
        return false;
    };
    let jobs = {
        let Ok(mut mgr) = shell_mgr.lock() else {
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
        let Some((task_id, next_status, next_live, next_duration)) =
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
        exec.live_output = next_live;
        exec.duration_ms = Some(next_duration);
        changed = true;
    }
    changed
}

fn shell_exec_live_update(
    app: &App,
    index: usize,
    jobs: &std::collections::HashMap<String, ShellJobSnapshot>,
) -> Option<(String, ToolStatus, Option<String>, u64)> {
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
    if exec.status == next_status
        && exec.live_output == next_live
        && exec.duration_ms == Some(job.elapsed_ms)
    {
        return None;
    }
    Some((task_id.to_string(), next_status, next_live, job.elapsed_ms))
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

fn active_reasoning_task_entries(app: &App) -> Vec<TaskPanelEntry> {
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
        .filter_map(|(idx, entry)| match entry {
            HistoryCell::Thinking {
                streaming: true, ..
            } => Some(TaskPanelEntry {
                id: format!("reasoning-{}", idx + 1),
                status: "running".to_string(),
                prompt_summary: "model reasoning".to_string(),
                duration_ms,
                kind: TaskPanelEntryKind::ModelReasoning,
                stale: false,
                elapsed_since_output_ms: None,
                owner_agent_id: None,
                owner_agent_name: None,
            }),
            _ => None,
        })
        .collect()
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

#[allow(clippy::too_many_lines)]
async fn run_event_loop(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    mut engine_handle: EngineHandle,
    task_manager: SharedTaskManager,
    event_broker: &EventBroker,
    translation_client: Option<Arc<DeepSeekClient>>,
) -> Result<()> {
    // Track streaming state
    let mut current_streaming_text = String::new();
    let (translation_tx, mut translation_rx) =
        tokio::sync::mpsc::unbounded_channel::<TranslationEvent>();
    let mut pending_translations = 0usize;
    let mut pending_thinking_translations = 0usize;
    let mut last_queue_state = (app.queued_messages.clone(), app.queued_draft.clone());
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
    let mut web_config_session: Option<WebConfigSession> = None;
    let mut prev_input_snapshot = String::new();
    let mut terminal_paused_at: Option<Instant> = None;
    let mut force_terminal_repaint = false;
    let mut draws_since_last_full_repaint: u64 = 0;
    // FocusGained debounce: some terminal emulators (e.g. Tabby) re-trigger
    // FocusGained when we re-arm focus-change reporting inside
    // recover_terminal_modes, creating a tight repaint loop. Skip
    // mode recovery (but still mark a repaint) within the debounce window.
    const FOCUS_RECOVERY_DEBOUNCE: Duration = Duration::from_millis(200);
    let mut last_focus_recovery = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);
    #[cfg(target_os = "windows")]
    let mut terminal_input = TerminalInputPump::spawn()?;
    #[cfg(not(target_os = "windows"))]
    let terminal_input = TerminalInputPump::spawn()?;
    let mut pending_terminal_events: VecDeque<Event> = VecDeque::new();
    let mut last_terminal_input_recovery = Instant::now()
        .checked_sub(TERMINAL_INPUT_RECOVERY_COOLDOWN)
        .unwrap_or_else(Instant::now);

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

    loop {
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
            refresh_active_task_panel(app, &task_manager).await;
            if refresh_shell_exec_live_output(app) {
                app.needs_redraw = true;
            }
            last_task_refresh = Instant::now();
            app.needs_redraw = true;
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
        let mut fallback_after_engine_error: Option<ApiProvider> = None;
        {
            let mut rx = engine_handle.rx_event.write().await;
            let mut progress_redraw_agents: HashSet<String> = HashSet::new();
            for _ in 0..MAX_ENGINE_EVENTS_PER_DRAIN {
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
                        app.streaming_output_token_estimate =
                            estimate_output_tokens_from_text(&current_streaming_text);
                        let index = ensure_streaming_assistant_history_cell(app);
                        app.streaming_state.push_content(0, &sanitized);
                        let committed = app.streaming_state.commit_text(0);
                        if !committed.is_empty() {
                            append_streaming_text(app, index, &committed);
                            transcript_batch_updated = true;
                        }
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
                            let remaining = app.streaming_state.finalize_block_text(0);
                            if !remaining.is_empty() {
                                append_streaming_text(app, index, &remaining);
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

                        let entry_idx = streaming_thinking::ensure_active_entry(app);
                        app.streaming_state.push_content(0, &sanitized);
                        let committed = app.streaming_state.commit_text(0);
                        if !committed.is_empty() {
                            if app.translation_enabled {
                                streaming_thinking::set_placeholder(app, entry_idx);
                            } else {
                                streaming_thinking::append(app, entry_idx, &committed);
                            }
                            transcript_batch_updated = true;
                        }
                    }
                    EngineEvent::ThinkingComplete { .. } => {
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
                    EngineEvent::ToolCallComplete { id, name, result } => {
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
                        ) {
                            refresh_active_task_panel(app, &task_manager).await;
                            last_task_refresh = Instant::now();
                        }
                        if matches!(name.as_str(), "agent") {
                            subagent_list_refresh_requested = true;
                        }
                    }
                    EngineEvent::TurnStarted { turn_id } => {
                        app.suppress_stream_events_until_turn_complete = false;
                        app.is_loading = true;
                        app.offline_mode = false;
                        app.turn_error_posted = false;
                        app.prompt_suggestion = None;
                        app.prompt_suggestion_gen
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        app.dispatch_started_at = None;
                        current_streaming_text.clear();
                        app.streaming_output_token_estimate = 0;
                        app.streaming_state.reset();
                        app.streaming_message_index = None;
                        app.streaming_thinking_active_entry = None;
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
                        app.session.last_tool_catalog = tool_catalog;
                        app.session.last_base_url = base_url;
                        let was_locally_cancelled = app.suppress_stream_events_until_turn_complete;
                        app.suppress_stream_events_until_turn_complete = false;
                        app.active_allowed_tools = None;
                        if app.paused_quarry.is_none() {
                            app.pausable = false;
                            app.paused = false;
                        }
                        if !matches!(status, crate::core::events::TurnOutcomeStatus::Completed)
                            || draws_since_last_full_repaint >= PERIODIC_FULL_REPAINT_EVERY_N
                        {
                            force_terminal_repaint = true;
                        }
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
                                "completed".to_string()
                            }
                            crate::core::events::TurnOutcomeStatus::Interrupted => {
                                "interrupted".to_string()
                            }
                            crate::core::events::TurnOutcomeStatus::Failed => "failed".to_string(),
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
                        app.push_turn_cache_record(crate::tui::app::TurnCacheRecord {
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
                        let pricing_model = if app.auto_model {
                            app.last_effective_model.as_deref().unwrap_or(&app.model)
                        } else {
                            &app.model
                        };
                        let turn_cost = crate::pricing::calculate_turn_cost_estimate_from_usage(
                            pricing_model,
                            &usage,
                        );
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
                        if let Ok(manager) = SessionManager::default_location() {
                            let session = build_session_snapshot(app, &manager);
                            app.current_session_id = Some(session.metadata.id.clone());
                            persistence_actor::persist(PersistRequest::SessionSnapshot(session));
                        }
                        persistence_actor::persist(PersistRequest::ClearCheckpoint);

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
                            app.plan_prompt_pending = true;
                            app.add_message(HistoryCell::System {
                                content: plan_next_step_prompt(),
                            });
                            if app.view_stack.top_kind() != Some(ModalKind::PlanPrompt) {
                                let plan = Some(app.plan_state.lock().await.snapshot());
                                let todos = Some(app.todos.lock().await.snapshot());
                                app.view_stack
                                    .push(PlanPromptView::new(plan).with_todos(todos));
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

                        execute_turn_end_observer_hook(app, &usage, turn_elapsed, error.as_deref());

                        if queued_to_send.is_none() {
                            queued_to_send = app.pop_queued_message();
                        }
                    }
                    EngineEvent::Error {
                        envelope,
                        recoverable: _,
                    } => {
                        let provider_before_error = app.api_provider;
                        let rollback_after_auth_failure =
                            matches!(
                                envelope.category,
                                crate::error_taxonomy::ErrorCategory::Authentication
                            ) && app.pending_provider_switch.is_some();
                        apply_engine_error_to_app(app, envelope);
                        if app.api_provider != provider_before_error && app.is_fallback_active() {
                            fallback_after_engine_error = Some(provider_before_error);
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
                        app.current_session_id = Some(session_id);
                        app.api_messages = messages;
                        app.system_prompt = system_prompt;
                        if app.auto_model {
                            app.last_effective_model = Some(model);
                        } else {
                            app.set_model_selection(model);
                        }
                        app.update_model_compaction_budget();
                        app.workspace = workspace;
                        if (app.is_loading || app.is_compacting || app.is_purging)
                            && let Ok(manager) = SessionManager::default_location()
                        {
                            let session = build_session_snapshot(app, &manager);
                            app.session_title = Some(session.metadata.title.clone());
                            persistence_actor::persist(PersistRequest::Checkpoint(session));
                        } else if app.session_title.is_none() {
                            // First turn on a brand-new session: persist hasn't fired yet so
                            // read the title from the session file if it already exists,
                            // otherwise fall back to deriving from messages.
                            let persisted = app
                                .current_session_id
                                .as_deref()
                                .and_then(|id| {
                                    SessionManager::default_location()
                                        .ok()?
                                        .load_session(id)
                                        .ok()
                                })
                                .map(|s| s.metadata.title);
                            app.session_title =
                                persisted.or_else(|| derive_session_title(&app.api_messages));
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
                        let prompt_summary = summarize_tool_output(&prompt);
                        execute_subagent_observer_hook(
                            app,
                            HookEvent::SubagentSpawn,
                            &id,
                            "prompt",
                            &prompt,
                        );
                        app.agent_progress
                            .insert(id.clone(), format!("starting: {prompt_summary}"));
                        app.agent_progress_meta.insert(
                            id.clone(),
                            crate::tui::app::AgentProgressMeta {
                                parent_run_id,
                                spawn_depth,
                            },
                        );
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
                        parent_run_id,
                        spawn_depth,
                    } => {
                        let display = friendly_subagent_progress(app, &id, &status);
                        if is_noisy_subagent_progress(&status) {
                            app.agent_progress
                                .entry(id.clone())
                                .or_insert_with(|| display.clone());
                        } else {
                            app.agent_progress.insert(id.clone(), display.clone());
                        }
                        app.agent_progress_meta.insert(
                            id.clone(),
                            crate::tui::app::AgentProgressMeta {
                                parent_run_id,
                                spawn_depth,
                            },
                        );
                        if app.agent_activity_started_at.is_none() {
                            app.agent_activity_started_at = Some(Instant::now());
                        }
                        // #3030: progress can arrive before AgentSpawned is
                        // observed — assign the stable label on first sight.
                        let label = app.ensure_agent_label(&id);
                        app.status_message = Some(format!("{label}: {display}"));
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
                        app.agent_progress_meta.remove(&id);
                        // #3030: stable label with raw-id fallback.
                        let label = app.agent_display_label(&id);
                        app.status_message = Some(format!(
                            "{label} completed: {}",
                            summarize_tool_output(&result)
                        ));
                        let should_recapture_terminal =
                            !has_other_running_subagents && app.use_alt_screen;
                        if !has_other_running_subagents
                            && let Some((method, threshold, include_summary)) =
                                notifications::settings(config)
                        {
                            let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                            let msg = notifications::subagent_completion_message(
                                app.ui_locale,
                                &id,
                                &result,
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
                    EngineEvent::AgentList { agents } => {
                        let mut sorted = agents.clone();
                        sort_subagents_in_place(&mut sorted);
                        sorted.retain(|a| !a.from_prior_session);
                        app.subagent_cache = sorted.clone();
                        reconcile_subagent_activity_state(app);
                        let view_agents = subagent_view_agents(app, &app.subagent_cache);
                        if app.view_stack.update_subagents(&view_agents) {
                            app.status_message =
                                Some(format!("Sub-agents: {} total", view_agents.len()));
                        }
                        // Individual spawn/complete events already log to history;
                        // full list available via /agents command.
                    }
                    EngineEvent::SubAgentMailbox { seq, message } => {
                        let should_refresh_subagents =
                            subagent_message_refreshes_workspace_context(&message);
                        let updated_transcript = handle_subagent_mailbox(app, seq, &message);
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
                        let session_denied = is_session_denied_for_key(app, &approval_key);
                        if session_denied {
                            // The user already said no to this exact tool /
                            // approval key in this session; auto-deny so the
                            // model's retry loop doesn't keep re-prompting
                            // (#360).
                            log_sensitive_event(
                                "tool.approval.auto_deny_session",
                                serde_json::json!({
                                    "tool_name": tool_name,
                                    "approval_key": approval_key,
                                    "session_id": app.current_session_id,
                                }),
                            );
                            let _ = engine_handle.deny_tool_call(id.clone()).await;
                        } else if should_auto_approve_approval_request(
                            app,
                            &tool_name,
                            &approval_grouping_key,
                            approval_force_prompt,
                        ) {
                            log_sensitive_event(
                                "tool.approval.auto_approve",
                                serde_json::json!({
                                    "tool_name": tool_name,
                                    "approval_key": approval_key,
                                    "session_id": app.current_session_id,
                                    "mode": app.mode.label(),
                                }),
                            );
                            let _ = engine_handle.approve_tool_call(id.clone()).await;
                        } else if app.approval_mode == ApprovalMode::Never {
                            log_sensitive_event(
                                "tool.approval.auto_deny",
                                serde_json::json!({
                                    "tool_name": tool_name,
                                    "session_id": app.current_session_id,
                                    "mode": app.mode.label(),
                                }),
                            );
                            let _ = engine_handle.deny_tool_call(id.clone()).await;
                            app.status_message =
                                Some(format!("Blocked tool '{tool_name}' (approval_mode=never)"));
                        } else {
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
                                let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
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
                    EngineEvent::UserInputRequired { id, request } => {
                        app.view_stack.push(UserInputView::new(id.clone(), request));
                        if let Some((method, _, _)) = crate::tui::notifications::settings(config) {
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
                    EngineEvent::ElevationRequired {
                        tool_id,
                        tool_name,
                        command,
                        denial_reason,
                        blocked_network,
                        blocked_write,
                    } => {
                        // In YOLO mode, auto-elevate to full access
                        if app.approval_mode == ApprovalMode::Auto {
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
            }
        }
        if let Some(previous_provider) = fallback_after_engine_error {
            apply_provider_fallback_switch(app, &mut engine_handle, config, previous_provider)
                .await;
        }
        if let Some(rollback_warning) = respawn_after_provider_rollback {
            let _ = engine_handle.send(Op::Shutdown).await;
            let engine_config = build_engine_config(app, config);
            engine_handle = spawn_engine(engine_config, config);
            if !app.api_messages.is_empty() {
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id: app.current_session_id.clone(),
                        messages: app.api_messages.clone(),
                        system_prompt: app.system_prompt.clone(),
                        system_prompt_override: false,
                        model: app.model.clone(),
                        workspace: app.workspace.clone(),
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
        if let Some(index) = app.streaming_message_index {
            let committed = app.streaming_state.commit_text(0);
            if !committed.is_empty() {
                append_streaming_text(app, index, &committed);
                transcript_batch_updated = true;
            }
        } else if let Some(entry_idx) = app.streaming_thinking_active_entry {
            let committed = app.streaming_state.commit_text(0);
            if !committed.is_empty() {
                if app.translation_enabled {
                    streaming_thinking::set_placeholder(app, entry_idx);
                } else {
                    streaming_thinking::append(app, entry_idx, &committed);
                }
                transcript_batch_updated = true;
            }
        }
        if transcript_batch_updated {
            app.mark_history_updated();
        }
        if received_engine_event {
            app.needs_redraw = true;
        }
        // #freeze: one trailing-edge sub-agent list refresh per drain, no
        // matter how many spawn/complete/mailbox events arrived this batch.
        if subagent_list_refresh_requested {
            let _ = engine_handle.send(Op::ListSubAgents).await;
        }

        if let Some(next) = queued_to_send {
            if let Err(err) = dispatch_user_message(app, config, &engine_handle, next.clone()).await
            {
                app.queue_message(next);
                app.status_message = Some(format!(
                    "Dispatch failed ({err}); kept {} queued message(s)",
                    app.queued_message_count()
                ));
            }

            app.needs_redraw = true;
        }

        let queue_state = (app.queued_messages.clone(), app.queued_draft.clone());
        if queue_state != last_queue_state {
            persist_offline_queue_state(app);
            last_queue_state = queue_state;
            app.needs_redraw = true;
        }

        if !app.view_stack.is_empty() {
            let events = app.view_stack.tick();
            if !events.is_empty() {
                app.needs_redraw = true;
            }
            if handle_view_events(
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

        let has_running_agents = running_agent_count(app) > 0;
        if reconcile_turn_liveness(app, Instant::now(), has_running_agents) {
            app.needs_redraw = true;
        }
        if (app.is_loading || has_running_agents || app.is_compacting || app.is_purging)
            && last_status_frame.elapsed()
                >= Duration::from_millis(status_animation_interval_ms(app))
        {
            if streaming_thinking::animate_pending_translation(
                app,
                pending_thinking_translations > 0,
            ) {
                app.mark_history_updated();
            }
            if !app.low_motion && history_has_live_motion(&app.history) {
                app.mark_history_updated();
            }
            app.needs_redraw = true;
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

        // Sync low-motion flag into the frame-rate limiter and streaming
        // chunking policy. Low-motion mode drops the frame cap to 30 FPS
        // and forces Smooth-only chunking so the display stays calm.
        frame_rate_limiter.set_low_motion(app.low_motion);
        app.streaming_state.set_low_motion(app.low_motion);

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
            let was_full_repaint = force_terminal_repaint;
            draw_app_frame_inner(terminal, app, force_terminal_repaint)?;
            force_terminal_repaint = false;
            if was_full_repaint {
                draws_since_last_full_repaint = 0;
            } else {
                draws_since_last_full_repaint = draws_since_last_full_repaint.saturating_add(1);
            }
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
                #[cfg(target_os = "windows")]
                match terminal_input.restart_detached() {
                    Ok(()) => {
                        app.push_status_toast(
                            "Recovered terminal input after a stalled Windows console poll.",
                            StatusToastLevel::Warning,
                            None,
                        );
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "failed to restart terminal input pump");
                        app.push_status_toast(
                            "Terminal input stalled; recovery failed. Restart CodeWhale if keys stop responding.",
                            StatusToastLevel::Error,
                            None,
                        );
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    app.push_status_toast(
                        "Terminal input heartbeat stalled; terminal modes were refreshed.",
                        StatusToastLevel::Warning,
                        None,
                    );
                }
                terminal_input.mark_alive();
                last_terminal_input_recovery = now;
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
                    onboarding::sync_api_key_validation_status(app, false);
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
                draw_app_frame_inner(terminal, app, true)?;
                draws_since_last_full_repaint = 0;
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
                if handle_view_events(
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
            // modifier flags (#2938).
            let mapped = crate::tui::composer_ui::normalize_macos_modifiers(key.modifiers);
            key.modifiers = mapped;

            // Decision card keyboard routing (v0.8.43 truth-surface).
            // When a card is active, number keys 1-9 select options,
            // j/k or Up/Down navigate, and Enter confirms.
            // Only route keys to the decision card when no other modal
            // (Help, Config, Pager, etc.) is on top of the view stack (#2005).
            if app.view_stack.is_empty()
                && let Some(card) = app.decision_card.as_mut()
            {
                match key.code {
                    KeyCode::Char(c @ '1'..='9') => {
                        let n = (c as u8 - b'1' + 1) as usize;
                        card.select_number(n);
                        card.confirm();
                        app.status_message = card
                            .confirmed_label()
                            .map(|label| format!("Selected: {label}"));
                        app.decision_card = None;
                        app.needs_redraw = true;
                    }
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
                submit_initial_input_if_ready(app, config, &engine_handle).await?;
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
                        app.onboarding = OnboardingState::Welcome;
                        app.api_key_input.clear();
                        app.api_key_cursor = 0;
                        app.status_message = None;
                    }
                    KeyCode::Esc if app.onboarding == OnboardingState::Language => {
                        app.onboarding = OnboardingState::Welcome;
                        app.status_message = None;
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
                        }
                        OnboardingState::ApiKey => {
                            let key = app.api_key_input.trim().to_string();
                            if let onboarding::ApiKeyValidation::Reject(message) =
                                onboarding::validate_api_key_for_onboarding(&key)
                            {
                                app.status_message = Some(message);
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
                                    // Recreate the engine so it picks up the newly saved key
                                    // without requiring a full process restart.
                                    let _ = engine_handle.send(Op::Shutdown).await;
                                    // Stamp the new key on the long-lived
                                    // `Config` reference so any future clone
                                    // (e.g. a subsequent /provider switch)
                                    // sees it; the explicit-override path
                                    // in `deepseek_api_key` (#343) makes
                                    // this win immediately.
                                    config.api_key = Some(key.clone());
                                    let mut refreshed_config = config.clone();
                                    refreshed_config.api_key = Some(key);
                                    let engine_config = build_engine_config(app, &refreshed_config);
                                    engine_handle = spawn_engine(engine_config, &refreshed_config);
                                    app.offline_mode = false;
                                    app.api_key_env_only = false;

                                    if !app.api_messages.is_empty() {
                                        let _ = engine_handle
                                            .send(Op::SyncSession {
                                                session_id: app.current_session_id.clone(),
                                                messages: app.api_messages.clone(),
                                                system_prompt: app.system_prompt.clone(),
                                                system_prompt_override: false,
                                                model: app.model.clone(),
                                                workspace: app.workspace.clone(),
                                            })
                                            .await;
                                    }

                                    onboarding::advance_onboarding_after_language(app);
                                }
                                Err(e) => {
                                    app.status_message = Some(e.to_string());
                                }
                            }
                        }
                        OnboardingState::TrustDirectory => {}
                        OnboardingState::Tips => {
                            app.finish_onboarding();
                        }
                        OnboardingState::None => {}
                    },
                    KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('1')
                        if app.onboarding == OnboardingState::TrustDirectory =>
                    {
                        match onboarding::mark_trusted(&app.workspace) {
                            Ok(_) => {
                                app.trust_mode = true;
                                app.hooks = HookExecutor::new(
                                    crate::hooks::HooksConfig::load_with_project(
                                        config.hooks_config(),
                                        &app.workspace,
                                    ),
                                    app.workspace.clone(),
                                );
                                app.runtime_services.hook_executor =
                                    Some(std::sync::Arc::new(app.hooks.clone()));
                                app.status_message = None;
                                if app.onboarding_workspace_trust_gate {
                                    app.onboarding_workspace_trust_gate = false;
                                    app.onboarding = OnboardingState::None;
                                } else {
                                    app.onboarding = OnboardingState::Tips;
                                }
                            }
                            Err(err) => {
                                app.status_message =
                                    Some(format!("Failed to trust workspace: {err}"));
                            }
                        }
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('2')
                        if app.onboarding == OnboardingState::TrustDirectory =>
                    {
                        let _ = engine_handle.send(Op::Shutdown).await;
                        return Ok(());
                    }
                    KeyCode::Backspace if app.onboarding == OnboardingState::ApiKey => {
                        app.delete_api_key_char();
                        onboarding::sync_api_key_validation_status(app, false);
                    }
                    KeyCode::Char('h')
                        if key_shortcuts::is_ctrl_h_backspace(&key)
                            && app.onboarding == OnboardingState::ApiKey =>
                    {
                        app.delete_api_key_char();
                        onboarding::sync_api_key_validation_status(app, false);
                    }
                    _ if key_shortcuts::is_paste_shortcut(&key)
                        && app.onboarding == OnboardingState::ApiKey =>
                    {
                        // Cmd+V / Ctrl+V paste (bracketed paste handled above)
                        app.paste_api_key_from_clipboard();
                        onboarding::sync_api_key_validation_status(app, false);
                    }
                    KeyCode::Char(c)
                        if app.onboarding == OnboardingState::ApiKey
                            && key_shortcuts::is_text_input_key(&key) =>
                    {
                        app.insert_api_key_char(c);
                        onboarding::sync_api_key_validation_status(app, false);
                    }
                    _ => {}
                }
                continue;
            }

            if key.code == KeyCode::F(1) {
                if app.view_stack.top_kind() == Some(ModalKind::Help) {
                    app.view_stack.pop();
                } else {
                    app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
                }
                continue;
            }

            if key.code == KeyCode::Char('/') && key.modifiers.contains(KeyModifiers::CONTROL) {
                if app.view_stack.top_kind() == Some(ModalKind::Help) {
                    app.view_stack.pop();
                } else {
                    app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
                }
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
                app.view_stack
                    .push(CommandPaletteView::new(build_command_palette_entries(
                        app.ui_locale,
                        &app.skills_dir,
                        app.skills_scan_codewhale_only,
                        &app.workspace,
                        &app.mcp_config_path,
                        app.mcp_snapshot.as_ref(),
                    )));
                continue;
            }

            // y / Y in the Tasks sidebar: yank the current turn id (y)
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
                    let _ = engine_handle.send(Op::CompactContext).await;
                }
                app.needs_redraw = true;
                continue;
            }

            if matches!(key.code, KeyCode::Char('b') | KeyCode::Char('B'))
                && key_shortcuts::has_control_like_modifier(key.modifiers)
                && app.view_stack.is_empty()
            {
                // #3032: Ctrl+B directly backgrounds the active foreground
                // shell command instead of opening a two-step shell-control
                // menu.  When nothing is backgroundable, the status message
                // tells the user what's going on.
                request_foreground_shell_background(app);
                app.needs_redraw = true;
                continue;
            }

            if matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
                && key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER)
                && app.view_stack.is_empty()
            {
                open_context_inspector(app);
                continue;
            }

            if !app.view_stack.is_empty() {
                let events = app.view_stack.handle_key(key);
                app.needs_redraw = true;
                if handle_view_events(
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

            if key_shortcuts::is_macos_option_v_legacy_key(&key) {
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
                // Bare `v` / `V` no longer opens the tool-details pager — that
                // path is owned exclusively by `Alt+V` at the lower arm, so
                // the letter `v` is freely usable as the first character of
                // a message. `details_shortcut_modifiers` previously allowed
                // empty/Shift here, eating the keystroke on empty composers.
                KeyCode::Char('o')
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && app.input.is_empty()
                        && open_activity_detail_pager(app) =>
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
                    if key.modifiers == KeyModifiers::CONTROL =>
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
                    app.status_message = Some("Sidebar focus: tasks".to_string());
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
                    app.status_message = Some("Sidebar focus: tasks".to_string());
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
                    app.view_stack.push(SessionPickerView::new(&app.workspace));
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
                            let prompt_restored = app.restore_last_submitted_prompt_if_empty();
                            app.status_message = Some(
                                if prompt_restored {
                                    "Request cancelled; prompt restored to composer"
                                } else {
                                    "Request cancelled"
                                }
                                .to_string(),
                            );
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
                                }
                                app.active_allowed_tools = None;
                                app.hunt.quarry = None;
                                app.hunt.tokens_used = 0;
                                app.hunt.time_used_seconds = 0;
                                app.hunt.continuation_count = 0;
                                app.status_message = Some("Paused command cancelled".to_string());
                            } else {
                                engine_handle.cancel();
                                mark_active_turn_cancelled_locally(app);
                                current_streaming_text.clear();
                                app.status_message = Some("Request cancelled".to_string());
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
                        sync_mode_update(&engine_handle, app.mode).await;
                    }
                    if app.model != prior_model {
                        let _ = engine_handle
                            .send(Op::SetModel {
                                model: app.model.clone(),
                                mode: app.mode,
                            })
                            .await;
                    }
                }
                KeyCode::BackTab => {
                    app.cycle_effort();
                }
                // Transcript-nav shortcuts now require Alt, leaving the bare
                // letters free to insert as text. Before v0.8.30, bare `g`,
                // `G`, `[`, `]`, `?`, `l`, and `v` on an empty composer were
                // hijacked for navigation — typing "good" yielded "ood" with
                // no whale and no warning. The Alt-prefixed shortcuts mirror
                // the Alt+R / Alt+V / Alt+C pattern already in use. Shift is
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
                // `Alt+?` opens the searchable help overlay (#93). F1 and
                // Ctrl+/ are also bound; bare `?` is reserved as text input
                // so users can start a message with "?" without losing the
                // first character.
                KeyCode::Char('?')
                    if key_shortcuts::alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open =>
                {
                    if app.view_stack.top_kind() != Some(ModalKind::Help) {
                        app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
                    }
                    continue;
                }
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
                            if let Err(err) =
                                steer_user_message(app, &engine_handle, queued.clone()).await
                            {
                                app.queue_message(queued);
                                app.status_message = Some(format!(
                                    "Steer failed ({err}); {} queued follow-up(s) — /queue send <n>",
                                    app.queued_message_count()
                                ));
                            }
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
                                if let Err(err) =
                                    steer_user_message(app, &engine_handle, queued.clone()).await
                                {
                                    app.queue_message(queued);
                                    app.status_message = Some(format!(
                                        "Steer failed ({err}); {} queued follow-up(s) — /queue send <n>",
                                        app.queued_message_count()
                                    ));
                                }
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
                        if config.memory_enabled() && is_memory_quick_add(&input) {
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
                KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+O: spawn $EDITOR on the composer contents (#91).
                    // Only fires when no modal is active (the !view_stack
                    // branch above already returns early in that case) and
                    // the composer is the focused input target. We accept the
                    // shortcut whether or not a model turn is streaming —
                    // editing the buffer never disturbs in-flight work.
                    let seed = app.input.clone();
                    match super::external_editor::spawn_editor_for_input(
                        terminal,
                        app.use_alt_screen,
                        app.use_mouse_capture,
                        app.use_bracketed_paste,
                        &seed,
                    ) {
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
                KeyCode::Char('s') | KeyCode::Char('S')
                    if key.modifiers == KeyModifiers::CONTROL =>
                {
                    if send_ctrl_s_queued_message_now(app, config, &engine_handle).await? {
                        continue;
                    }
                    // #440: park the current draft to the persistent
                    // stash and clear the composer. Empty composers
                    // are a no-op so a stray Ctrl+S can't pollute the
                    // file. Surface a toast so the user sees the
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
                KeyCode::Char('v') | KeyCode::Char('V')
                    if key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    open_tool_details_pager(app);
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

fn hotbar_slot_from_key(app: &App, key: &event::KeyEvent) -> Option<u8> {
    if app.onboarding != OnboardingState::None || !app.view_stack.is_empty() {
        return None;
    }

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
        return Some(slot);
    }

    None
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

    if let Ok(mut settings) = Settings::load() {
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
            .and_then(|effort| effort.api_value_for_provider(app.api_provider))
            .map(str::to_string)
    } else {
        app.reasoning_effort
            .api_value_for_provider(app.api_provider)
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

// `format_*` chip/message builders moved to `tui/format_helpers.rs`.

fn build_session_snapshot(app: &App, manager: &SessionManager) -> SavedSession {
    let model = app.model_selection_for_persistence();
    if let Some(ref existing_id) = app.current_session_id
        && let Ok(existing) = manager.load_session(existing_id)
    {
        let mut updated = update_session(
            existing,
            &app.api_messages,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
        );
        updated.metadata.model = model;
        updated.metadata.mode = Some(app.mode.as_setting().to_string());
        app.sync_cost_to_metadata(&mut updated.metadata);
        updated.context_references = app.session_context_references.clone();
        updated.artifacts = app.session_artifacts.clone();
        updated
    } else {
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
        app.sync_cost_to_metadata(&mut session.metadata);
        session.context_references = app.session_context_references.clone();
        session.artifacts = app.session_artifacts.clone();
        session
    }
}

fn queued_ui_to_session(msg: &QueuedMessage) -> QueuedSessionMessage {
    QueuedSessionMessage {
        display: msg.display.clone(),
        skill_instruction: msg.skill_instruction.clone(),
    }
}

fn queued_session_to_ui(msg: QueuedSessionMessage) -> QueuedMessage {
    QueuedMessage {
        display: msg.display,
        skill_instruction: msg.skill_instruction,
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
                now.saturating_duration_since(last_activity) > TURN_STALL_WATCHDOG_TIMEOUT
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
            "Tool stalled with no progress for 15m — recovered; the command may still be running in the background. Use exec_shell_cancel or retry.",
            StatusToastLevel::Error,
        );
        return true;
    }

    false
}

/// #2739: persist the current in-memory session state before a recovery or
/// cancellation path clears turn bookkeeping. Without this snapshot, the
/// just-finalised partial turn lives only in `app.api_messages` and is never
/// written to disk, so `--continue` loads the *previous* save — effectively
/// losing the entire in-progress turn.
fn persist_recovery_snapshot(app: &mut App) {
    if let Ok(manager) = SessionManager::default_location() {
        let session = build_session_snapshot(app, &manager);
        if app.current_session_id.is_none() {
            app.current_session_id = Some(session.metadata.id.clone());
        }
        persistence_actor::persist(PersistRequest::SessionSnapshot(session));
    }
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
        app.onboarding = OnboardingState::ApiKey;
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
        previous_config,
        previous_onboarding,
        previous_onboarding_needs_api_key,
        previous_api_key_env_only,
    } = pending;

    *config = previous_config;
    app.api_provider = previous_provider;
    app.set_model_selection(previous_model.clone());
    app.provider_models
        .insert(previous_provider.as_str().to_string(), previous_model);
    app.model_ids_passthrough = previous_model_ids_passthrough;
    app.update_model_compaction_budget();
    app.clear_model_scoped_telemetry();
    app.offline_mode = false;
    app.onboarding = previous_onboarding;
    app.onboarding_needs_api_key = previous_onboarding_needs_api_key;
    app.api_key_env_only = previous_api_key_env_only;

    let persistence_error = (|| -> anyhow::Result<()> {
        crate::config_persistence::persist_root_string_key(
            app.config_path.as_deref(),
            "provider",
            previous_provider.as_str(),
        )?;
        let mut settings = crate::settings::Settings::load()?;
        settings.default_provider = Some(previous_provider.as_str().to_string());
        settings.set_model_for_provider(
            previous_provider.as_str(),
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
    })()
    .err()
    .map(|err| format!("provider rollback not fully persisted: {err}"));

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

    if matches!(name, "run_tests" | "run_verifiers" | "task_gate_run") {
        return crate::core::engine::compact_tool_result_for_context(&app.model, name, output);
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

    crate::core::engine::compact_tool_result_for_context(&app.model, name, output)
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
    QueuedMessage::new(input, skill_instruction)
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
        dispatch_user_message(app, config, engine_handle, queued).await?;
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
    app.queue_message(queued);
    app.status_message = Some(format!(
        "{} queued follow-up(s) — ↑ edit last, /queue send <n>",
        app.queued_message_count()
    ));
    true
}

fn take_ctrl_s_queued_message(app: &mut App) -> Option<(QueuedMessage, Option<usize>)> {
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

async fn send_ctrl_s_queued_message_now(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
) -> Result<bool> {
    let Some((message, restore_index)) = take_ctrl_s_queued_message(app) else {
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
    } else if let Err(err) =
        dispatch_user_message(app, config, engine_handle, message.clone()).await
    {
        restore_queued_message(app, restore_index, message);
        app.status_message = Some(format!(
            "Dispatch failed ({err}); kept {} queued follow-up(s)",
            app.queued_message_count()
        ));
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
) -> String {
    // Pass the process CWD explicitly so the resolver's two-pass logic can
    // honor the user's launch directory when it differs from `--workspace`
    // (issue #101 — file mentions silently routing to the wrong root).
    let user_request = crate::tui::file_mention::user_request_with_file_mentions(
        &message.display,
        &app.workspace,
        cwd,
    );
    if let Some(skill_instruction) = message.skill_instruction.as_ref() {
        format!("{skill_instruction}\n\n---\n\nUser request: {user_request}")
    } else {
        user_request
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
        "\n\nCodeWhale paused custom slash command context:\n\
Paused custom slash command: {title}\n\
Paused command: {title}\n\
{instruction}"
    )
}

fn prepare_paused_command_message(
    app: &mut App,
    engine_handle: &EngineHandle,
    user_message: &str,
) -> Option<String> {
    if !app.paused && app.paused_quarry.is_none() {
        engine_handle.set_paused(false);
        return None;
    }

    engine_handle.set_paused(false);
    app.paused = false;

    let Some(quarry) = app
        .paused_quarry
        .clone()
        .or_else(|| app.hunt.quarry.clone())
    else {
        app.pausable = false;
        return None;
    };
    let title = paused_quarry_title(&quarry).to_string();
    if is_resume_message(user_message) {
        app.hunt.quarry = Some(app.paused_quarry.take().unwrap_or(quarry));
        app.pausable = true;
        Some(paused_command_note(&title, true))
    } else {
        app.hunt.quarry = None;
        app.hunt.tokens_used = 0;
        app.hunt.time_used_seconds = 0;
        app.hunt.continuation_count = 0;
        Some(paused_command_note(&title, false))
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

async fn dispatch_user_message(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    mut message: QueuedMessage,
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

    let paused_note = prepare_paused_command_message(app, engine_handle, &message.display);

    // Set immediately to prevent double-dispatch before TurnStarted event arrives.
    let dispatch_started_at = Instant::now();
    app.is_loading = true;
    app.dispatch_started_at = Some(dispatch_started_at);
    app.runtime_turn_status = None;
    app.last_send_at = Some(dispatch_started_at);
    app.last_submitted_prompt = Some(message.display.clone());
    // Clear the previous turn's receipt and evidence.
    app.clear_receipt();
    app.tool_evidence.clear();

    let cwd = std::env::current_dir().ok();
    let references = crate::tui::file_mention::context_references_from_input(
        &message.display,
        &app.workspace,
        cwd.clone(),
    );
    let mut content = queued_message_content_for_app(app, &message, cwd);
    if let Some(note) = paused_note.as_deref() {
        content.push_str(note);
    }
    let auto_selection = if auto_router::should_resolve_auto_model_selection(app) {
        match auto_router::resolve_auto_model_selection(app, config, &message, &content).await {
            Ok(selection) => Some(selection),
            Err(err) => {
                app.is_loading = false;
                app.dispatch_started_at = None;
                app.last_send_at = None;
                app.status_message = Some(format!("Auto model route unavailable: {err}"));
                return Err(err);
            }
        }
    } else {
        None
    };
    let effective_provider = auto_selection
        .as_ref()
        .map(|selection| selection.provider)
        .unwrap_or(app.api_provider);
    let message_index = app.api_messages.len();
    app.system_prompt = Some(build_app_system_prompt(app, config));
    app.add_message(HistoryCell::User {
        content: message.display.clone(),
    });
    let history_cell = app.history.len().saturating_sub(1);
    app.record_context_references(history_cell, message_index, references);
    app.scroll_to_bottom();
    app.api_messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: content.clone(),
            cache_control: None,
        }],
    });
    maybe_warn_context_pressure(app);
    if should_auto_compact_before_send(app) {
        app.status_message =
            Some("Context threshold reached; compacting before send...".to_string());
        let _ = engine_handle.send(Op::CompactContext).await;
    }
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_output_throughput = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    // Persist immediately so abrupt termination can recover this in-flight turn.
    // Offloaded to the persistence actor.
    if let Ok(manager) = SessionManager::default_location() {
        let session = build_session_snapshot(app, &manager);
        persistence_actor::persist(PersistRequest::Checkpoint(session));
    }

    let effective_model = if app.auto_model {
        auto_selection
            .as_ref()
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| {
                crate::model_routing::auto_model_heuristic(&message.display, &app.model)
            })
    } else {
        app.model.clone()
    };

    let auto_controls_reasoning = app.auto_model || app.reasoning_effort == ReasoningEffort::Auto;
    let effective_reasoning_effort = if auto_controls_reasoning {
        let effort = auto_selection
            .as_ref()
            .and_then(|selection| selection.reasoning_effort)
            .unwrap_or_else(|| {
                auto_router::normalize_auto_routed_effort(crate::auto_reasoning::select(
                    false,
                    &message.display,
                ))
            });
        app.last_effective_reasoning_effort = Some(effort);
        effort
            .api_value_for_provider(effective_provider)
            .map(str::to_string)
    } else {
        app.last_effective_reasoning_effort = None;
        app.reasoning_effort
            .api_value_for_provider(effective_provider)
            .map(str::to_string)
    };

    if let Some(selection) = auto_selection.as_ref() {
        if app.auto_model {
            app.last_effective_model = Some(effective_model.clone());
            let mut status = format!(
                "Auto model selected: {} / {effective_model} via {}",
                selection.provider.display_name(),
                selection.source.label()
            );
            if let Some(effort) = app.last_effective_reasoning_effort {
                status.push_str(&format!(
                    "; thinking auto: {}",
                    effort.display_label_for_provider(effective_provider)
                ));
            }
            app.status_message = Some(status);
        }
    } else {
        app.last_effective_model = None;
    }

    if let Err(err) = engine_handle
        .send(Op::SendMessage {
            content,
            mode: app.mode,
            provider: Some(effective_provider),
            model: effective_model,
            goal_objective: app.hunt.quarry.clone(),
            goal_token_budget: app.hunt.token_budget,
            goal_status: app.hunt.verdict.goal_status(),
            reasoning_effort: effective_reasoning_effort,
            reasoning_effort_auto: auto_controls_reasoning,
            auto_model: app.auto_model,
            allow_shell: app.allow_shell,
            trust_mode: app.trust_mode,
            auto_approve: app.mode == AppMode::Yolo,
            approval_mode: app.approval_mode,
            translation_enabled: app.translation_enabled,
            show_thinking: app.show_thinking,
            allowed_tools: app.active_allowed_tools.clone(),
            dynamic_tools: Vec::new(),
            hook_executor: app.runtime_services.hook_executor.clone(),
            verbosity: app.verbosity.clone(),
            provenance: crate::core::ops::UserInputProvenance::ExternalUser,
        })
        .await
    {
        app.is_loading = false;
        app.dispatch_started_at = None;
        app.last_send_at = None;
        return Err(err);
    }

    Ok(())
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
    let Some(objective) = snapshot
        .objective
        .as_deref()
        .map(str::trim)
        .filter(|objective| !objective.is_empty())
    else {
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
    true
}

async fn sync_mode_update(engine_handle: &EngineHandle, mode: AppMode) {
    let _ = engine_handle.send(Op::ChangeMode { mode }).await;
}

async fn apply_mode_update(app: &mut App, engine_handle: &EngineHandle, mode: AppMode) -> bool {
    if app.set_mode(mode) {
        sync_mode_update(engine_handle, mode).await;
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
            trust_mode: app.trust_mode,
            auto_approve: app.mode == AppMode::Yolo,
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
) {
    let _ = engine_handle
        .send(Op::SetModel {
            model: compaction.model.clone(),
            mode,
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
    mut effort: crate::tui::app::ReasoningEffort,
    previous_model: String,
    previous_effort: crate::tui::app::ReasoningEffort,
) {
    let model_is_auto = model.trim().eq_ignore_ascii_case("auto");
    if model_is_auto {
        effort = ReasoningEffort::Auto;
    } else {
        effort = effort.normalize_for_provider(target_provider.unwrap_or(app.api_provider));
    }
    if let Some(target_provider) = target_provider
        && target_provider != app.api_provider
        && !model_is_auto
    {
        switch_provider(
            app,
            engine_handle,
            config,
            target_provider,
            Some(model.clone()),
        )
        .await;
        if app.api_provider != target_provider {
            return;
        }
        apply_picker_effort_choice(app, engine_handle, effort, previous_effort).await;
        return;
    }

    let model_changed = model != previous_model || app.auto_model != model_is_auto;
    let effort_changed = effort != previous_effort;
    if !model_changed && !effort_changed {
        app.status_message = Some(format!(
            "Model unchanged: {model} · thinking {}",
            effort.display_label_for_provider(app.api_provider)
        ));
        return;
    }

    let mut resolved_model = model.clone();
    if model_changed && !model_is_auto {
        let saved_provider_model = config
            .provider_config_for(app.api_provider)
            .and_then(|provider| provider.model.as_deref());
        match resolve_route_candidate(
            app.api_provider,
            Some(&model),
            saved_provider_model,
            Some(config.deepseek_base_url()),
        ) {
            Ok(candidate) => {
                resolved_model = candidate.wire_model_id.as_str().to_string();
            }
            Err(reason) => {
                app.status_message = Some(reason);
                return;
            }
        }
    }

    if model_changed {
        app.set_model_selection(resolved_model.clone());
        app.provider_models.insert(
            app.api_provider.as_str().to_string(),
            resolved_model.clone(),
        );
        app.clear_model_scoped_telemetry();
    }
    if effort_changed {
        app.reasoning_effort = effort;
        app.last_effective_reasoning_effort = None;
    }
    if model_changed || effort_changed {
        app.update_model_compaction_budget();
    }

    // Best-effort persist; surface a status warning if the settings file
    // can't be written rather than aborting the in-memory change.
    let mut persist_warning: Option<String> = None;
    let persist_result = (|| -> anyhow::Result<()> {
        let mut settings = crate::settings::Settings::load()?;
        if model_changed {
            if matches!(
                app.api_provider,
                ApiProvider::Deepseek | ApiProvider::DeepseekCN
            ) {
                settings.set("default_model", &resolved_model)?;
            }
            settings.set_model_for_provider(app.api_provider.as_str(), &resolved_model);
        }
        if effort_changed {
            settings.set(
                "reasoning_effort",
                effort.as_setting_for_provider(app.api_provider),
            )?;
        }
        settings.save()
    })();
    if let Err(err) = persist_result {
        persist_warning = Some(format!("(not persisted: {err})"));
    }

    if model_changed {
        apply_model_and_compaction_update(engine_handle, app.compaction_config(), app.mode).await;
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
    if let Some(warning) = persist_warning {
        summary.push(' ');
        summary.push_str(&warning);
    }
    app.status_message = Some(summary);
}

async fn apply_picker_effort_choice(
    app: &mut App,
    engine_handle: &EngineHandle,
    mut effort: ReasoningEffort,
    previous_effort: ReasoningEffort,
) {
    effort = effort.normalize_for_provider(app.api_provider);
    if effort == previous_effort {
        return;
    }

    app.reasoning_effort = effort;
    app.last_effective_reasoning_effort = None;
    app.update_model_compaction_budget();

    let persist_warning = (|| -> anyhow::Result<()> {
        let mut settings = crate::settings::Settings::load()?;
        settings.set(
            "reasoning_effort",
            effort.as_setting_for_provider(app.api_provider),
        )?;
        settings.save()
    })()
    .err()
    .map(|err| format!(" (not persisted: {err})"));

    apply_model_and_compaction_update(engine_handle, app.compaction_config(), app.mode).await;

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
) {
    let previous_provider = app.api_provider;
    let previous_model = app.model.clone();
    let previous_model_ids_passthrough = app.model_ids_passthrough;
    let previous_config = config.clone();
    app.pending_provider_switch = Some(PendingProviderSwitch {
        previous_provider,
        previous_model: previous_model.clone(),
        previous_model_ids_passthrough,
        previous_config: previous_config.clone(),
        previous_onboarding: app.onboarding,
        previous_onboarding_needs_api_key: app.onboarding_needs_api_key,
        previous_api_key_env_only: app.api_key_env_only,
    });

    let resolved_route = match resolve_runtime_route(config, target, model_override.as_deref()) {
        Ok(route) => route,
        Err(reason) => {
            app.pending_provider_switch = None;
            app.add_message(HistoryCell::System {
                content: format!(
                    "Cannot switch to {}: {reason}\nProvider unchanged ({}).",
                    target.as_str(),
                    previous_provider.as_str()
                ),
            });
            app.status_message = Some(format!(
                "Route rejected before provider switch: {}.",
                target.as_str()
            ));
            return;
        }
    };
    let resolved_endpoint = resolved_route.candidate.endpoint.base_url.clone();
    let next_config = resolved_route.config;
    let new_model = resolved_route.model;

    if let Err(err) = DeepSeekClient::new(&next_config) {
        app.pending_provider_switch = None;
        app.add_message(HistoryCell::System {
            content: format!(
                "Failed to switch provider to {}: {err}\nProvider unchanged ({}).",
                target.as_str(),
                previous_provider.as_str()
            ),
        });
        return;
    }
    *config = next_config;

    let new_base_url = resolved_endpoint;
    let new_endpoint = display_base_url_host(&new_base_url);
    let cache_scope_changed = previous_provider != target || previous_model != new_model;
    app.api_provider = target;
    app.max_subagents = config
        .max_subagents_for_provider(target)
        .clamp(1, crate::config::MAX_SUBAGENTS);
    app.provider_chain = target
        .kind()
        .map(|kind| codewhale_config::ProviderChain::new(kind, &config.fallback_providers))
        .filter(|chain| chain.providers().len() > 1);
    app.last_fallback_reason = None;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.reasoning_effort = app.reasoning_effort.normalize_for_provider(target);
    app.set_model_selection(new_model.clone());
    if model_override.is_some() {
        app.provider_models
            .insert(target.as_str().to_string(), new_model.clone());
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
    *engine_handle = spawn_engine(engine_config, config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
            })
            .await;
    }
    let _ = engine_handle
        .send(Op::SetCompaction {
            config: app.compaction_config(),
        })
        .await;

    let persist_warning = (|| -> anyhow::Result<()> {
        crate::config_persistence::persist_root_string_key(
            app.config_path.as_deref(),
            "provider",
            target.as_str(),
        )?;

        let mut settings = crate::settings::Settings::load()?;
        settings.default_provider = Some(target.as_str().to_string());
        if model_override.is_some() {
            settings.set_model_for_provider(target.as_str(), &new_model);
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
        previous_provider.as_str(),
        target.as_str(),
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

    let mut status_message = format!("Provider: {} via {}", target.as_str(), new_endpoint);
    if persist_warning.is_some() {
        status_message.push_str(" (not fully persisted)");
    }
    app.status_message = Some(status_message);
}

async fn apply_provider_fallback_switch(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    previous_provider: ApiProvider,
) {
    let target = app.api_provider;
    let previous_model = app.model.clone();

    let resolved_route = match resolve_runtime_route(config, target, None) {
        Ok(route) => route,
        Err(reason) => {
            app.api_provider = previous_provider;
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
    let resolved_endpoint = resolved_route.candidate.endpoint.base_url.clone();
    let next_config = resolved_route.config;
    let new_model = resolved_route.model;

    if let Err(err) = DeepSeekClient::new(&next_config) {
        app.api_provider = previous_provider;
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
    *config = next_config;

    let new_base_url = resolved_endpoint;
    let new_endpoint = display_base_url_host(&new_base_url);
    let cache_scope_changed = previous_provider != target || previous_model != new_model;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.reasoning_effort = app.reasoning_effort.normalize_for_provider(target);
    app.set_model_selection(new_model.clone());
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
    *engine_handle = spawn_engine(engine_config, config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
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
    config.provider = Some(app.api_provider.as_str().to_string());
}

fn provider_picker_model_override(app: &App, provider: ApiProvider) -> Option<String> {
    (app.api_provider == provider).then(|| app.model.clone())
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

pub(crate) fn open_context_inspector(app: &mut App) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let content = build_context_inspector_text(app, app.ui_locale);
    app.view_stack.push(PagerView::from_text(
        tr(app.ui_locale, MessageId::CtxInspTitle),
        &content,
        width.saturating_sub(2),
    ));
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
                app.status_message = Some(format!("Session loaded from {}", path.display()));
            }
            AppAction::SyncSession {
                session_id,
                messages,
                system_prompt,
                model,
                workspace,
            } => {
                let mut session_id = session_id;
                let is_full_reset = messages.is_empty() && system_prompt.is_none();
                if is_full_reset && session_id.is_none() {
                    let new_session_id = uuid::Uuid::new_v4().to_string();
                    app.current_session_id = Some(new_session_id.clone());
                    session_id = Some(new_session_id);
                }
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id,
                        messages,
                        system_prompt,
                        system_prompt_override: false,
                        model,
                        workspace,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SetCompaction {
                        config: app.compaction_config(),
                    })
                    .await;
                if is_full_reset {
                    if let Ok(manager) = SessionManager::default_location() {
                        let session = build_session_snapshot(app, &manager);
                        app.current_session_id = Some(session.metadata.id.clone());
                        persistence_actor::persist(PersistRequest::SessionSnapshot(session));
                    }
                    persistence_actor::persist(PersistRequest::ClearCheckpoint);
                }
            }
            AppAction::ModeChanged(mode) => {
                sync_mode_update(engine_handle, mode).await;
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
                let _ = engine_handle.send(Op::ListSubAgents).await;
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
            AppAction::CacheWarmup => {
                app.status_message = Some("Warming DeepSeek cache...".to_string());
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
            AppAction::UpdateCompaction(compaction) => {
                apply_model_and_compaction_update(engine_handle, compaction, app.mode).await;
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
                        .push(crate::tui::model_picker::ModelPickerView::new(app));
                }
            }
            AppAction::OpenProviderPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    app.view_stack
                        .push(crate::tui::provider_picker::ProviderPickerView::new(
                            app.api_provider,
                            config,
                        ));
                }
            }
            AppAction::OpenModePicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ModePicker) {
                    app.view_stack
                        .push(crate::tui::views::mode_picker::ModePickerView::new(
                            app.mode,
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
                    app.view_stack
                        .push(crate::tui::theme_picker::ThemePickerView::new(original));
                }
            }
            AppAction::OpenFleetSetup => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetSetup) {
                    app.view_stack
                        .push(crate::tui::views::fleet_setup::FleetSetupView::new(
                            app, config,
                        ));
                }
            }
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
            AppAction::CompactContext => {
                app.status_message = Some("Compacting context...".to_string());
                let _ = engine_handle.send(Op::CompactContext).await;
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
                    auto_approve: Some(app.approval_mode == ApprovalMode::Auto),
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
                    Ok(task) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Task {} status: {:?}", task.id, task.status),
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
                // Tasks sidebar stays accurate without waiting for the
                // next 2.5 s periodic refresh (#2937).
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::Mcp(action) => {
                handle_mcp_ui_action(app, config, action).await;
            }
            AppAction::SwitchWorkspace { workspace } => {
                switch_workspace(app, engine_handle, task_manager, config, workspace).await;
            }
            AppAction::SwitchProfile { profile } => {
                app.config_profile = Some(profile.clone());
                match Config::load(app.config_path.clone(), Some(&profile)) {
                    Ok(new_config) => {
                        *config = new_config.clone();
                        app.api_provider = config.api_provider();
                        let new_model = config.default_model();
                        app.set_model_selection(new_model.clone());
                        app.update_model_compaction_budget();
                        app.session.last_prompt_tokens = None;
                        app.session.last_completion_tokens = None;
                        app.session.last_output_throughput = None;
                        // Rebuild the engine with the new config so API key/model/base URL take effect.
                        let _ = engine_handle.send(Op::Shutdown).await;
                        let engine_config = build_engine_config(app, config);
                        *engine_handle = spawn_engine(engine_config, config);
                        if !app.api_messages.is_empty() {
                            let _ = engine_handle
                                .send(Op::SyncSession {
                                    session_id: app.current_session_id.clone(),
                                    messages: app.api_messages.clone(),
                                    system_prompt: app.system_prompt.clone(),
                                    system_prompt_override: false,
                                    model: app.model.clone(),
                                    workspace: app.workspace.clone(),
                                })
                                .await;
                        }
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Switched to profile '{profile}'. Model: {new_model}, Provider: {}",
                                config.api_provider().as_str()
                            ),
                        });
                        app.status_message = Some(format!("Profile: {profile}"));
                    }
                    Err(err) => {
                        app.config_profile = None;
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
    *engine_handle = spawn_engine(engine_config, config);
    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: workspace.clone(),
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
    config: &Config,
    action: crate::tui::app::McpUiAction,
) {
    use crate::mcp::{self, McpWriteStatus};

    let path = app.mcp_config_path.clone();
    let mut changed = false;
    let mut message = None;
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
        crate::tui::app::McpUiAction::Validate | crate::tui::app::McpUiAction::Reload => Ok(()),
    };

    if let Err(err) = action_result {
        add_mcp_message(app, format!("MCP action failed: {err}"));
        return;
    }

    if changed {
        app.mcp_restart_required = true;
    }
    if let Some(message) = message {
        add_mcp_message(app, message);
    }

    let snapshot_result = if discover {
        let network_policy = config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        });
        mcp::discover_manager_snapshot_with_workspace(
            &path,
            &app.workspace,
            network_policy,
            app.mcp_restart_required,
        )
        .await
    } else {
        mcp::manager_snapshot_from_config_with_workspace(
            &path,
            &app.workspace,
            app.mcp_restart_required,
        )
    };

    match snapshot_result {
        Ok(snapshot) => {
            if discover {
                add_mcp_message(
                    app,
                    "MCP discovery refreshed for the UI. Restart the TUI after config edits to rebuild the model-visible MCP tool pool.".to_string(),
                );
            }
            // Keep the boot-time MCP-count chip in sync with the live
            // snapshot so footers and panels reflect post-/mcp edits
            // (#502).
            app.mcp_configured_count = snapshot.servers.len();
            app.mcp_snapshot = Some(snapshot.clone());
            open_mcp_manager_pager(app, &snapshot);
        }
        Err(err) => add_mcp_message(app, format!("MCP snapshot failed: {err}")),
    }
}

fn mcp_ui_action_refreshes_discovery(action: &crate::tui::app::McpUiAction) -> bool {
    matches!(
        action,
        crate::tui::app::McpUiAction::Show
            | crate::tui::app::McpUiAction::Validate
            | crate::tui::app::McpUiAction::Reload
    )
}

fn handle_shell_job_action(app: &mut App, action: crate::tui::app::ShellJobAction) {
    let Some(shell_manager) = app.runtime_services.shell_manager.clone() else {
        add_shell_job_message(app, "Command center is not attached.".to_string());
        return;
    };

    let mut manager = match shell_manager.lock() {
        Ok(manager) => manager,
        Err(_) => {
            add_shell_job_message(app, "Command center lock is poisoned.".to_string());
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
        config.api_key = None;
        config.provider_config_for_mut(app.api_provider).api_key = None;
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

fn parse_queue_send_command(input: &str) -> Option<Result<usize, String>> {
    let rest = strip_queue_command_prefix(input.trim())?;
    let mut parts = rest.split_whitespace();
    let action = parts.next()?;
    if !matches!(action.to_ascii_lowercase().as_str(), "send" | "now") {
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
    let paused_note = prepare_paused_command_message(app, engine_handle, &message.display);
    let cwd = std::env::current_dir().ok();
    let references = crate::tui::file_mention::context_references_from_input(
        &message.display,
        &app.workspace,
        cwd.clone(),
    );
    let mut content = queued_message_content_for_app(app, &message, cwd);
    if let Some(note) = paused_note.as_deref() {
        content.push_str(note);
    }
    let message_index = app.api_messages.len();

    engine_handle.steer(content.clone()).await?;
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

/// Park a draft on the queued-messages bucket for dispatch after TurnComplete.
/// Unlike a steer, the message is NOT forwarded immediately — it waits for
/// the current turn to finish, then dispatches as a normal user message.
async fn queue_follow_up(app: &mut App, message: QueuedMessage) -> Result<()> {
    let display = message.display.clone();
    app.queue_message(message);
    app.status_message = Some(format!(
        "Queued: {} ({} total) — ↑ to edit",
        display,
        app.queued_message_count()
    ));
    Ok(())
}

async fn submit_or_steer_message(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) -> Result<()> {
    match app.decide_submit_disposition() {
        SubmitDisposition::Immediate => {
            dispatch_user_message(app, config, engine_handle, message).await
        }
        SubmitDisposition::Queue => {
            let count = app.queued_message_count().saturating_add(1);
            app.queue_message(message);
            if app.offline_mode {
                app.status_message = Some(format!(
                    "Offline: {count} queued follow-up(s) — ↑ edit last, /queue send <n>"
                ));
            } else {
                app.status_message = Some(format!(
                    "{count} queued follow-up(s) — ↑ edit last, /queue send <n>"
                ));
            }
            Ok(())
        }
        // Steer: reached via Enter when busy-but-waiting (v0.8.44), or
        // via Ctrl+Enter override in any busy state.
        SubmitDisposition::Steer => {
            if let Err(err) = steer_user_message(app, engine_handle, message.clone()).await {
                app.queue_message(message);
                app.status_message = Some(format!(
                    "Steer failed ({err}); {} queued follow-up(s) — /queue send <n>",
                    app.queued_message_count()
                ));
            } else {
                app.push_status_toast(
                    "Steering into current turn",
                    StatusToastLevel::Info,
                    Some(1_500),
                );
            }
            Ok(())
        }
        SubmitDisposition::QueueFollowUp => queue_follow_up(app, message).await,
    }
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
    let mut bodies: Vec<String> = Vec::with_capacity(drained.len());
    for msg in drained {
        if skill_instruction.is_none() {
            skill_instruction = msg.skill_instruction;
        }
        bodies.push(msg.display);
    }
    Some(QueuedMessage::new(bodies.join("\n\n"), skill_instruction))
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
        "  1) Accept + implement in Agent mode",
        "  2) Accept + implement in YOLO mode",
        "  3) Revise the plan / ask follow-ups",
        "  4) Return to Agent mode without implementing",
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
    match choice {
        PlanChoice::AcceptAgent => {
            apply_mode_update(app, engine_handle, AppMode::Agent).await;
            app.add_message(HistoryCell::System {
                content: "Plan accepted. Switching to Agent mode and starting implementation."
                    .to_string(),
            });
            let followup = QueuedMessage::new("Proceed with the accepted plan.".to_string(), None);
            if app.is_loading {
                app.queue_message(followup);
                app.status_message =
                    Some("Queued accepted plan execution (agent mode).".to_string());
            } else {
                dispatch_user_message(app, config, engine_handle, followup).await?;
            }
        }
        PlanChoice::AcceptYolo => {
            apply_mode_update(app, engine_handle, AppMode::Yolo).await;
            app.add_message(HistoryCell::System {
                content: "Plan accepted. Switching to YOLO mode and starting implementation."
                    .to_string(),
            });
            let followup = QueuedMessage::new("Proceed with the accepted plan.".to_string(), None);
            if app.is_loading {
                app.queue_message(followup);
                app.status_message =
                    Some("Queued accepted plan execution (YOLO mode).".to_string());
            } else {
                dispatch_user_message(app, config, engine_handle, followup).await?;
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
                    "Exited Plan mode. Switched to Agent mode.\n\n",
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
/// - `queued_messages` — Enter while busy (offline-mode FIFO); drained at
///   end-of-turn.
fn build_pending_input_preview(app: &App) -> PendingInputPreview {
    let mut preview = PendingInputPreview::new();
    let selected_attachment = app.selected_composer_attachment_index();
    let mut attachment_index = 0usize;
    preview.context_items = crate::tui::file_mention::pending_context_previews(
        &app.input,
        &app.workspace,
        std::env::current_dir().ok(),
    )
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

fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Clear entire area with the configured app background.
    let background = Block::default().style(Style::default().bg(app.ui_theme.surface_bg));
    f.render_widget(background, size);

    // Show onboarding screen if needed
    if app.onboarding != OnboardingState::None {
        onboarding::render(f, size, app);
        return;
    }

    let header_height = 1;
    let footer_height = 1;
    let slash_menu_entries = visible_slash_menu_entries(app, SLASH_MENU_LIMIT);
    let mention_menu_limit = app.mention_menu_limit;
    let mention_menu_entries =
        crate::tui::file_mention::visible_mention_menu_entries(app, mention_menu_limit);
    if !mention_menu_entries.is_empty() && app.mention_menu_selected >= mention_menu_entries.len() {
        app.mention_menu_selected = mention_menu_entries.len().saturating_sub(1);
    }
    let context_usage = context_usage_snapshot(app);

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
        .saturating_sub(MIN_CHAT_HEIGHT + footer_height)
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
    let preview_height = pending_preview.desired_height(size.width);

    let body_chunks = Layout::default()
        .direction(Direction::Vertical)
        .flex(ratatui::layout::Flex::Start)
        .constraints([
            Constraint::Min(1),                  // Chat area
            Constraint::Length(preview_height),  // Pending input preview (0 if empty)
            Constraint::Length(composer_height), // Composer
            Constraint::Length(footer_height),   // Footer
        ])
        .split(body_area);

    // Render header
    {
        let sanitized_context_window = context_usage
            .as_ref()
            .map(|(_, max, _)| *max)
            .or_else(|| crate::models::context_window_for_model(&app.model));
        let sanitized_prompt_tokens = context_usage
            .as_ref()
            .and_then(|(used, _, _)| u32::try_from(*used).ok());
        let workspace_name = app
            .workspace
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("workspace");
        let model_label = app.model_display_label();
        let effort_label = app.reasoning_effort_display_label();
        let provider_label = match app.api_provider {
            crate::config::ApiProvider::Deepseek => None,
            crate::config::ApiProvider::DeepseekCN => None,
            crate::config::ApiProvider::DeepseekAnthropic => Some("DS-A"),
            crate::config::ApiProvider::NvidiaNim => Some("NIM"),
            crate::config::ApiProvider::Openai => Some("OpenAI"),
            crate::config::ApiProvider::Anthropic => Some("Claude"),
            crate::config::ApiProvider::Atlascloud => Some("Atlas"),
            crate::config::ApiProvider::WanjieArk => Some("Wanjie"),
            crate::config::ApiProvider::Volcengine => Some("Volc"),
            crate::config::ApiProvider::Openrouter => Some("OR"),
            crate::config::ApiProvider::XiaomiMimo => Some("MiMo"),
            crate::config::ApiProvider::Novita => Some("Novita"),
            crate::config::ApiProvider::Fireworks => Some("Fireworks"),
            crate::config::ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => {
                Some("SiliconFlow")
            }
            crate::config::ApiProvider::Arcee => Some("Arcee"),
            crate::config::ApiProvider::Moonshot => Some("Kimi"),
            crate::config::ApiProvider::Sglang => Some("SGLang"),
            crate::config::ApiProvider::Vllm => Some("vLLM"),
            crate::config::ApiProvider::Ollama => Some("Ollama"),
            crate::config::ApiProvider::Huggingface => Some("HF"),
            crate::config::ApiProvider::Deepinfra => Some("DeepInfra"),
            crate::config::ApiProvider::Together => Some("Together"),
            crate::config::ApiProvider::Qianfan => Some("Qianfan"),
            crate::config::ApiProvider::OpenaiCodex => Some("Codex"),
            crate::config::ApiProvider::Zai => Some("Z.ai"),
            crate::config::ApiProvider::Stepfun => Some("StepFun"),
            crate::config::ApiProvider::Minimax => Some("MiniMax"),
        };
        let status_indicator_started_at = if app.low_motion {
            None
        } else {
            app.turn_started_at
        };
        let header_data = HeaderData::new(
            app.mode,
            &model_label,
            workspace_name,
            app.is_loading,
            app.ui_theme.header_bg,
        )
        .with_usage(
            app.session.total_conversation_tokens,
            sanitized_context_window,
            app.session.session_cost,
            sanitized_prompt_tokens,
        )
        .with_reasoning_effort(Some(&effort_label))
        .with_provider(provider_label)
        .with_status_indicator(crate::tui::widgets::header_status_indicator_frame(
            status_indicator_started_at,
            &app.status_indicator,
        ));
        let header_widget = HeaderWidget::new(header_data);
        let buf = f.buffer_mut();
        header_widget.render(header_area, buf);
    }

    // Render chat + sidebar + optional file-tree pane
    {
        // Defensive backstop (#400): fill the entire body area with ink
        // background before any sub-widgets render, so cells that end up
        // uncovered by layout splits (e.g. after file-tree toggle or
        // resize) don't retain stale content from a previous frame.
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(body_chunks[0], f.buffer_mut());

        let mut sidebar_area = None;

        // When the file-tree pane is visible and the terminal is wide
        // enough, reserve the left ~25% for the file tree.
        let mut chat_area =
            if app.file_tree.is_some() && body_chunks[0].width >= SIDEBAR_VISIBLE_MIN_WIDTH {
                app.file_tree_visible = true;
                let split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                    .split(body_chunks[0]);
                let tree_area = split[0];
                let remaining = split[1];

                // Render the file-tree pane.
                if let Some(ref mut state) = app.file_tree {
                    super::file_tree::render_file_tree(f, tree_area, state, app.ui_theme.mode);
                }

                remaining
            } else {
                app.file_tree_visible = false;
                body_chunks[0]
            };

        // Auto-reveal: in Auto focus mode, collapse the sidebar to a
        // full-width transcript when nothing is active; bring it back the
        // moment there is a To-do, a live fleet, or background jobs.
        app.last_sidebar_host_width = Some(chat_area.width);
        let sidebar_auto_collapsed = crate::tui::sidebar::sidebar_auto_idle(app);
        if !sidebar_auto_collapsed
            && let Some(sidebar_width) = sidebar_width_for_chat_area(app, chat_area.width)
        {
            // Record total width for drag-to-resize percentage calculation.
            app.sidebar_resize_total_width = chat_area.width;
            let split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(1), Constraint::Length(sidebar_width)])
                .split(chat_area);
            chat_area = split[0];
            sidebar_area = Some(split[1]);
        }

        // Record the sidebar rect (or its absence) every frame so mouse
        // hit-testing can route scroll events correctly.
        app.viewport.last_sidebar_area = sidebar_area;

        // When the sidebar is hidden or doesn't fit, drop its stale mouse
        // hit areas and any in-flight resize so clicks on those columns
        // don't keep routing to an invisible handle (#3063).
        if sidebar_area.is_none() {
            app.last_sidebar_area = None;
            app.last_sidebar_handle_area = None;
            app.sidebar_resizing = false;
        }

        let chat_widget = ChatWidget::new(app, chat_area);
        let buf = f.buffer_mut();
        chat_widget.render(chat_area, buf);

        if let Some(sidebar_area) = sidebar_area {
            // Store sidebar area for mouse hit-testing (resize handle).
            app.last_sidebar_area = Some(sidebar_area);

            // Render sidebar
            super::sidebar::render_sidebar(f, sidebar_area, app);

            // Paint resize handle (1-col draggable bar) on the left edge of
            // the sidebar, over the sidebar content. Mouse drag on this strip
            // adjusts sidebar_width_percent in real time.
            let handle_rect = Rect {
                x: sidebar_area.x,
                y: sidebar_area.y,
                width: 1,
                height: sidebar_area.height,
            };

            // Store for mouse event handler.
            app.last_sidebar_handle_area = Some(handle_rect);

            let mouse_over = app.last_mouse_pos.is_some_and(|(col, row)| {
                row >= handle_rect.y
                    && row < handle_rect.y.saturating_add(handle_rect.height)
                    && col == handle_rect.x
            });

            let handle_style = if app.sidebar_resizing {
                Style::default()
                    .bg(palette::WHALE_ACCENT_PRIMARY)
                    .fg(palette::TEXT_PRIMARY)
            } else if mouse_over {
                Style::default()
                    .bg(palette::STATUS_WARNING)
                    .fg(palette::TEXT_MUTED)
            } else {
                Style::default()
                    .bg(palette::DEEPSEEK_SLATE)
                    .fg(palette::TEXT_MUTED)
            };

            let buf = f.buffer_mut();
            for row in handle_rect.y..handle_rect.y.saturating_add(handle_rect.height) {
                if row < buf.area().height {
                    buf[(handle_rect.x, row)]
                        .set_char('│')
                        .set_style(handle_style);
                }
            }

            // Render sidebar hover popover if active.
            if let Some(ref tooltip_text) = app.sidebar_hover_tooltip
                && let Some((mouse_col, mouse_row)) = app.last_mouse_pos
            {
                let max_popup_width = 72u16.min(size.width.saturating_sub(4));
                if max_popup_width >= 10 && size.height >= 3 {
                    let popup_width = tooltip_text
                        .lines()
                        .map(text_display_width)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(2)
                        .clamp(12, max_popup_width as usize)
                        as u16;
                    let inner_width = popup_width.saturating_sub(2).max(1) as usize;
                    let wrapped_rows = tooltip_text.lines().fold(0u16, |rows, line| {
                        let width = text_display_width(line);
                        rows.saturating_add(((width.max(1) - 1) / inner_width + 1) as u16)
                    });
                    let popup_content_height = wrapped_rows.clamp(1, 10);
                    let popup_height = popup_content_height.saturating_add(2);
                    let x = mouse_col
                        .saturating_add(2)
                        .min(size.width.saturating_sub(popup_width));
                    // Sit one row BELOW the cursor so the tooltip never paints over
                    // the row above the hovered line (which read as corruption).
                    let y = mouse_row
                        .saturating_add(1)
                        .min(size.height.saturating_sub(popup_height));
                    let tooltip_area = Rect {
                        x,
                        y,
                        width: popup_width,
                        height: popup_height,
                    };
                    // Neutral elevated-surface styling so the popover reads as a
                    // detail surface, not a warning highlight.
                    let tooltip = ratatui::widgets::Paragraph::new(tooltip_text.as_str())
                        .wrap(ratatui::widgets::Wrap { trim: false })
                        .block(
                            Block::default()
                                .borders(ratatui::widgets::Borders::ALL)
                                .border_style(Style::default().fg(palette::WHALE_ACCENT_PRIMARY))
                                .style(
                                    Style::default()
                                        .bg(palette::SURFACE_ELEVATED)
                                        .fg(palette::TEXT_PRIMARY),
                                ),
                        );
                    f.render_widget(tooltip, tooltip_area);
                }
            }
        }
    }

    // Render pending-input preview (queued/steered messages, if any).
    if preview_height > 0 {
        let buf = f.buffer_mut();
        pending_preview.render(body_chunks[1], buf);
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
        composer_widget.render(body_chunks[2], buf);
        composer_widget.cursor_pos(body_chunks[2])
    };
    app.viewport.last_composer_area = Some(body_chunks[2]);
    {
        let area = body_chunks[2];
        let has_panel = app.composer_border && area.height >= 3 && area.width >= 12;
        let inner = if has_panel {
            ratatui::widgets::Block::default()
                .borders(ratatui::widgets::Borders::ALL)
                .inner(area)
        } else {
            area
        };
        app.viewport.last_composer_content = Some(inner);

        // Compute scroll offset and top padding for mouse coordinate mapping.
        let input_text = app.composer_display_input();
        let input_cursor = app.composer_display_cursor();
        let content_width = usize::from(inner.width.max(1));
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
        let visible_lines = if input_text.is_empty() {
            1
        } else {
            // Count wrapped lines (approximation matching the render path).
            crate::tui::widgets::wrap_input_lines_for_mouse(input_text, content_width).len()
        };
        let top_padding = budget.saturating_sub(visible_lines.clamp(1, budget));
        app.viewport.last_composer_scroll_offset = scroll_offset;
        app.viewport.last_composer_top_padding = top_padding;
    }
    if let Some(cursor_pos) = cursor_pos {
        f.set_cursor_position(cursor_pos);
    }

    // Render footer
    render_footer(f, body_chunks[3], app);
    // Toast stack overlay (#439): when multiple status toasts are queued,
    // surface the older ones as a 1-2 line strip above the footer so a
    // burst of events isn't collapsed to a single visible message.
    render_toast_stack_overlay(f, size, body_chunks[2], body_chunks[3], app);

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
        terminal.draw(|f| render(f, app))?;
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

/// Toggle the live transcript overlay on `Ctrl+T`. Closes the overlay if it's
/// already on top; otherwise pushes a fresh one in sticky-tail mode.
fn toggle_live_transcript_overlay(app: &mut App) {
    if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
        app.view_stack.pop();
        app.needs_redraw = true;
        return;
    }
    let mut overlay = LiveTranscriptOverlay::new();
    overlay.refresh_from_app(app);
    app.view_stack.push(overlay);
    app.status_message = Some("Live transcript: tailing (Esc to close)".to_string());
    app.needs_redraw = true;
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
                let _ = engine_handle.submit_user_input(tool_id, response).await;
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
                        let recovered = apply_loaded_session(app, config, &session);
                        sync_runtime_workspace_state(task_manager, app.workspace.clone()).await;
                        let _ = engine_handle
                            .send(Op::SyncSession {
                                session_id: app.current_session_id.clone(),
                                messages: app.api_messages.clone(),
                                system_prompt: app.system_prompt.clone(),
                                system_prompt_override: false,
                                model: app.model.clone(),
                                workspace: app.workspace.clone(),
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
                    }
                    Err(err) => {
                        app.status_message = Some(format!(
                            "Failed to load session {}: {err}",
                            crate::session_manager::truncate_id(&session_id)
                        ));
                    }
                }
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
                let result = commands::set_config_value(app, &key, &value, persist);
                // Theme / background changes require a full terminal repaint
                // because ratatui's incremental diff may miss color-only
                // changes in cells that were rendered with theme-resolved
                // colors (sidebar panels) rather than palette constants that
                // go through the backend remap layer.  A full repaint
                // (terminal clear + all cells redrawn) guarantees every cell
                // picks up the new theme immediately.
                if matches!(
                    key.as_str(),
                    "theme" | "ui_theme" | "background_color" | "background" | "bg"
                ) {
                    app.force_next_full_repaint = true;
                }
                // Only surface the "key = value" confirmation when the
                // change is being persisted. Live-preview events
                // (`persist: false`, e.g. arrow keys in the theme picker)
                // fire on every navigation tick and would otherwise spam
                // a `System` cell into the transcript per row visited.
                if persist && let Some(msg) = result.message {
                    app.add_message(HistoryCell::System { content: msg });
                }

                if let Some(action) = result.action {
                    match action {
                        AppAction::UpdateCompaction(compaction) => {
                            apply_model_and_compaction_update(engine_handle, compaction, app.mode)
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
                        AppAction::OpenConfigView => {}
                        _ => {}
                    }
                }

                if app.view_stack.top_kind() == Some(ModalKind::Config) {
                    app.view_stack.pop();
                    app.view_stack.push(ConfigView::new_for_app(app));
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
            ViewEvent::SubAgentsRefresh => {
                app.status_message = Some("Refreshing sub-agents...".to_string());
                let _ = engine_handle.send(Op::ListSubAgents).await;
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
                    effort,
                    previous_model,
                    previous_effort,
                )
                .await;
            }
            ViewEvent::ProviderPickerApplied { provider } => {
                let model_override = provider_picker_model_override(app, provider);
                switch_provider(app, engine_handle, config, provider, model_override).await;
            }
            ViewEvent::ProviderPickerApiKeySubmitted { provider, api_key } => {
                apply_provider_picker_api_key(app, engine_handle, config, provider, api_key).await;
            }
            ViewEvent::ProviderPickerKimiOAuthEnabled { provider } => {
                apply_provider_picker_auth_mode(
                    app,
                    engine_handle,
                    config,
                    provider,
                    "kimi_oauth",
                    "Linked Kimi CLI OAuth",
                )
                .await;
            }
            ViewEvent::ModeSelected { mode } => {
                let prior_mode = app.mode;
                let msg = commands::switch_mode(app, mode);
                if app.mode != prior_mode {
                    sync_mode_update(engine_handle, app.mode).await;
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
        }
    }

    Ok(false)
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
    if tool_name == "apply_patch" {
        maybe_add_patch_preview(app, tool_input);
    }

    let request = ApprovalRequest::new_with_intent(
        id,
        tool_name,
        description,
        tool_input,
        approval_key,
        intent_summary,
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
            app.status_message = Some("Request cancelled".to_string());
        }
    }
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

    // Trim the API-message log at the matching user message. We
    // re-walk `api_messages` from the tail, counting role=="user"
    // boundaries so the depth aligns with what the model sees on the
    // next turn.
    let mut user_seen = 0usize;
    let mut cut = None;
    for (idx, msg) in app.api_messages.iter().enumerate().rev() {
        if msg.role == "user" {
            if user_seen == depth {
                cut = Some(idx);
                break;
            }
            user_seen += 1;
        }
    }
    if let Some(idx) = cut {
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
async fn apply_provider_picker_api_key(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    provider: ApiProvider,
    api_key: String,
) {
    use crate::config::save_api_key_for;

    match save_api_key_for(provider, &api_key) {
        Ok(path) => {
            app.status_message = Some(format!(
                "Saved {} API key to {}",
                provider.as_str(),
                path.display()
            ));
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to save {} API key: {err}\nProvider unchanged.",
                    provider.as_str()
                ),
            });
            return;
        }
    }

    // Mirror the saved key into the in-memory config so the engine sees it
    // immediately without a reload — `save_api_key_for` only touches disk.
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        config.api_key = Some(api_key);
    } else {
        let providers = config
            .providers
            .get_or_insert_with(ProvidersConfig::default);
        let entry: &mut ProviderConfig = match provider {
            ApiProvider::Deepseek | ApiProvider::DeepseekCN => {
                // Guarded by the outer `if` above; safety net against refactors.
                return;
            }
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
            ApiProvider::Zai => &mut providers.zai,
            ApiProvider::Stepfun => &mut providers.stepfun,
            ApiProvider::Minimax => &mut providers.minimax,
        };
        entry.api_key = Some(api_key);
    }

    switch_provider(app, engine_handle, config, provider, None).await;
}

async fn apply_provider_picker_auth_mode(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    provider: ApiProvider,
    auth_mode: &str,
    status_prefix: &str,
) {
    match save_provider_auth_mode_for(provider, auth_mode) {
        Ok(path) => {
            set_provider_auth_mode_in_memory(config, provider, auth_mode.to_string());
            app.status_message = Some(format!("{status_prefix}; saved to {}", path.display()));
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to save {} auth mode: {err}\nProvider unchanged.",
                    provider.as_str()
                ),
            });
            return;
        }
    }

    switch_provider(app, engine_handle, config, provider, None).await;
}

fn set_provider_auth_mode_in_memory(config: &mut Config, provider: ApiProvider, auth_mode: String) {
    let providers = config
        .providers
        .get_or_insert_with(ProvidersConfig::default);
    let entry: &mut ProviderConfig = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => return,
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
        ApiProvider::Zai => &mut providers.zai,
        ApiProvider::Stepfun => &mut providers.stepfun,
        ApiProvider::Minimax => &mut providers.minimax,
    };
    entry.auth_mode = Some(auth_mode);
}

fn apply_loaded_session(app: &mut App, config: &Config, session: &SavedSession) -> bool {
    let (messages, recovered_draft) = recover_interrupted_user_tail(&session.messages);
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
    app.set_model_selection(session.metadata.model.clone());
    app.update_model_compaction_budget();
    apply_workspace_runtime_state(app, config, session.metadata.workspace.clone());
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
    recovered
}

/// Derive a short display title from the API message list.
/// Skips the `<turn_meta>` block prepended by the engine and takes the first
/// real user-text block, truncated to 32 characters.
fn derive_session_title(messages: &[Message]) -> Option<String> {
    messages.iter().find(|m| m.role == "user").and_then(|m| {
        m.content.iter().find_map(|block| match block {
            ContentBlock::Text { text, .. } if !text.starts_with(TURN_META_PREFIX) => {
                let first_line = text.trim().lines().next().unwrap_or("").trim();
                if first_line.is_empty() {
                    return None;
                }
                let char_count = first_line.chars().count();
                let chars: String = first_line.chars().take(SESSION_TITLE_MAX_CHARS).collect();
                if char_count > SESSION_TITLE_MAX_CHARS {
                    Some(format!("{chars}…"))
                } else {
                    Some(chars)
                }
            }
            _ => None,
        })
    })
}

fn recover_interrupted_user_tail(messages: &[Message]) -> (Vec<Message>, Option<QueuedMessage>) {
    let mut recovered = messages.to_vec();
    let Some(last) = recovered.last() else {
        return (recovered, None);
    };
    if last.role != "user" {
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
        execute!(terminal.backend_mut(), DisableBracketedPaste)?;
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

fn disable_alternate_scroll_mode<W: Write>(writer: &mut W) {
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
    let _ = execute!(stdout, DisableBracketedPaste);
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
fn recover_terminal_modes<W: Write>(
    writer: &mut W,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
) {
    #[cfg(target_os = "windows")]
    enable_windows_ime_console_mode();

    pop_keyboard_enhancement_flags(writer);
    push_keyboard_enhancement_flags(writer);
    enable_alternate_scroll_mode(writer);
    if use_mouse_capture && let Err(err) = execute!(writer, EnableMouseCapture) {
        tracing::debug!(?err, "EnableMouseCapture ignored");
    }
    if use_bracketed_paste && let Err(err) = execute!(writer, EnableBracketedPaste) {
        tracing::debug!(?err, "EnableBracketedPaste ignored");
    }
    if let Err(err) = execute!(writer, EnableFocusChange) {
        tracing::debug!(?err, "EnableFocusChange ignored");
    }
}

fn terminal_event_needs_viewport_recapture(evt: &Event) -> bool {
    matches!(evt, Event::FocusGained)
}

pub(crate) fn status_color(level: StatusToastLevel) -> ratatui::style::Color {
    match level {
        StatusToastLevel::Info => palette::DEEPSEEK_SKY,
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
        app.status_message = Some("No foreground shell command to background".to_string());
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
            "Cannot background: {reason}. Press Ctrl+C to cancel the turn, or wait for completion."
        ));
        return;
    }

    let Some(shell_manager) = app.runtime_services.shell_manager.clone() else {
        app.status_message = Some("Shell manager is not attached".to_string());
        return;
    };

    match shell_manager.lock() {
        Ok(mut manager) => {
            manager.request_foreground_background();
            app.status_message = Some("Backgrounding current shell command...".to_string());
        }
        Err(_) => {
            app.status_message = Some("Shell manager lock is poisoned".to_string());
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
    let max =
        provider_capability(app.api_provider, app.effective_model_for_budget()).context_window;
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

fn maybe_warn_context_pressure(app: &mut App) {
    let Some((used, max, percent)) = context_usage_snapshot(app) else {
        return;
    };

    let configured_threshold = app.auto_compact_threshold_percent.clamp(10.0, 100.0);
    let warning_threshold = CONTEXT_SUGGEST_COMPACT_THRESHOLD_PERCENT.min(configured_threshold);
    if percent < warning_threshold {
        return;
    }

    let recommendation = if !app.auto_compact {
        "Consider enabling auto_compact or use /compact."
    } else if percent >= configured_threshold {
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

fn should_auto_compact_before_send(app: &App) -> bool {
    if !app.auto_compact {
        return false;
    }
    context_usage_snapshot(app)
        .map(|(_, _, pct)| pct >= app.auto_compact_threshold_percent.clamp(10.0, 100.0))
        .unwrap_or(false)
}

fn status_animation_interval_ms(app: &App) -> u64 {
    if app.low_motion {
        2_400
    } else {
        UI_STATUS_ANIMATION_MS
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

fn history_has_live_motion(history: &[HistoryCell]) -> bool {
    use crate::tui::history::SubAgentCell;
    use crate::tui::widgets::agent_card::AgentLifecycle;
    history.iter().any(|cell| match cell {
        HistoryCell::Thinking { streaming, .. } => *streaming,
        HistoryCell::Tool(tool) => match tool {
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
        },
        HistoryCell::SubAgent(SubAgentCell::Delegate(card)) => matches!(
            card.status,
            AgentLifecycle::Pending | AgentLifecycle::Running
        ),
        HistoryCell::SubAgent(SubAgentCell::Fanout(card)) => card
            .workers
            .iter()
            .any(|w| matches!(w.status, AgentLifecycle::Pending | AgentLifecycle::Running)),
        _ => false,
    })
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

/// Compatibility wrapper for the old test name. The user-facing Ctrl+O
/// surface is now Activity Detail, not a thinking-only pager.
#[cfg(test)]
fn open_thinking_pager(app: &mut App) -> bool {
    open_activity_detail_pager(app)
}

/// Open a pager for the activity the user is most likely asking about.
///
/// Ctrl+O uses this path. It prefers an explicitly selected activity cell,
/// then a live activity in the current turn, then the most recent meaningful
/// activity across history + active cells. Tool activity is intentionally
/// rendered through the compact live view so Activity Detail does not become
/// an accidental raw-output dump; Alt+V remains the direct full tool-detail
/// surface.
fn open_activity_detail_pager(app: &mut App) -> bool {
    let Some(idx) = activity_target_cell_index(app) else {
        app.status_message = Some("No activity detail available".to_string());
        return true;
    };

    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let Some(text) = activity_detail_text(app, idx, width) else {
        app.status_message = Some("No activity detail available".to_string());
        return true;
    };
    let title = if matches!(
        app.cell_at_virtual_index(idx),
        Some(HistoryCell::Thinking { .. })
    ) {
        "Reasoning Timeline"
    } else {
        "Activity Detail"
    };
    app.view_stack
        .push(PagerView::from_text(title, &text, width.saturating_sub(2)));
    true
}

fn activity_target_cell_index(app: &App) -> Option<usize> {
    if let Some(selected) = selected_transcript_cell_index(app)
        && app
            .cell_at_virtual_index(selected)
            .is_some_and(is_meaningful_activity_cell)
    {
        return Some(selected);
    }

    current_activity_cell_index(app).or_else(|| {
        (0..app.virtual_cell_count()).rev().find(|&idx| {
            app.cell_at_virtual_index(idx)
                .is_some_and(is_meaningful_activity_cell)
        })
    })
}

fn selected_transcript_cell_index(app: &App) -> Option<usize> {
    app.viewport
        .transcript_selection
        .ordered_endpoints()
        .and_then(|(start, _)| {
            app.viewport
                .transcript_cache
                .line_meta()
                .get(start.line_index)
                .and_then(|meta| meta.cell_line())
                .map(|(cell_index, _)| app.original_cell_index_for_rendered(cell_index))
        })
}

fn current_activity_cell_index(app: &App) -> Option<usize> {
    let active = app.active_cell.as_ref()?;
    let base = app.history.len();
    for desired_rank in [0, 1, 2] {
        if let Some((entry_idx, _)) = active
            .entries()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, cell)| activity_cell_rank(cell) == Some(desired_rank))
        {
            return Some(base + entry_idx);
        }
    }
    None
}

fn is_meaningful_activity_cell(cell: &HistoryCell) -> bool {
    activity_cell_rank(cell).is_some()
}

fn activity_cell_rank(cell: &HistoryCell) -> Option<u8> {
    match cell {
        HistoryCell::Thinking {
            streaming: true, ..
        } => Some(0),
        HistoryCell::Tool(tool) => match tool_status_for_activity(tool) {
            Some(ToolStatus::Running) => Some(0),
            Some(ToolStatus::Failed) => Some(1),
            Some(ToolStatus::Hydrated) => Some(2),
            Some(ToolStatus::Success) => Some(2),
            None => Some(2),
        },
        HistoryCell::SubAgent(_) => Some(0),
        HistoryCell::Error { .. } => Some(1),
        HistoryCell::Thinking { .. } => Some(2),
        _ => None,
    }
}

fn activity_detail_text(app: &App, cell_index: usize, width: u16) -> Option<String> {
    let cell = app.cell_at_virtual_index(cell_index)?;
    if matches!(cell, HistoryCell::Thinking { .. }) {
        return reasoning_timeline_text(app, cell_index);
    }

    let mut sections = Vec::new();

    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = app.runtime_turn_status.as_deref().unwrap_or("in progress");
        sections.push(format!(
            "Turn: {} ({status})",
            truncate_line_to_width(turn_id, 24)
        ));
    }

    sections.push(format!(
        "Activity: {}",
        activity_cell_label(app, cell_index, cell)
    ));

    if let Some(status) = activity_status_line(cell) {
        sections.push(status);
    }

    let activity_indices = activity_indices(app);
    if let Some(position) = activity_indices.iter().position(|&idx| idx == cell_index) {
        sections.push(format!(
            "Activity chunk: {} of {}",
            position + 1,
            activity_indices.len()
        ));
        sections.extend(activity_navigation_lines(app, position, &activity_indices));
    }

    if let Some(handle) = activity_detail_handle_line(app, cell_index, cell) {
        sections.push(handle);
    }
    if let Some(summary) = activity_input_summary_line(cell) {
        sections.push(summary);
    }

    sections.push(String::new());
    sections.push(activity_cell_to_text(cell, width));
    Some(sections.join("\n"))
}

fn reasoning_timeline_text(app: &App, selected_cell_index: usize) -> Option<String> {
    let thinking_indices: Vec<usize> = (0..app.virtual_cell_count())
        .filter(|&idx| {
            matches!(
                app.cell_at_virtual_index(idx),
                Some(HistoryCell::Thinking { .. })
            )
        })
        .collect();
    if thinking_indices.is_empty() {
        return None;
    }

    let selected_position = thinking_indices
        .iter()
        .position(|&idx| idx == selected_cell_index)
        .map(|idx| idx + 1);
    let total = thinking_indices.len();
    let running = thinking_indices.iter().any(|&idx| {
        matches!(
            app.cell_at_virtual_index(idx),
            Some(HistoryCell::Thinking {
                streaming: true,
                ..
            })
        )
    });

    let mut sections = Vec::new();
    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = app.runtime_turn_status.as_deref().unwrap_or("in progress");
        sections.push(format!(
            "Turn: {} ({status})",
            truncate_line_to_width(turn_id, 24)
        ));
    }
    sections.push("Activity: reasoning timeline".to_string());
    sections.push(format!(
        "Status: {} · {total} chunk{}",
        if running { "running" } else { "done" },
        if total == 1 { "" } else { "s" }
    ));
    if let Some(position) = selected_position {
        sections.push(format!("Selected chunk: {position} of {total}"));
        if position > 1 {
            let previous_index = thinking_indices[position - 2];
            let preview = thinking_chunk_preview(app, previous_index);
            sections.push(format!(
                "Previous chunk: {} of {total} - {preview}",
                position - 1
            ));
        }
        if position < total {
            let next_index = thinking_indices[position];
            let preview = thinking_chunk_preview(app, next_index);
            sections.push(format!(
                "Next chunk: {} of {total} - {preview}",
                position + 1
            ));
        }
    }
    sections.push(String::new());

    for (position, cell_index) in thinking_indices.iter().copied().enumerate() {
        let Some(HistoryCell::Thinking {
            content,
            streaming,
            duration_secs,
        }) = app.cell_at_virtual_index(cell_index)
        else {
            continue;
        };
        let position = position + 1;
        let marker = if Some(position) == selected_position {
            " (selected)"
        } else {
            ""
        };
        let mut status = if *streaming {
            "running".to_string()
        } else {
            "done".to_string()
        };
        if let Some(duration_secs) = duration_secs {
            status.push_str(" · ");
            status.push_str(&format!("{duration_secs:.1}s"));
        }
        sections.push(format!("Thinking chunk {position} of {total}{marker}"));
        sections.push(format!("Status: {status}"));
        let body = content.trim();
        if body.is_empty() {
            sections.push("(no reasoning text recorded)".to_string());
        } else {
            sections.push(body.to_string());
        }
        sections.push(String::new());
    }

    Some(sections.join("\n"))
}

fn thinking_chunk_preview(app: &App, cell_index: usize) -> String {
    let Some(HistoryCell::Thinking { content, .. }) = app.cell_at_virtual_index(cell_index) else {
        return "thinking".to_string();
    };
    let preview = one_line_summary(content, 64);
    if preview.is_empty() {
        "thinking".to_string()
    } else {
        preview
    }
}

fn activity_cell_label(app: &App, cell_index: usize, cell: &HistoryCell) -> String {
    match cell {
        HistoryCell::Thinking { .. } => "thinking".to_string(),
        HistoryCell::Error { .. } => "error".to_string(),
        HistoryCell::SubAgent(_) => "sub-agent".to_string(),
        HistoryCell::Tool(ToolCell::Generic(generic)) => {
            crate::tui::widgets::tool_card::tool_activity_label_for_name(
                &generic.name,
                app.ui_locale,
            )
        }
        HistoryCell::Tool(_) => {
            detail_target_label(app, cell_index).unwrap_or_else(|| "tool activity".to_string())
        }
        _ => "message".to_string(),
    }
}

fn activity_status_line(cell: &HistoryCell) -> Option<String> {
    match cell {
        HistoryCell::Thinking {
            streaming,
            duration_secs,
            ..
        } => {
            let mut line = if *streaming {
                "Status: running".to_string()
            } else {
                "Status: done".to_string()
            };
            if let Some(duration_secs) = duration_secs {
                line.push_str(" · ");
                line.push_str(&format!("{duration_secs:.1}s"));
            }
            Some(line)
        }
        HistoryCell::Tool(tool) => {
            let status = tool_status_for_activity(tool)?;
            let mut line = format!("Status: {}", activity_status_label(status));
            if let Some(duration_ms) = tool_duration_for_activity(tool) {
                line.push_str(" · ");
                line.push_str(&format_activity_duration_ms(duration_ms));
            }
            Some(line)
        }
        HistoryCell::Error { severity, .. } => Some(format!("Status: {severity:?}")),
        HistoryCell::SubAgent(_) => None,
        _ => None,
    }
}

fn tool_status_for_activity(tool: &ToolCell) -> Option<ToolStatus> {
    match tool {
        ToolCell::Exec(cell) => Some(cell.status),
        ToolCell::Exploring(cell) => {
            if cell
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Running)
            {
                Some(ToolStatus::Running)
            } else if cell
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Failed)
            {
                Some(ToolStatus::Failed)
            } else if cell
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Hydrated)
            {
                Some(ToolStatus::Hydrated)
            } else {
                Some(ToolStatus::Success)
            }
        }
        ToolCell::PlanUpdate(cell) => Some(cell.status),
        ToolCell::PatchSummary(cell) => Some(cell.status),
        ToolCell::Review(cell) => Some(cell.status),
        ToolCell::DiffPreview(_) => Some(ToolStatus::Success),
        ToolCell::Mcp(cell) => Some(cell.status),
        ToolCell::ViewImage(_) => Some(ToolStatus::Success),
        ToolCell::WebSearch(cell) => Some(cell.status),
        ToolCell::Generic(cell) => Some(cell.status),
    }
}

fn tool_duration_for_activity(tool: &ToolCell) -> Option<u64> {
    match tool {
        ToolCell::Exec(cell) => cell.duration_ms.or_else(|| {
            (cell.status == ToolStatus::Running).then(|| {
                u64::try_from(
                    cell.started_at
                        .map(|started| started.elapsed().as_millis())
                        .unwrap_or_default(),
                )
                .unwrap_or(u64::MAX)
            })
        }),
        _ => None,
    }
}

fn activity_status_label(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Running => "running",
        ToolStatus::Success => "done",
        ToolStatus::Hydrated => "tool loaded - retry required",
        ToolStatus::Failed => "failed",
    }
}

fn format_activity_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

fn activity_indices(app: &App) -> Vec<usize> {
    (0..app.virtual_cell_count())
        .filter(|&idx| {
            app.cell_at_virtual_index(idx)
                .is_some_and(is_meaningful_activity_cell)
        })
        .collect()
}

fn activity_navigation_lines(
    app: &App,
    position: usize,
    activity_indices: &[usize],
) -> Vec<String> {
    let total = activity_indices.len();
    let mut lines = Vec::new();
    if position > 0 {
        let previous_idx = activity_indices[position - 1];
        if let Some(cell) = app.cell_at_virtual_index(previous_idx) {
            let label = activity_cell_label(app, previous_idx, cell);
            lines.push(format!(
                "Previous activity: {} of {total} - {}",
                position,
                truncate_line_to_width(&label, 56)
            ));
        }
    }
    if position + 1 < total {
        let next_idx = activity_indices[position + 1];
        if let Some(cell) = app.cell_at_virtual_index(next_idx) {
            let label = activity_cell_label(app, next_idx, cell);
            lines.push(format!(
                "Next activity: {} of {total} - {}",
                position + 2,
                truncate_line_to_width(&label, 56)
            ));
        }
    }
    lines
}

fn activity_detail_handle_line(app: &App, cell_index: usize, cell: &HistoryCell) -> Option<String> {
    if let Some(detail) = app.tool_detail_record_for_cell(cell_index) {
        if let Some(artifact) = app
            .session_artifacts
            .iter()
            .find(|artifact| artifact.tool_call_id == detail.tool_id)
        {
            return Some(format!(
                "Detail handle: {} (retrieve_tool_result ref={}; Alt+V raw details)",
                artifact.id, artifact.id
            ));
        }
        return Some(format!(
            "Detail handle: tool:{} (Alt+V raw details)",
            detail.tool_id
        ));
    }

    match cell {
        HistoryCell::Tool(_) => Some("Detail handle: Alt+V details".to_string()),
        HistoryCell::SubAgent(_) => Some("Detail handle: Alt+V details".to_string()),
        _ => None,
    }
}

fn activity_input_summary_line(cell: &HistoryCell) -> Option<String> {
    let HistoryCell::Tool(ToolCell::Generic(generic)) = cell else {
        return None;
    };
    let summary = generic.input_summary.as_deref()?.trim();
    if summary.is_empty() {
        None
    } else {
        Some(format!("Input: {summary}"))
    }
}

fn activity_cell_to_text(cell: &HistoryCell, width: u16) -> String {
    let lines = match cell {
        HistoryCell::Tool(_) => cell.lines_with_options(
            width,
            TranscriptRenderOptions {
                calm_mode: true,
                low_motion: true,
                ..TranscriptRenderOptions::default()
            },
        ),
        _ => cell.transcript_lines(width),
    };
    lines
        .iter()
        .map(line_to_plain)
        .collect::<Vec<_>>()
        .join("\n")
}

fn open_tool_details_pager(app: &mut App) -> bool {
    let target_cell = detail_target_cell_index(app);

    let Some(cell_index) = target_cell else {
        return false;
    };
    open_details_pager_for_cell(app, cell_index)
}

/// Build the trailing "Spillover" section for the tool-details pager
/// (#500). Returns `None` when the cell at `cell_index` is not a
/// `GenericToolCell` with a recorded spillover path, or when the
/// spillover file is missing or unreadable. Failures fall back to a
/// short notice in the section so the user understands why the full
/// content can't be loaded — better than silent truncation.
fn spillover_pager_section(app: &App, cell_index: usize) -> Option<String> {
    use crate::tui::history::{GenericToolCell, HistoryCell, ToolCell};

    let cell = app.cell_at_virtual_index(cell_index)?;
    let HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
        spillover_path: Some(path),
        ..
    })) = cell
    else {
        return None;
    };
    let path_str = path.display().to_string();
    let body = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => format!("(could not read spillover file: {err})"),
    };
    Some(format!(
        "── Full output (spillover) ──\nFile: {path_str}\n\n{body}"
    ))
}

pub(crate) fn open_details_pager_for_cell(app: &mut App, cell_index: usize) -> bool {
    if let Some(detail) = app.tool_detail_record_for_cell(cell_index) {
        let input = serde_json::to_string_pretty(&detail.input)
            .unwrap_or_else(|_| detail.input.to_string());
        let output = detail.output.as_deref().map_or(
            "(not available)".to_string(),
            std::string::ToString::to_string,
        );

        // #500: when the tool result was spilled to disk, fold the full
        // file content into the pager body so the user can see what was
        // elided (the model only ever saw the head). The truncated head
        // stays above as `Output:` so the user can compare what the
        // model received against the full payload.
        let spillover_section = spillover_pager_section(app, cell_index);

        let content = if let Some(section) = spillover_section {
            format!(
                "Tool ID: {}\nTool: {}\n\nInput:\n{}\n\nOutput:\n{}\n\n{}",
                detail.tool_id, detail.tool_name, input, output, section
            )
        } else {
            format!(
                "Tool ID: {}\nTool: {}\n\nInput:\n{}\n\nOutput:\n{}",
                detail.tool_id, detail.tool_name, input, output
            )
        };

        let width = app
            .viewport
            .last_transcript_area
            .map(|area| area.width)
            .unwrap_or(80);
        app.view_stack.push(PagerView::from_text(
            format!("Tool: {}", detail.tool_name),
            &content,
            width.saturating_sub(2),
        ));
        return true;
    }

    let Some(cell) = app.cell_at_virtual_index(cell_index) else {
        app.status_message = Some("No details available for the selected line".to_string());
        return false;
    };
    let title = match cell {
        HistoryCell::User { .. } => "You".to_string(),
        HistoryCell::Assistant { .. } => "Assistant".to_string(),
        HistoryCell::System { .. } => "Note".to_string(),
        HistoryCell::Error { .. } => "Error".to_string(),
        HistoryCell::Thinking { .. } => "Reasoning".to_string(),
        HistoryCell::Tool(_) => "Message".to_string(),
        HistoryCell::SubAgent(_) => "Sub-agent".to_string(),
        HistoryCell::ArchivedContext { .. } => "Archived Context".to_string(),
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let content = history_cell_to_text(cell, width);
    app.view_stack.push(PagerView::from_text(
        title,
        &content,
        width.saturating_sub(2),
    ));
    true
}

/// Copy the "focused" transcript cell to the system clipboard.
/// The focused cell is determined by the detail-target heuristic
/// (viewport centre or most recent cell). Returns true when text
/// was actually copied.
fn copy_focused_cell(app: &mut App) -> bool {
    let cell_index = detail_target_cell_index(app);
    let Some(index) = cell_index else {
        return false;
    };
    copy_cell_to_clipboard(app, index)
}

pub(crate) fn copy_cell_to_clipboard(app: &mut App, cell_index: usize) -> bool {
    let Some(cell) = app.cell_at_virtual_index(cell_index) else {
        app.status_message = Some("No message at that line".to_string());
        return false;
    };
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let text = history_cell_to_text(cell, width);
    if text.trim().is_empty() {
        app.status_message = Some("Message is empty".to_string());
        return false;
    }
    if app.clipboard.write_text(&text).is_ok() {
        app.status_message = Some("Message copied".to_string());
        true
    } else {
        app.status_message = Some("Copy failed".to_string());
        false
    }
}

fn detail_target_cell_index(app: &App) -> Option<usize> {
    if let Some((start, _)) = app.viewport.transcript_selection.ordered_endpoints() {
        return app
            .viewport
            .transcript_cache
            .line_meta()
            .get(start.line_index)
            .and_then(|meta| meta.cell_line())
            .map(|(cell_index, _)| app.original_cell_index_for_rendered(cell_index));
    }

    app.detail_cell_index_for_viewport(
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_visible.max(1),
        app.viewport.transcript_cache.line_meta(),
    )
    .or_else(|| app.history.len().checked_sub(1))
}

pub(crate) fn selected_detail_footer_label(app: &App) -> Option<String> {
    if app.viewport.transcript_selection.is_active() {
        return None;
    }
    let cell_index = activity_footer_target_cell_index(app)?;
    let cell = app.cell_at_virtual_index(cell_index)?;
    let label = truncate_line_to_width(&activity_cell_label(app, cell_index, cell), 30);
    let detail_hint = if app.cell_has_detail_target(cell_index) {
        let noun = if matches!(cell, HistoryCell::SubAgent(_)) {
            "details"
        } else {
            "raw"
        };
        format!(
            " · {} {noun}",
            key_shortcuts::tool_details_shortcut_hint_label()
        )
    } else {
        String::new()
    };
    Some(format!(
        "{} Activity: {label}{detail_hint}",
        key_shortcuts::activity_shortcut_label()
    ))
}

fn activity_footer_target_cell_index(app: &App) -> Option<usize> {
    let line_meta = app.viewport.transcript_cache.line_meta();
    let start = app
        .viewport
        .last_transcript_top
        .min(line_meta.len().saturating_sub(1));
    let end = start
        .saturating_add(app.viewport.last_transcript_visible.max(1))
        .min(line_meta.len());
    for meta in line_meta.iter().take(end).skip(start) {
        let Some((cell_index, _)) = meta.cell_line() else {
            continue;
        };
        let cell_index = app.original_cell_index_for_rendered(cell_index);
        if app
            .cell_at_virtual_index(cell_index)
            .is_some_and(is_meaningful_activity_cell)
        {
            return Some(cell_index);
        }
    }

    activity_target_cell_index(app)
}

pub(crate) fn detail_target_label(app: &App, cell_index: usize) -> Option<String> {
    if let Some(detail) = app.tool_detail_record_for_cell(cell_index) {
        return Some(detail.tool_name.clone());
    }
    let cell = app.cell_at_virtual_index(cell_index)?;
    match cell {
        HistoryCell::Tool(ToolCell::Exec(exec)) => {
            Some(format!("run {}", one_line_summary(&exec.command, 80)))
        }
        HistoryCell::Tool(ToolCell::Exploring(explore)) => Some(format!(
            "workspace {} item{}",
            explore.entries.len(),
            if explore.entries.len() == 1 { "" } else { "s" }
        )),
        HistoryCell::Tool(ToolCell::PlanUpdate(_)) => Some("update plan".to_string()),
        HistoryCell::Tool(ToolCell::PatchSummary(patch)) => Some(format!("patch {}", patch.path)),
        HistoryCell::Tool(ToolCell::Review(review)) => {
            let target = one_line_summary(&review.target, 80);
            Some(if target.is_empty() {
                "review".to_string()
            } else {
                format!("review {target}")
            })
        }
        HistoryCell::Tool(ToolCell::DiffPreview(diff)) => Some(format!("diff {}", diff.title)),
        HistoryCell::Tool(ToolCell::Mcp(mcp)) => Some(format!("tool {}", mcp.tool)),
        HistoryCell::Tool(ToolCell::ViewImage(image)) => {
            Some(format!("image {}", image.path.display()))
        }
        HistoryCell::Tool(ToolCell::WebSearch(search)) => Some(format!("search {}", search.query)),
        HistoryCell::Tool(ToolCell::Generic(generic)) => Some(
            crate::tui::widgets::tool_card::tool_activity_label_for_name(
                &generic.name,
                app.ui_locale,
            ),
        ),
        HistoryCell::SubAgent(_) => Some("sub-agent".to_string()),
        _ => None,
    }
}

// Keyboard-shortcut predicates moved to `tui/key_shortcuts.rs`.

fn extract_reasoning_header(text: &str) -> Option<String> {
    let start = text.find("**")?;
    let rest = &text[start + 2..];
    let end = rest.find("**")?;
    let header = rest[..end].trim().trim_end_matches(':');
    if header.is_empty() {
        None
    } else {
        Some(header.to_string())
    }
}

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
        "v{latest} available - run `codewhale update` and restart"
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

#[cfg(test)]
mod tests;
