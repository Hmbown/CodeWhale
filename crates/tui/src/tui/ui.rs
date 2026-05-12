//! TUI event loop and rendering logic for `DeepSeek` CLI.

use std::collections::HashSet;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEvent, MouseEventKind,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    layout::{Constraint, Direction, Layout, Rect, Size},
    prelude::Widget,
    style::Style,
    text::Span,
    widgets::Block,
};
use tracing;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::audit::log_sensitive_event;
use crate::automation_manager::{AutomationManager, AutomationSchedulerConfig, spawn_scheduler};
use crate::client::{DeepSeekClient, build_cache_warmup_request};
use crate::commands;
use crate::compaction::estimate_input_tokens_conservative;
use crate::config::{ApiProvider, Config, DEFAULT_NVIDIA_NIM_BASE_URL};
use crate::config_ui::{self, ConfigUiMode, WebConfigSession, WebConfigSessionEvent};
use crate::core::coherence::CoherenceState;
use crate::core::engine::{EngineConfig, EngineHandle, spawn_engine};
use crate::core::events::Event as EngineEvent;
use crate::core::ops::Op;
use crate::hooks::HookEvent;
use crate::llm_client::LlmClient;
use crate::models::{
    ContentBlock, Message, MessageRequest, SystemPrompt, Usage, context_window_for_model,
};
use crate::palette;
use crate::prompts;
use crate::session_manager::{
    OfflineQueueState, QueuedSessionMessage, SavedSession, SessionManager,
    create_saved_session_with_id_and_mode, create_saved_session_with_mode, update_session,
};
use crate::task_manager::{
    NewTaskRequest, SharedTaskManager, TaskManager, TaskManagerConfig, TaskStatus,
};
use crate::tools::spec::RuntimeToolServices;
use crate::tools::subagent::SubAgentStatus;
use crate::tui::color_compat::ColorCompatBackend;
use crate::tui::command_palette::{
    CommandPaletteView, build_entries as build_command_palette_entries,
};
use crate::tui::context_inspector::build_context_inspector_text;
use crate::tui::context_menu::{ContextMenuEntry, ContextMenuView};
use crate::tui::event_broker::EventBroker;
use crate::tui::live_transcript::LiveTranscriptOverlay;
use crate::tui::mcp_routing::{add_mcp_message, open_mcp_manager_pager};
use crate::tui::onboarding;
use crate::tui::pager::PagerView;
use crate::tui::persistence_actor::{self, PersistRequest};
use crate::tui::plan_prompt::PlanPromptView;
use crate::tui::scrolling::{ScrollDirection, TranscriptScroll};
use crate::tui::selection::{SelectionAutoscroll, TranscriptSelectionPoint};
use crate::tui::session_picker::SessionPickerView;
use crate::tui::shell_job_routing::{
    add_shell_job_message, format_shell_job_list, format_shell_poll, open_shell_job_pager,
};
use crate::tui::subagent_routing::{
    active_fanout_counts, format_task_list, handle_subagent_mailbox, open_task_pager,
    reconcile_subagent_activity_state, running_agent_count, sort_subagents_in_place,
    task_mode_label, task_summary_to_panel_entry,
};
#[cfg(test)]
use crate::tui::tool_routing::exploring_label;
use crate::tui::tool_routing::{
    handle_tool_call_complete, handle_tool_call_started, maybe_add_patch_preview,
};
use crate::tui::ui_text::{history_cell_to_text, line_to_plain, slice_text, text_display_width};
use crate::tui::user_input::UserInputView;
use crate::tui::views::subagent_view_agents;

use super::active_cell::ActiveCell;
use super::app::{
    App, AppAction, AppMode, OnboardingState, QueuedMessage, ReasoningEffort, SidebarFocus,
    StatusToastLevel, SubmitDisposition, TaskPanelEntry, ToolDetailRecord, TuiOptions,
};
use super::approval::{
    ApprovalMode, ApprovalRequest, ApprovalView, ElevationRequest, ElevationView, ReviewDecision,
};
use super::history::{
    HistoryCell, ToolCell, ToolStatus, history_cells_from_message, summarize_tool_output,
};
use super::slash_menu::{
    apply_slash_menu_selection, try_autocomplete_slash_command, visible_slash_menu_entries,
};
use super::views::{
    ConfigView, ContextMenuAction, HelpView, ModalKind, ShellControlView, ViewEvent,
};
use super::widgets::pending_input_preview::{ContextPreviewItem, PendingInputPreview};
use super::widgets::{
    ChatWidget, ComposerWidget, FooterProps, FooterToast, FooterWidget, HeaderData, HeaderWidget,
    Renderable,
};

// === Constants ===

/// Upper bound on slash-menu entries returned to the renderer. The composer's
/// render path already paginates with center-tracking (see
/// `widgets::ComposerWidget::render`), so this only needs to be high enough to
/// encompass the full filtered command list — never the visible-row budget.
/// Bumped from 6 to 128 to fix #64 (selection couldn't reach commands beyond
/// the visible window because the source list itself was capped).
const SLASH_MENU_LIMIT: usize = 128;
const MENTION_MENU_LIMIT: usize = 6;
const MIN_CHAT_HEIGHT: u16 = 3;
const MIN_COMPOSER_HEIGHT: u16 = 2;
const CONTEXT_WARNING_THRESHOLD_PERCENT: f64 = 85.0;
const CONTEXT_CRITICAL_THRESHOLD_PERCENT: f64 = 95.0;
const UI_IDLE_POLL_MS: u64 = 48;
const UI_ACTIVE_POLL_MS: u64 = 24;
const WEB_CONFIG_POLL_MS: u64 = 16;
const DISPATCH_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(30);
// Forced repaint cadence while a turn is live (model loading, compacting,
// sub-agents running). Drives the footer water-spout animation as well as
// the per-tool spinner pulse — keep this fast enough that the spout reads as
// motion (~12 fps) instead of teleport-frames.
const UI_STATUS_ANIMATION_MS: u64 = 80;
const WORKSPACE_CONTEXT_REFRESH_SECS: u64 = 15;
const SIDEBAR_VISIBLE_MIN_WIDTH: u16 = 100;
const DEFAULT_TERMINAL_PROBE_TIMEOUT_MS: u64 = 500;

type AppTerminal = Terminal<ColorCompatBackend<Stdout>>;
// Reset scroll region (`\x1b[r`), origin mode (`\x1b[?6l`), and home the cursor
// (`\x1b[H`) before letting ratatui's diff renderer repaint. The destructive
// `\x1b[2J\x1b[3J` pair was previously appended here to also wipe the visible
// screen and saved scrollback, but combined with the immediately-following
// `terminal.clear()` it produced a double-clear that several terminals
// (Ghostty, VSCode terminal, Win10 conhost) render as visible flicker on every
// TurnComplete / focus-gain / resize. The alt-screen buffer's double-buffering
// plus ratatui's `terminal.clear()` are sufficient to repaint cleanly.
const TERMINAL_ORIGIN_RESET: &[u8] = b"\x1b[r\x1b[?6l\x1b[H";
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
    // Default-off on Windows because legacy `cmd.exe` and pre-Win11
    // PowerShell consoles don't always honor the OSC 8 string
    // terminator (`ESC \`) cleanly — emitting the escape can leave
    // stray bytes that eat the leading column of the next line and
    // duplicate the composer panel during scroll. Reported on a
    // Windows session (issue forthcoming, screenshot showed
    // "eepseek-v4-flash" with the leading `d` consumed and three
    // overlapping composer panels). v0.8.8 also surfaced macOS
    // corruption ("526sOPEN" instead of "526   OPEN") because OSC 8
    // wrappers are emitted inside ratatui `Span` content; ratatui's
    // grapheme filter drops the bare ESC byte but paints every other
    // byte of the wrapper into a buffer cell, drifting columns. Until
    // OSC 8 is emitted out-of-band of the buffer pipeline, default off
    // on every platform; opt back in via `[ui] osc8_links = true`.
    let osc8_default_on = false;
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
        enable_raw_mode().map_err(|e| anyhow::anyhow!("Failed to enable raw mode: {}", e))
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

    let mut stdout = io::stdout();
    if use_alt_screen {
        execute!(stdout, EnterAlternateScreen)?;
    }
    // Initialize the file-backed TUI log and (on Unix) redirect raw stderr
    // away from the alt-screen for the lifetime of this guard. Any
    // `eprintln!`, panic message, or third-party stderr write that would
    // otherwise leak into the alt-screen buffer and shift ratatui's
    // diff-renderer view (the "scroll demon" reported in #1085) now lands
    // in `~/.deepseek/logs/tui-YYYY-MM-DD.log` instead. The guard is held
    // until the function returns; dropping it (after `LeaveAlternateScreen`
    // below) restores the original stderr fd so shutdown messages reach
    // the user's terminal. We accept the init failing (e.g., read-only
    // `$HOME`) and continue without the redirect rather than refusing to
    // start the TUI.
    let _tui_log_guard = match crate::runtime_log::init() {
        Ok(guard) => Some(guard),
        Err(err) => {
            tracing::warn!(target: "runtime_log", ?err, "TUI log init failed; stderr leaks may render as scroll-demon");
            None
        }
    };
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
    recover_terminal_modes(&mut stdout, use_mouse_capture, use_bracketed_paste);
    let color_depth = palette::ColorDepth::detect();
    let palette_mode = palette::PaletteMode::detect();
    tracing::debug!(
        ?color_depth,
        ?palette_mode,
        "terminal color profile detected"
    );
    let backend = ColorCompatBackend::new(stdout, color_depth, palette_mode);
    let mut terminal = Terminal::new(backend)?;
    reset_terminal_viewport(&mut terminal)?;
    let event_broker = EventBroker::new();

    // Local mutable copy so runtime config flips (e.g. `/provider` switch)
    // can rebuild the API client without restarting the process.
    let mut config = config.clone();
    let config = &mut config;
    let mut app = App::new(options.clone(), config);

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
                let recovered = apply_loaded_session(&mut app, &saved);
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
    app.runtime_services = RuntimeToolServices {
        shell_manager: Some(shell_manager),
        task_manager: Some(task_manager.clone()),
        automations: Some(automations),
        task_data_dir: Some(task_manager.data_dir()),
        active_task_id: None,
        active_thread_id: None,
        // #456: plumb the App's HookExecutor so `exec_shell` can surface
        // the configured `shell_env` hooks. Wrapped in Arc once and shared.
        hook_executor: Some(std::sync::Arc::new(app.hooks.clone())),
    };
    refresh_active_task_panel(&mut app, &task_manager).await;

    let engine_config = build_engine_config(&app, config);

    // Spawn the Engine - it will handle all API communication
    let engine_handle = spawn_engine(engine_config, config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
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

    let result = run_event_loop(
        &mut terminal,
        &mut app,
        config,
        engine_handle,
        task_manager,
        &event_broker,
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

    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(terminal.backend_mut(), DisableFocusChange)?;
    disable_raw_mode()?;
    if use_alt_screen {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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
    "To continue this session, execute deepseek run --continue"
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
    EngineConfig {
        model: app.model.clone(),
        workspace: app.workspace.clone(),
        allow_shell: app.allow_shell,
        trust_mode: app.trust_mode,
        notes_path: config.notes_path(),
        mcp_config_path: config.mcp_config_path(),
        skills_dir: app.skills_dir.clone(),
        instructions: config.instructions_paths(),
        project_context_pack_enabled: config.project_context_pack_enabled(),
        // Effectively unlimited. V4 has a 1M context window and the user
        // wants the model running until it's actually done. The previous cap
        // of 100 hit the ceiling on long multi-step plans (wide refactors,
        // sub-agent orchestration) and presented as the agent "giving up
        // mid-task". `u32::MAX` is the type ceiling; users can still
        // interrupt with Ctrl+C / Esc, and a turn naturally ends when the
        // model stops emitting tool calls. A real runaway is rare and
        // human-noticeable; we trust the operator over a hard step cap.
        max_steps: u32::MAX,
        max_subagents: app.max_subagents,
        features: config.features(),
        compaction: app.compaction_config(),
        cycle: app.cycle_config(),
        capacity: crate::core::capacity::CapacityControllerConfig::from_app_config(config),
        todos: app.todos.clone(),
        plan_state: app.plan_state.clone(),
        max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
        network_policy: config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        }),
        snapshots_enabled: config.snapshots_config().enabled,
        lsp_config: config
            .lsp
            .clone()
            .map(crate::config::LspConfigToml::into_runtime),
        runtime_services: app.runtime_services.clone(),
        subagent_model_overrides: config.subagent_model_overrides(),
        memory_enabled: config.memory_enabled(),
        memory_path: config.memory_path(),
        strict_tool_mode: config.strict_tool_mode.unwrap_or(false),
        goal_objective: app.goal.goal_objective.clone(),
        locale_tag: app.ui_locale.tag().to_string(),
        workshop: config.workshop.clone(),
    }
}

async fn refresh_active_task_panel(app: &mut App, task_manager: &SharedTaskManager) {
    let tasks = task_manager.list_tasks(None).await;
    let mut entries: Vec<TaskPanelEntry> = tasks
        .into_iter()
        .filter(|task| matches!(task.status, TaskStatus::Queued | TaskStatus::Running))
        .map(task_summary_to_panel_entry)
        .collect();

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
            });
        }
    }

    app.task_panel = entries;
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
            if generic.name != "rlm" || generic.status != ToolStatus::Running {
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
            })
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
async fn run_event_loop(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    mut engine_handle: EngineHandle,
    task_manager: SharedTaskManager,
    event_broker: &EventBroker,
) -> Result<()> {
    // Track streaming state
    let mut current_streaming_text = String::new();
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
    // #376: native-copy escape — hold Shift to bypass alt-screen mouse capture
    // for terminal-native text selection.
    let mut shift_bypass_active = false;
    let mut terminal_paused_at: Option<Instant> = None;
    let mut force_terminal_repaint = false;

    loop {
        if !drain_web_config_events(&mut web_config_session, app, config, &engine_handle).await {
            web_config_session = None;
        }

        if last_task_refresh.elapsed() >= Duration::from_millis(2500) {
            refresh_active_task_panel(app, &task_manager).await;
            last_task_refresh = Instant::now();
            app.needs_redraw = true;
        }

        // First, poll for engine events (non-blocking)
        let mut received_engine_event = false;
        let mut transcript_batch_updated = false;
        let mut queued_to_send: Option<QueuedMessage> = None;
        {
            let mut rx = engine_handle.rx_event.write().await;
            while let Ok(event) = rx.try_recv() {
                received_engine_event = true;
                match event {
                    EngineEvent::MessageStarted { .. } => {
                        // Assistant text starting after parallel tool work
                        // means the tool group is done. Flush the active
                        // cell first so the message lands BELOW the
                        // committed tool group (Codex pattern: streamed
                        // assistant content always flows after work).
                        app.flush_active_cell();
                        current_streaming_text.clear();
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
                            if finalize_current_streaming_thinking(app) {
                                transcript_batch_updated = true;
                            }
                            stash_reasoning_buffer_into_last_reasoning(app);
                        }
                        if let Some(index) = app.streaming_message_index.take() {
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

                        let mut blocks = Vec::new();
                        let thinking = app.last_reasoning.take();
                        if let Some(thinking) = thinking {
                            blocks.push(ContentBlock::Thinking { thinking });
                        }
                        if !current_streaming_text.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: current_streaming_text.clone(),
                                cache_control: None,
                            });
                        }
                        for (id, name, input) in app.pending_tool_uses.drain(..) {
                            blocks.push(ContentBlock::ToolUse {
                                id,
                                name,
                                input,
                                caller: None,
                            });
                        }

                        // DeepSeek rejects assistant messages that contain only reasoning blocks.
                        // Keep reasoning in transcript cells, but only persist assistant turns that
                        // include visible text and/or tool calls.
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
                    EngineEvent::ThinkingStarted { .. } => {
                        // P2.3: thinking lives in the active cell so it groups
                        // visually with the tool calls that follow until the
                        // next assistant prose chunk flushes the group.
                        if start_streaming_thinking_block(app) {
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

                        let entry_idx = ensure_streaming_thinking_active_entry(app);
                        app.streaming_state.push_content(0, &sanitized);
                        let committed = app.streaming_state.commit_text(0);
                        if !committed.is_empty() {
                            append_streaming_thinking(app, entry_idx, &committed);
                            transcript_batch_updated = true;
                        }
                    }
                    EngineEvent::ThinkingComplete { .. } => {
                        if finalize_current_streaming_thinking(app) {
                            transcript_batch_updated = true;
                        }
                        stash_reasoning_buffer_into_last_reasoning(app);
                    }
                    EngineEvent::ToolCallStarted { id, name, input } => {
                        app.pending_tool_uses
                            .push((id.clone(), name.clone(), input.clone()));
                        // Note this dispatch so the next sub-agent `Started`
                        // mailbox envelope routes into the right card kind
                        // (delegate vs fanout).
                        if matches!(name.as_str(), "agent_spawn" | "rlm" | "delegate") {
                            app.pending_subagent_dispatch = Some(name.clone());
                            if name == "rlm" {
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
                        let tool_content = match &result {
                            Ok(output) => sanitize_stream_chunk(
                                &crate::core::engine::compact_tool_result_for_context(
                                    &app.model, &name, output,
                                ),
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
                        handle_tool_call_complete(app, &id, &name, &result);

                        // Immediately refresh the task panel sidebar when a
                        // tool that changes task state completes, so the
                        // Tasks panel stays in sync with tool execution
                        // rather than waiting up to 2.5 s for the periodic
                        // poll. Also merge shell jobs (#373).
                        if matches!(
                            name.as_str(),
                            "agent_spawn"
                                | "agent_cancel"
                                | "todo_write"
                                | "task_shell_start"
                                | "exec_shell"
                        ) {
                            refresh_active_task_panel(app, &task_manager).await;
                            last_task_refresh = Instant::now();
                        }
                        if matches!(
                            name.as_str(),
                            "agent_spawn"
                                | "agent_cancel"
                                | "agent_wait"
                                | "agent_result"
                                | "agent_status"
                        ) {
                            let _ = engine_handle.send(Op::ListSubAgents).await;
                        }
                    }
                    EngineEvent::TurnStarted { turn_id } => {
                        app.is_loading = true;
                        app.offline_mode = false;
                        app.turn_error_posted = false;
                        app.dispatch_started_at = None;
                        current_streaming_text.clear();
                        app.streaming_state.reset();
                        app.streaming_message_index = None;
                        app.streaming_thinking_active_entry = None;
                        app.turn_started_at = Some(Instant::now());
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
                    } => {
                        force_terminal_repaint = true;
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
                        app.offline_mode = false;
                        app.streaming_state.reset();
                        // Capture elapsed before clearing turn_started_at so
                        // notifications can use the real wall-clock duration.
                        let turn_elapsed =
                            app.turn_started_at.map(|t| t.elapsed()).unwrap_or_default();
                        app.turn_started_at = None;
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
                            let _ = engine_handle.send(Op::ListSubAgents).await;
                        }
                        let turn_tokens = usage.input_tokens + usage.output_tokens;
                        app.session.total_tokens =
                            app.session.total_tokens.saturating_add(turn_tokens);
                        app.session.total_conversation_tokens = app
                            .session
                            .total_conversation_tokens
                            .saturating_add(turn_tokens);
                        app.session.last_prompt_tokens = Some(usage.input_tokens);
                        app.session.last_completion_tokens = Some(usage.output_tokens);
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
                        if let Some(error) = error {
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

                        // Emit OSC 9 / BEL desktop notification for long turns.
                        if status == crate::core::events::TurnOutcomeStatus::Completed
                            && let Some((method, threshold, include_summary)) =
                                notification_settings(config)
                        {
                            let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                            let msg = completed_turn_notification_message(
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
                                app.view_stack.push(PlanPromptView::new());
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

                        if queued_to_send.is_none() {
                            queued_to_send = app.pop_queued_message();
                        }
                    }
                    EngineEvent::Error {
                        envelope,
                        recoverable: _,
                    } => {
                        apply_engine_error_to_app(app, envelope);
                    }
                    EngineEvent::Status { message } => {
                        app.status_message = Some(message);
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
                            app.model = model;
                            app.last_effective_model = None;
                        }
                        app.update_model_compaction_budget();
                        app.workspace = workspace;
                        if (app.is_loading || app.is_compacting)
                            && let Ok(manager) = SessionManager::default_location()
                        {
                            let session = build_session_snapshot(app, &manager);
                            persistence_actor::persist(PersistRequest::Checkpoint(session));
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
                    EngineEvent::CycleAdvanced { from, to, briefing } => {
                        // Mirror the engine-side counter on the UI app state
                        // so the sidebar / slash commands stay in sync, and
                        // record the briefing so `/cycle <n>` can show it.
                        app.cycle_count = to;
                        let briefing_tokens = briefing.token_estimate;
                        app.cycle_briefings.push(briefing);
                        let separator = format!(
                            "─── cycle {from} → {to}  (briefing: {briefing_tokens} tokens) ───"
                        );
                        app.add_message(HistoryCell::System { content: separator });
                        app.status_message = Some(format!(
                            "↻ context refreshed (cycle {from} → {to}, briefing: {briefing_tokens} tokens carried)"
                        ));
                    }
                    EngineEvent::CoherenceState { state, .. } => {
                        app.coherence_state = state;
                    }
                    EngineEvent::CapacityDecision { .. } => {
                        // Telemetry-only event. Surface actual interventions and failures
                        // instead of replacing the footer with no-op guardrail chatter.
                    }
                    EngineEvent::CapacityIntervention {
                        action,
                        before_prompt_tokens,
                        after_prompt_tokens,
                        ..
                    } => {
                        app.status_message = Some(format!(
                            "Capacity intervention: {action} (~{before_prompt_tokens} -> ~{after_prompt_tokens} tokens)"
                        ));
                    }
                    EngineEvent::CapacityMemoryPersistFailed { action, error, .. } => {
                        app.status_message = Some(format!(
                            "Capacity memory persist failed ({action}): {error}"
                        ));
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
                            )?;
                            event_broker.resume_events();
                            terminal_paused_at = None;
                        }
                    }
                    EngineEvent::AgentSpawned { id, prompt } => {
                        let prompt_summary = summarize_tool_output(&prompt);
                        app.agent_progress
                            .insert(id.clone(), format!("starting: {prompt_summary}"));
                        if app.agent_activity_started_at.is_none() {
                            app.agent_activity_started_at = Some(Instant::now());
                        }
                        app.status_message =
                            Some(format!("Sub-agent {id} starting: {prompt_summary}"));
                        let _ = engine_handle.send(Op::ListSubAgents).await;
                    }
                    EngineEvent::AgentProgress { id, status } => {
                        let display = friendly_subagent_progress(app, &id, &status);
                        if is_noisy_subagent_progress(&status) {
                            app.agent_progress
                                .entry(id.clone())
                                .or_insert_with(|| display.clone());
                        } else {
                            app.agent_progress.insert(id.clone(), display.clone());
                        }
                        if app.agent_activity_started_at.is_none() {
                            app.agent_activity_started_at = Some(Instant::now());
                        }
                        app.status_message = Some(format!("Sub-agent {id}: {display}"));
                    }
                    EngineEvent::AgentComplete { id, result } => {
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
                        app.status_message = Some(format!(
                            "Sub-agent {id} completed: {}",
                            summarize_tool_output(&result)
                        ));
                        let should_recapture_terminal =
                            !has_other_running_subagents && app.use_alt_screen;
                        if !has_other_running_subagents
                            && let Some((method, threshold, include_summary)) =
                                notification_settings(config)
                        {
                            let in_tmux = std::env::var("TMUX").is_ok_and(|v| !v.is_empty());
                            let msg = subagent_completion_notification_message(
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
                        if should_recapture_terminal {
                            resume_terminal(
                                terminal,
                                app.use_alt_screen,
                                app.use_mouse_capture,
                                app.use_bracketed_paste,
                            )?;
                            event_broker.resume_events();
                            terminal_paused_at = None;
                            app.needs_redraw = true;
                        }
                        let _ = engine_handle.send(Op::ListSubAgents).await;
                    }
                    EngineEvent::AgentList { agents } => {
                        let mut sorted = agents.clone();
                        sort_subagents_in_place(&mut sorted);
                        sorted.retain(|a| !a.from_prior_session);
                        app.subagent_cache = sorted.clone();
                        reconcile_subagent_activity_state(app);
                        let view_agents = subagent_view_agents(app, &sorted);
                        if app.view_stack.update_subagents(&view_agents) {
                            app.status_message =
                                Some(format!("Sub-agents: {} total", view_agents.len()));
                        }
                        // Individual spawn/complete events already log to history;
                        // full list available via /agents command.
                    }
                    EngineEvent::SubAgentMailbox { seq, message } => {
                        handle_subagent_mailbox(app, seq, &message);
                        transcript_batch_updated = true;
                    }
                    EngineEvent::ApprovalRequired {
                        id,
                        tool_name,
                        description,
                        approval_key,
                    } => {
                        let session_approved =
                            app.approval_session_approved.contains(&approval_key)
                                || app.approval_session_approved.contains(&tool_name);
                        let session_denied = app.approval_session_denied.contains(&approval_key)
                            || app.approval_session_denied.contains(&tool_name);
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
                        } else if session_approved || app.approval_mode == ApprovalMode::Auto {
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
                            let tool_input = app
                                .pending_tool_uses
                                .iter()
                                .find(|(tool_id, _, _)| tool_id == &id)
                                .map(|(_, _, input)| input.clone())
                                .unwrap_or_else(|| serde_json::json!({}));

                            if tool_name == "apply_patch" {
                                maybe_add_patch_preview(app, &tool_input);
                            }

                            // Create approval request and show overlay
                            let request = ApprovalRequest::new(
                                &id,
                                &tool_name,
                                &description,
                                &tool_input,
                                &approval_key,
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
                            app.view_stack
                                .push(ApprovalView::new_for_locale(request, app.ui_locale));
                            app.status_message = Some(format!(
                                "Approval required for '{tool_name}': {description}"
                            ));
                        }
                    }
                    EngineEvent::UserInputRequired { id, request } => {
                        app.view_stack.push(UserInputView::new(id.clone(), request));
                        app.status_message = Some(
                            "Action required: answer the popup with 1-4, arrows, or Enter"
                                .to_string(),
                        );
                    }
                    EngineEvent::ToolCallProgress { id, output } => {
                        app.status_message =
                            Some(format!("Tool {id}: {}", summarize_tool_output(&output)));
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
                            app.view_stack.push(ElevationView::new(request));
                            app.status_message =
                                Some(format!("Sandbox blocked {tool_name}: {denial_reason}"));
                        }
                    }
                }
            }
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
                append_streaming_thinking(app, entry_idx, &committed);
                transcript_batch_updated = true;
            }
        }
        if transcript_batch_updated {
            app.mark_history_updated();
        }
        if received_engine_event {
            app.needs_redraw = true;
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
        if (app.is_loading || has_running_agents || app.is_compacting)
            && last_status_frame.elapsed()
                >= Duration::from_millis(status_animation_interval_ms(app))
        {
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
        // While the user is drag-selecting past the transcript edge, advance
        // the viewport on a fixed cadence and extend the selection head so a
        // long passage can be selected in one drag (#1163).
        tick_selection_autoscroll(app);
        let allow_workspace_context_refresh =
            !app.is_loading && !has_running_agents && !app.is_compacting;
        refresh_workspace_context_if_needed(app, now, allow_workspace_context_refresh);

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
        if app.needs_redraw && draw_wait.is_none() {
            draw_app_frame_inner(terminal, app, force_terminal_repaint)?;
            force_terminal_repaint = false;
            frame_rate_limiter.mark_emitted(Instant::now());
            app.needs_redraw = false;
        }

        let mut poll_timeout = if app.is_loading || has_running_agents || app.is_compacting {
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

        // #549: this async task also performs a blocking terminal poll. Give
        // the engine task a scheduler turn before we block again so an
        // interactive submit can reach the API instead of appearing stuck on
        // `working.` with no network activity.
        tokio::task::yield_now().await;

        if event::poll(poll_timeout)? {
            let evt = event::read()?;
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
                    sync_api_key_validation_status(app, false);
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
                recover_terminal_modes(
                    terminal.backend_mut(),
                    app.use_mouse_capture,
                    app.use_bracketed_paste,
                );
                force_terminal_repaint = true;
                app.needs_redraw = true;
            }
            if let Event::Resize(width, height) = evt {
                tracing::debug!(
                    width,
                    height,
                    coherence = ?app.coherence_state,
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
                while event::poll(Duration::from_millis(0)).unwrap_or(false) {
                    match event::read() {
                        Ok(Event::Resize(w, h)) => {
                            final_w = w;
                            final_h = h;
                        }
                        Ok(other) => {
                            // Non-resize event during the drain: we can't
                            // un-read it. Drop it and let the user re-issue
                            // — the resize-coalesce window is tiny.
                            tracing::debug!(
                                ?other,
                                "non-resize event during resize coalesce; dropping"
                            );
                            break;
                        }
                        Err(_) => break,
                    }
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

                reset_terminal_viewport(terminal)?;
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
                    backend.force_size(Size::new(final_w, final_h));
                }
                // Draw immediately so the cleared screen gets repainted before
                // any other events can interleave. Without this, the next
                // iteration's draw can race against fast follow-up input and
                // leave the user staring at a blank/partial frame.
                draw_app_frame(terminal, app)?;
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
                // #376: hold Shift to bypass alt-screen mouse capture for
                // terminal-native text selection. While bypass is active,
                // mouse events pass through to the terminal instead of
                // being consumed by the TUI.
                if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    if !shift_bypass_active {
                        let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
                        shift_bypass_active = true;
                        app.push_status_toast(
                            "Native selection \u{2014} release Shift to return",
                            StatusToastLevel::Info,
                            Some(3_000),
                        );
                    }
                    // Let the terminal handle this mouse event natively.
                    continue;
                }
                if shift_bypass_active {
                    let _ = execute!(terminal.backend_mut(), EnableMouseCapture);
                    shift_bypass_active = false;
                    app.push_status_toast(
                        "Mouse capture restored",
                        StatusToastLevel::Info,
                        Some(2_000),
                    );
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
                continue;
            }

            let Event::Key(key) = evt else {
                continue;
            };

            if key.kind != KeyEventKind::Press {
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
                    // Language picker hotkeys: 1-5 select + persist (#566).
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
                                    advance_onboarding_after_language(app);
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
                            advance_onboarding_from_welcome(app);
                        }
                        OnboardingState::Language => {
                            // Enter without a digit pick keeps the existing
                            // setting (which defaults to "auto").
                            advance_onboarding_after_language(app);
                        }
                        OnboardingState::ApiKey => {
                            let key = app.api_key_input.trim().to_string();
                            if let ApiKeyValidation::Reject(message) =
                                validate_api_key_for_onboarding(&key)
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
                                                model: app.model.clone(),
                                                workspace: app.workspace.clone(),
                                            })
                                            .await;
                                    }

                                    advance_onboarding_after_language(app);
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
                        sync_api_key_validation_status(app, false);
                    }
                    KeyCode::Char('h')
                        if is_ctrl_h_backspace(&key)
                            && app.onboarding == OnboardingState::ApiKey =>
                    {
                        app.delete_api_key_char();
                        sync_api_key_validation_status(app, false);
                    }
                    _ if is_paste_shortcut(&key) && app.onboarding == OnboardingState::ApiKey => {
                        // Cmd+V / Ctrl+V paste (bracketed paste handled above)
                        app.paste_api_key_from_clipboard();
                        sync_api_key_validation_status(app, false);
                    }
                    KeyCode::Char(c)
                        if app.onboarding == OnboardingState::ApiKey && is_text_input_key(&key) =>
                    {
                        app.insert_api_key_char(c);
                        sync_api_key_validation_status(app, false);
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
                        &app.workspace,
                        &app.mcp_config_path,
                        app.mcp_snapshot.as_ref(),
                    )));
                continue;
            }

            // Shifted shortcuts toggle the file-tree pane. Keep plain Ctrl+E
            // reserved for the composer end-of-line binding used by shells.
            if is_file_tree_toggle_shortcut(&key) {
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
            // composer is focused (no other modal on top of the stack) and the
            // engine is not actively streaming a turn.
            if key.code == KeyCode::Char('p')
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && app.view_stack.is_empty()
                && !app.is_loading
            {
                open_file_picker(app);
                continue;
            }

            if matches!(key.code, KeyCode::Char('b') | KeyCode::Char('B'))
                && key.modifiers.contains(KeyModifiers::CONTROL)
                && app.view_stack.is_empty()
            {
                open_shell_control(app);
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
                continue;
            }

            // File-tree navigation: intercept keys when the file-tree pane is
            // visible so Up/Down/Enter/Esc operate on the tree rather than
            // falling through to composer or modal handlers.
            if app.file_tree.is_some() {
                match key.code {
                    KeyCode::Up => {
                        if let Some(state) = app.file_tree.as_mut() {
                            state.cursor_up();
                        }
                        app.needs_redraw = true;
                        continue;
                    }
                    KeyCode::Down => {
                        if let Some(state) = app.file_tree.as_mut() {
                            state.cursor_down();
                        }
                        app.needs_redraw = true;
                        continue;
                    }
                    KeyCode::Enter => {
                        if let Some(state) = app.file_tree.as_mut() {
                            if let Some(rel_path) = state.activate() {
                                // Insert @path into the composer.
                                let path_str = rel_path.to_string_lossy().to_string();
                                app.status_message = Some(format!("Attached @{path_str}"));
                                app.insert_str(&format!("@{} ", path_str));
                            } else {
                                // Directory was expanded/collapsed; rebuild.
                                app.needs_redraw = true;
                            }
                        }
                        continue;
                    }
                    KeyCode::Esc => {
                        app.file_tree = None;
                        app.status_message = Some("File tree closed".to_string());
                        app.needs_redraw = true;
                        continue;
                    }
                    _ => {}
                }
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

            if is_macos_option_v_legacy_key(&key) {
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
            let mention_menu_entries =
                crate::tui::file_mention::visible_mention_menu_entries(app, MENTION_MENU_LIMIT);
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
                KeyCode::Char('l')
                    if alt_nav_modifiers(key.modifiers)
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
                        && open_thinking_pager(app) =>
                {
                    continue;
                }
                KeyCode::Char('t') | KeyCode::Char('T')
                    if key.modifiers == KeyModifiers::CONTROL =>
                {
                    toggle_live_transcript_overlay(app);
                    continue;
                }
                KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::ALT) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        app.set_sidebar_focus(SidebarFocus::Plan);
                        app.status_message = Some("Sidebar focus: plan".to_string());
                    } else {
                        app.set_mode(AppMode::Plan);
                    }
                    continue;
                }
                KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::ALT) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        app.set_sidebar_focus(SidebarFocus::Todos);
                        app.status_message = Some("Sidebar focus: todos".to_string());
                    } else {
                        app.set_mode(AppMode::Agent);
                    }
                    continue;
                }
                KeyCode::Char('3') if key.modifiers.contains(KeyModifiers::ALT) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        app.set_sidebar_focus(SidebarFocus::Tasks);
                        app.status_message = Some("Sidebar focus: tasks".to_string());
                    } else {
                        app.set_mode(AppMode::Yolo);
                    }
                    continue;
                }
                KeyCode::Char('4') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_alt_4_shortcut(app, key.modifiers);
                    continue;
                }
                KeyCode::Char('!') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Plan);
                    app.status_message = Some("Sidebar focus: plan".to_string());
                    continue;
                }
                KeyCode::Char('@') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Todos);
                    app.status_message = Some("Sidebar focus: todos".to_string());
                    continue;
                }
                KeyCode::Char('#') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Tasks);
                    app.status_message = Some("Sidebar focus: tasks".to_string());
                    continue;
                }
                KeyCode::Char('$') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Agents);
                    app.status_message = Some("Sidebar focus: agents".to_string());
                    continue;
                }
                KeyCode::Char('%') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Context);
                    app.status_message = Some("Sidebar focus: context".to_string());
                    continue;
                }
                KeyCode::Char(')') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Auto);
                    app.status_message = Some("Sidebar focus: auto".to_string());
                    continue;
                }
                KeyCode::Char('0') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_sidebar_focus(SidebarFocus::Auto);
                    app.status_message = Some("Sidebar focus: auto".to_string());
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
                KeyCode::Char('c') | KeyCode::Char('C') if is_copy_shortcut(&key) => {
                    copy_active_selection(app);
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
                            app.is_loading = false;
                            app.dispatch_started_at = None;
                            app.streaming_state.reset();
                            // Optimistically clear the turn-in-progress flag
                            // so the footer wave animation halts immediately —
                            // without this, the strip keeps animating until
                            // the engine eventually emits TurnComplete (#5a).
                            // The engine's eventual TurnComplete event will
                            // overwrite with the real outcome ("interrupted").
                            app.runtime_turn_status = None;
                            app.status_message = Some("Request cancelled".to_string());
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
                            engine_handle.cancel();
                            app.is_loading = false;
                            app.dispatch_started_at = None;
                            app.streaming_state.reset();
                            // Optimistically halt the wave + working label —
                            // engine's TurnComplete will resync with the real
                            // outcome. Fixes #5a (wave kept animating after Esc).
                            app.runtime_turn_status = None;
                            // Finalize any in-flight tool entries optimistically so
                            // the composer regains focus and the footer's "tool ...
                            // · X active" chip clears immediately rather than
                            // waiting for the engine's TurnComplete echo to drain.
                            // Idempotent with the TurnComplete handler that runs
                            // when the engine actually echoes the cancel (#243).
                            // Background sub-agents continue running — they are
                            // tracked via `subagent_cache` independently of the
                            // foreground turn.
                            app.finalize_active_cell_as_interrupted();
                            app.finalize_streaming_assistant_as_interrupted();
                            app.status_message = Some("Request cancelled".to_string());
                        }
                        EscapeAction::DiscardQueuedDraft => {
                            app.backtrack.reset();
                            app.queued_draft = None;
                            app.status_message = Some("Stopped editing queued message".to_string());
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
                KeyCode::Down if key.modifiers.is_empty() && mention_menu_open => {
                    app.mention_menu_selected = (app.mention_menu_selected + 1)
                        .min(mention_menu_entries.len().saturating_sub(1));
                }
                KeyCode::Down if key.modifiers.is_empty() && slash_menu_open => {
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
                    let prior_model = app.model.clone();
                    app.cycle_mode();
                    if app.model != prior_model {
                        let _ = engine_handle
                            .send(Op::SetModel {
                                model: app.model.clone(),
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
                // permitted so capital-letter forms (e.g. `Alt+Shift+G` for
                // bottom) work; Ctrl/Super are blocked so the bindings don't
                // collide with platform clipboard / window shortcuts.
                KeyCode::Char('g')
                    if alt_nav_modifiers(key.modifiers)
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
                    if alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open =>
                {
                    app.scroll_to_bottom();
                }
                KeyCode::Char('[')
                    if alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open
                        && !jump_to_adjacent_tool_cell(app, SearchDirection::Backward) =>
                {
                    app.status_message = Some("No previous tool output".to_string());
                }
                KeyCode::Char(']')
                    if alt_nav_modifiers(key.modifiers)
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
                    if alt_nav_modifiers(key.modifiers)
                        && app.input.is_empty()
                        && !slash_menu_open =>
                {
                    if app.view_stack.top_kind() != Some(ModalKind::Help) {
                        app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
                    }
                    continue;
                }
                // Input handling
                _ if is_composer_newline_key(key) => {
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
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(input) = app.submit_input() {
                        if input.starts_with('/') {
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
                                        "Steer failed ({err}); queued {} message(s)",
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
                    if slash_menu_open
                        && !slash_menu_entries.is_empty()
                        && app.input.starts_with('/')
                        && apply_slash_menu_selection(app, &slash_menu_entries, false)
                    {
                        app.close_slash_menu();
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
                        if input.starts_with('/') {
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
                    if is_ctrl_h_backspace(&key) && !app.remove_selected_composer_attachment() =>
                {
                    app.delete_char();
                }
                KeyCode::Char('h') if is_ctrl_h_backspace(&key) => {}
                KeyCode::Delete if !app.remove_selected_composer_attachment() => {
                    app.delete_char_forward();
                }
                KeyCode::Delete => {}
                KeyCode::Left if is_word_cursor_modifier(key.modifiers) => {
                    app.move_cursor_word_backward();
                }
                KeyCode::Left => {
                    app.move_cursor_left();
                }
                KeyCode::Right if is_word_cursor_modifier(key.modifiers) => {
                    app.move_cursor_word_forward();
                }
                KeyCode::Right => {
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
                    app.move_cursor_start();
                }
                KeyCode::Home => {
                    app.move_cursor_start();
                }
                KeyCode::End => {
                    app.move_cursor_end();
                }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.move_cursor_end();
                }
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
                KeyCode::Char('w') | KeyCode::Char('W')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.delete_word_backward();
                }
                KeyCode::Char('s') | KeyCode::Char('S')
                    if key.modifiers == KeyModifiers::CONTROL && !app.input.is_empty() =>
                {
                    // #440: park the current draft to the persistent
                    // stash and clear the composer. Empty composers
                    // are a no-op so a stray Ctrl+S can't pollute the
                    // file. Surface a toast so the user sees the
                    // confirmation (no-op feels broken otherwise).
                    crate::composer_stash::push_stash(&app.input);
                    app.clear_input_recoverable();
                    app.push_status_toast(
                        "Draft stashed — `/stash pop` to restore",
                        StatusToastLevel::Info,
                        Some(3_000),
                    );
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
                    let new_mode = match app.mode {
                        AppMode::Plan => AppMode::Agent,
                        _ => AppMode::Plan,
                    };
                    app.set_mode(new_mode);
                }
                _ if is_paste_shortcut(&key) => {
                    app.paste_from_clipboard();
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_mode(AppMode::Agent);
                    continue;
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_mode(AppMode::Yolo);
                    continue;
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_mode(AppMode::Plan);
                    continue;
                }
                KeyCode::Char('A') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_mode(AppMode::Agent);
                    continue;
                }
                KeyCode::Char('Y') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_mode(AppMode::Yolo);
                    continue;
                }
                KeyCode::Char('P') if key.modifiers.contains(KeyModifiers::ALT) => {
                    app.set_mode(AppMode::Plan);
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
                    handle_vim_normal_key(app, c);
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
                KeyCode::Char(c) => {
                    app.insert_char(c);
                }
                _ => {}
            }

            if !is_plain_char && !is_enter {
                app.paste_burst.clear_window_after_non_char();
            }
        }
    }
}

/// Handle a plain character key press when the composer is in vim Normal mode.
///
/// Implements the core set of normal-mode bindings:
/// - `h` / `l`  — left / right by character
/// - `j` / `k`  — down / up by logical line (falls back to prev/next history)
/// - `w` / `b`  — word forward / backward
/// - `0` / `$`  — line start / end
/// - `x`        — delete character under cursor
/// - `d` (×2)   — delete current line (`dd`)
/// - `i`        — enter Insert before cursor
/// - `a`        — enter Insert after cursor
/// - `o`        — open new line below and enter Insert
/// - `v`        — enter Visual mode
/// - `G`        — move to end of buffer
fn handle_vim_normal_key(app: &mut App, c: char) {
    use crate::tui::app::VimMode;

    // Handle pending `d` (waiting for second `d` to complete `dd`).
    if app.composer.vim_pending_d {
        app.composer.vim_pending_d = false;
        if c == 'd' {
            app.vim_delete_line();
        }
        // Any other key cancels the pending operator.
        return;
    }

    match c {
        'h' => {
            app.move_cursor_left();
        }
        'l' => {
            app.move_cursor_right();
        }
        'j' => {
            app.vim_move_down();
        }
        'k' => {
            app.vim_move_up();
        }
        'w' => {
            app.vim_move_word_forward();
        }
        'b' => {
            app.vim_move_word_backward();
        }
        '0' => {
            app.vim_move_line_start();
        }
        '$' => {
            app.vim_move_line_end();
        }
        'x' => {
            app.vim_delete_char_under_cursor();
        }
        'd' => {
            // Start the `dd` operator sequence.
            app.composer.vim_pending_d = true;
        }
        'i' => {
            app.vim_enter_insert();
        }
        'a' => {
            app.vim_enter_append();
        }
        'o' => {
            app.vim_open_line_below();
        }
        'v' => {
            app.composer.vim_mode = VimMode::Visual;
            app.needs_redraw = true;
        }
        'G' => {
            app.move_cursor_end();
        }
        _ => {
            // Unknown normal-mode key — silently ignored in Normal mode.
        }
    }
}

fn apply_alt_4_shortcut(app: &mut App, _modifiers: KeyModifiers) {
    app.set_sidebar_focus(SidebarFocus::Agents);
    app.status_message = Some("Sidebar focus: agents".to_string());
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

async fn run_cache_warmup(app: &App, config: &Config) -> Result<Usage> {
    let client = DeepSeekClient::new(config)?;
    let reasoning_effort = if app.reasoning_effort == ReasoningEffort::Auto {
        app.last_effective_reasoning_effort
            .and_then(ReasoningEffort::api_value)
            .map(str::to_string)
    } else {
        app.reasoning_effort.api_value().map(str::to_string)
    };
    let request = MessageRequest {
        model: app.model.clone(),
        messages: app.api_messages.clone(),
        max_tokens: 1024,
        system: app.system_prompt.clone(),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort,
        stream: None,
        temperature: None,
        top_p: None,
    };
    let warmup = build_cache_warmup_request(&request);
    let response =
        tokio::time::timeout(Duration::from_secs(45), client.create_message(warmup)).await??;
    Ok(response.usage)
}

fn format_cache_warmup_result(usage: &Usage) -> String {
    let cache = match (
        usage.prompt_cache_hit_tokens,
        usage.prompt_cache_miss_tokens,
    ) {
        (Some(hit), Some(miss)) => format!("Cache warmup complete: hit {hit} | miss {miss}"),
        (Some(hit), None) => format!("Cache warmup complete: hit {hit} | miss unavailable"),
        (None, Some(miss)) => format!("Cache warmup complete: hit unavailable | miss {miss}"),
        (None, None) => "Cache warmup complete: cache telemetry unavailable".to_string(),
    };
    format!(
        "{cache}\nNote: the first warmup is usually a miss. Later requests that reuse the same stable prefix may hit the provider cache; a hit is not guaranteed."
    )
}

fn format_available_models_message(current_model: &str, models: &[String]) -> String {
    let mut lines = vec![format!("Available models ({})", models.len())];
    for model in models {
        if model == current_model {
            lines.push(format!("* {model} (current)"));
        } else {
            lines.push(format!("  {model}"));
        }
    }
    lines.join("\n")
}

fn build_session_snapshot(app: &App, manager: &SessionManager) -> SavedSession {
    if let Some(ref existing_id) = app.current_session_id
        && let Ok(existing) = manager.load_session(existing_id)
    {
        let mut updated = update_session(
            existing,
            &app.api_messages,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
        );
        updated.metadata.mode = Some(app.mode.as_setting().to_string());
        updated.context_references = app.session_context_references.clone();
        updated.artifacts = app.session_artifacts.clone();
        updated
    } else {
        let mut session = if let Some(existing_id) = app.current_session_id.as_ref() {
            create_saved_session_with_id_and_mode(
                existing_id.clone(),
                &app.api_messages,
                &app.model,
                &app.workspace,
                u64::from(app.session.total_tokens),
                app.system_prompt.as_ref(),
                Some(app.mode.as_setting()),
            )
        } else {
            create_saved_session_with_mode(
                &app.api_messages,
                &app.model,
                &app.workspace,
                u64::from(app.session.total_tokens),
                app.system_prompt.as_ref(),
                Some(app.mode.as_setting()),
            )
        };
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
        && app.dispatch_started_at.is_some_and(|started| {
            now.saturating_duration_since(started) > DISPATCH_WATCHDOG_TIMEOUT
        })
    {
        app.is_loading = false;
        app.dispatch_started_at = None;
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
    {
        app.is_loading = false;
        app.dispatch_started_at = None;
        app.push_status_toast(
            "Recovered from an inconsistent busy state.",
            StatusToastLevel::Warning,
            None,
        );
        return true;
    }

    false
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
    finalize_current_streaming_thinking(app);
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
            "The API key from DEEPSEEK_API_KEY was rejected. Paste a valid key to save it to ~/.deepseek/config.toml, or update the environment variable.".to_string(),
        );
        return;
    }
    if !recoverable {
        app.offline_mode = true;
    }
    // Error is already in the transcript as HistoryCell::Error above;
    // don't emit a redundant status_message that would become a sticky
    // toast in the footer — that duplicates the transcript entry.
}

fn persist_offline_queue_state(app: &App) {
    if let Ok(manager) = SessionManager::default_location() {
        if app.queued_messages.is_empty() && app.queued_draft.is_none() {
            let _ = manager.clear_offline_queue_state();
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
        let _ = manager.save_offline_queue_state(&state, app.current_session_id.as_deref());
    }
}

fn sanitize_stream_chunk(chunk: &str) -> String {
    // Keep printable characters and common whitespace; drop control bytes.
    chunk
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// Resolve the effective notification method/threshold/include-summary tuple
/// for a completed turn, taking the high-level
/// `[tui].notification_condition` override into account on top of the
/// lower-level `[notifications]` block.
///
/// Returns `None` to mean "do not notify" (either because the user set
/// `notification_condition = "never"` or because the resolved method is
/// `Off`).
fn notification_settings(
    config: &Config,
) -> Option<(crate::tui::notifications::Method, Duration, bool)> {
    let notif = config.notifications_config();
    let method = match notif.method {
        crate::config::NotificationMethod::Auto => crate::tui::notifications::Method::Auto,
        crate::config::NotificationMethod::Osc9 => crate::tui::notifications::Method::Osc9,
        crate::config::NotificationMethod::Bel => crate::tui::notifications::Method::Bel,
        crate::config::NotificationMethod::Off => crate::tui::notifications::Method::Off,
    };

    if let Some(condition) = config
        .tui
        .as_ref()
        .and_then(|tui| tui.notification_condition)
    {
        match condition {
            crate::config::NotificationCondition::Always => {
                return Some((method, Duration::ZERO, notif.include_summary));
            }
            crate::config::NotificationCondition::Never => return None,
        }
    }

    Some((
        method,
        Duration::from_secs(notif.threshold_secs),
        notif.include_summary,
    ))
}

/// Build the notification body for a completed turn. Prefers the live
/// streaming text the user just saw; falls back to the latest assistant
/// message in `api_messages` if streaming text is empty (for example, the
/// turn finished entirely through tool output). When `include_summary` is
/// true, an elapsed/cost line is appended.
fn completed_turn_notification_message(
    app: &App,
    current_streaming_text: &str,
    include_summary: bool,
    turn_elapsed: Duration,
    turn_cost: Option<crate::pricing::CostEstimate>,
) -> String {
    let mut msg = notification_text_summary(current_streaming_text)
        .or_else(|| latest_assistant_notification_text(&app.api_messages))
        .unwrap_or_else(|| "deepseek: turn complete".to_string());

    if include_summary {
        let human = crate::tui::notifications::humanize_duration(turn_elapsed);
        let summary = match turn_cost {
            Some(c) => {
                let cost = crate::pricing::format_cost_estimate(c, app.cost_currency);
                format!("deepseek: turn complete ({human}, {cost})")
            }
            None => format!("deepseek: turn complete ({human})"),
        };
        if msg == "deepseek: turn complete" {
            msg = summary;
        } else {
            msg.push('\n');
            msg.push_str(&summary);
        }
    }

    msg
}

fn subagent_completion_notification_message(
    id: &str,
    result: &str,
    include_summary: bool,
    elapsed: Duration,
) -> String {
    let result_line = result
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("<deepseek:subagent.done>"));
    let mut msg = result_line
        .and_then(notification_text_summary)
        .map(|summary| format!("sub-agent {id}: {summary}"))
        .unwrap_or_else(|| format!("deepseek: sub-agent {id} complete"));

    if include_summary {
        let human = crate::tui::notifications::humanize_duration(elapsed);
        msg.push('\n');
        msg.push_str(&format!("deepseek: sub-agent complete ({human})"));
    }

    msg
}

fn latest_assistant_notification_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .and_then(|message| {
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    ContentBlock::Thinking { .. }
                    | ContentBlock::ToolUse { .. }
                    | ContentBlock::ToolResult { .. }
                    | ContentBlock::ServerToolUse { .. }
                    | ContentBlock::ToolSearchToolResult { .. }
                    | ContentBlock::CodeExecutionToolResult { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            notification_text_summary(&text)
        })
}

fn notification_text_summary(text: &str) -> Option<String> {
    const MAX_CHARS: usize = 360;

    let sanitized = sanitize_stream_chunk(text);
    let collapsed = sanitized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((idx, _)) = trimmed.char_indices().nth(MAX_CHARS) {
        let mut s = String::with_capacity(idx + 3);
        s.push_str(&trimmed[..idx]);
        s.push_str("...");
        Some(s)
    } else {
        Some(trimmed.to_string())
    }
}

/// Ensure an in-flight streaming Assistant cell exists in history and return
/// its index. Thinking cells go through `ensure_streaming_thinking_active_entry`
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

/// Ensure an in-flight Thinking entry exists in `active_cell` and return its
/// entry index. If no thinking entry is currently streaming, push a fresh one.
/// P2.3: thinking shares the active cell with subsequent tool calls so the
/// pair render as one logical "Working…" block.
fn ensure_streaming_thinking_active_entry(app: &mut App) -> usize {
    if let Some(idx) = app.streaming_thinking_active_entry {
        return idx;
    }
    if app.active_cell.is_none() {
        app.active_cell = Some(ActiveCell::new());
    }
    let active = app.active_cell.as_mut().expect("active_cell just ensured");
    let entry_idx = active.push_thinking(HistoryCell::Thinking {
        content: String::new(),
        streaming: true,
        duration_secs: None,
    });
    app.streaming_thinking_active_entry = Some(entry_idx);
    app.bump_active_cell_revision();
    entry_idx
}

/// Append text to a streaming Thinking entry inside `active_cell`. Bumps the
/// active-cell revision so the renderer re-draws the live tail.
fn append_streaming_thinking(app: &mut App, entry_idx: usize, text: &str) {
    if text.is_empty() {
        return;
    }
    let mutated = if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking { content, .. }) = active.entry_mut(entry_idx)
    {
        content.push_str(text);
        true
    } else {
        false
    };
    if mutated {
        app.bump_active_cell_revision();
    }
}

/// Start a new streaming thinking block. If another thinking block is still
/// active, first drain its pending UI tail so a late block boundary cannot
/// discard content buffered inside `StreamingState`.
fn start_streaming_thinking_block(app: &mut App) -> bool {
    let finalized_previous = if app.streaming_thinking_active_entry.is_some() {
        let finalized = finalize_current_streaming_thinking(app);
        stash_reasoning_buffer_into_last_reasoning(app);
        finalized
    } else {
        false
    };

    app.reasoning_buffer.clear();
    app.reasoning_header = None;
    app.thinking_started_at = Some(Instant::now());
    app.streaming_state.reset();
    app.streaming_state.start_thinking(0, None);
    let _ = ensure_streaming_thinking_active_entry(app);
    finalized_previous
}

fn finalize_current_streaming_thinking(app: &mut App) -> bool {
    let duration = app
        .thinking_started_at
        .take()
        .map(|t| t.elapsed().as_secs_f32());
    let remaining = app.streaming_state.finalize_block_text(0);
    finalize_streaming_thinking_active_entry(app, duration, &remaining)
}

fn stash_reasoning_buffer_into_last_reasoning(app: &mut App) {
    if app.reasoning_buffer.is_empty() {
        return;
    }

    if let Some(existing) = app.last_reasoning.as_mut()
        && !existing.is_empty()
    {
        if !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&app.reasoning_buffer);
    } else {
        app.last_reasoning = Some(app.reasoning_buffer.clone());
    }
    app.reasoning_buffer.clear();
}

/// Finalize the in-flight thinking entry in `active_cell`: append the
/// collector's remaining buffered text, stop the spinner, and stamp the
/// duration. Returns `true` when a thinking entry was finalized (so the
/// dispatch loop knows the transcript was touched). No-op if no thinking
/// entry is currently streaming.
fn finalize_streaming_thinking_active_entry(
    app: &mut App,
    duration: Option<f32>,
    remaining: &str,
) -> bool {
    let Some(entry_idx) = app.streaming_thinking_active_entry.take() else {
        return false;
    };
    if !remaining.is_empty() {
        append_streaming_thinking(app, entry_idx, remaining);
    }
    if let Some(active) = app.active_cell.as_mut()
        && let Some(HistoryCell::Thinking {
            streaming,
            duration_secs,
            ..
        }) = active.entry_mut(entry_idx)
    {
        *streaming = false;
        *duration_secs = duration;
    }
    app.bump_active_cell_revision();
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EscapeAction {
    CloseSlashMenu,
    CancelRequest,
    DiscardQueuedDraft,
    ClearInput,
    Noop,
}

fn next_escape_action(app: &App, slash_menu_open: bool) -> EscapeAction {
    if slash_menu_open {
        EscapeAction::CloseSlashMenu
    } else if app.is_loading {
        EscapeAction::CancelRequest
    } else if app.queued_draft.is_some() && app.input.is_empty() {
        EscapeAction::DiscardQueuedDraft
    } else if !app.input.is_empty() {
        EscapeAction::ClearInput
    } else {
        EscapeAction::Noop
    }
}

fn select_previous_slash_menu_entry(app: &mut App, entry_count: usize) {
    if entry_count == 0 {
        return;
    }
    let selected = app.slash_menu_selected.min(entry_count.saturating_sub(1));
    app.slash_menu_selected = (selected + entry_count - 1) % entry_count;
}

fn select_next_slash_menu_entry(app: &mut App, entry_count: usize) {
    if entry_count == 0 {
        return;
    }
    let selected = app.slash_menu_selected.min(entry_count.saturating_sub(1));
    app.slash_menu_selected = (selected + 1) % entry_count;
}

fn handle_composer_history_arrow(
    app: &mut App,
    key: KeyEvent,
    slash_menu_open: bool,
    mention_menu_open: bool,
) -> bool {
    if slash_menu_open || mention_menu_open {
        return false;
    }
    if key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SUPER) {
        return false;
    }

    // When `composer_arrows_scroll` is enabled and the composer is empty,
    // plain Up/Down scroll the transcript.  This helps terminals that map
    // trackpad gestures to arrow keys.  Otherwise arrows always navigate
    // input history regardless of composer state (#1117).
    let scroll_on_empty = app.composer_arrows_scroll && app.input.trim().is_empty();

    match key.code {
        KeyCode::Up => {
            if scroll_on_empty {
                app.scroll_up(1);
            } else {
                app.history_up();
            }
            true
        }
        KeyCode::Down => {
            if scroll_on_empty {
                app.scroll_down(1);
            } else {
                app.history_down();
            }
            true
        }
        _ => false,
    }
}

fn is_word_cursor_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT)
}

fn is_composer_newline_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('j') => key.modifiers.contains(KeyModifiers::CONTROL),
        KeyCode::Enter => {
            key.modifiers.contains(KeyModifiers::ALT)
                || (key.modifiers.contains(KeyModifiers::SHIFT)
                    && !key.modifiers.contains(KeyModifiers::CONTROL))
        }
        _ => false,
    }
}

fn handle_history_search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let _ = app.accept_history_search();
        }
        KeyCode::Esc => {
            app.cancel_history_search();
        }
        KeyCode::Char('c') | KeyCode::Char('C')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            app.cancel_history_search();
        }
        KeyCode::Backspace => {
            app.history_search_backspace();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            while app
                .history_search_query()
                .is_some_and(|query| !query.is_empty())
            {
                app.history_search_backspace();
            }
        }
        KeyCode::Up => {
            app.history_search_select_previous();
        }
        KeyCode::Down => {
            app.history_search_select_next();
        }
        KeyCode::Char(ch)
            if key.modifiers.is_empty()
                || key.modifiers == KeyModifiers::SHIFT
                || key.modifiers == KeyModifiers::NONE =>
        {
            app.history_search_insert_char(ch);
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ApiKeyValidation {
    Accept { warning: Option<String> },
    Reject(String),
}

fn validate_api_key_for_onboarding(api_key: &str) -> ApiKeyValidation {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return ApiKeyValidation::Reject("API key cannot be empty.".to_string());
    }
    if trimmed.contains(char::is_whitespace) {
        return ApiKeyValidation::Reject(
            "API key appears malformed (contains whitespace).".to_string(),
        );
    }
    if trimmed.len() < 16 {
        return ApiKeyValidation::Accept {
            warning: Some(
                "API key looks short. Double-check it, but unusual formats are allowed."
                    .to_string(),
            ),
        };
    }
    if !trimmed.contains('-') {
        return ApiKeyValidation::Accept {
            warning: Some(
                "API key format looks unusual. Check that the full key was copied.".to_string(),
            ),
        };
    }
    ApiKeyValidation::Accept { warning: None }
}

fn advance_onboarding_from_welcome(app: &mut App) {
    app.status_message = None;
    app.onboarding = OnboardingState::Language;
}

fn advance_onboarding_after_language(app: &mut App) {
    app.status_message = None;
    if app.onboarding_needs_api_key {
        app.onboarding = OnboardingState::ApiKey;
    } else if !app.trust_mode && onboarding::needs_trust(&app.workspace) {
        app.onboarding = OnboardingState::TrustDirectory;
    } else {
        app.onboarding = OnboardingState::Tips;
    }
}

fn sync_api_key_validation_status(app: &mut App, show_empty_error: bool) {
    if app.api_key_input.trim().is_empty() && !show_empty_error {
        app.status_message = None;
        return;
    }

    match validate_api_key_for_onboarding(&app.api_key_input) {
        ApiKeyValidation::Accept { warning } => {
            app.status_message = warning;
        }
        ApiKeyValidation::Reject(message) => {
            app.status_message = Some(message);
        }
    }
}

fn build_queued_message(app: &mut App, input: String) -> QueuedMessage {
    let skill_instruction = app.active_skill.take();
    QueuedMessage::new(input, skill_instruction)
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
        "{} queued — ↑ to edit, /queue list",
        app.queued_message_count()
    ));
    true
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

async fn dispatch_user_message(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) -> Result<()> {
    // #455 (observer-only): fire `message_submit` hooks before
    // dispatch. Hooks see the user's display text via the
    // `with_message` builder. Read-only — they can log, audit, or
    // notify but cannot mutate the message that goes to the engine.
    // Fast-path skip when no hooks configured.
    if app
        .hooks
        .has_hooks_for_event(crate::hooks::HookEvent::MessageSubmit)
    {
        let context = app.base_hook_context().with_message(&message.display);
        let _ = app.execute_hooks(crate::hooks::HookEvent::MessageSubmit, &context);
    }

    // Set immediately to prevent double-dispatch before TurnStarted event arrives.
    let dispatch_started_at = Instant::now();
    app.is_loading = true;
    app.dispatch_started_at = Some(dispatch_started_at);
    app.runtime_turn_status = None;
    app.last_send_at = Some(dispatch_started_at);

    let cwd = std::env::current_dir().ok();
    let references = crate::tui::file_mention::context_references_from_input(
        &message.display,
        &app.workspace,
        cwd.clone(),
    );
    let content = queued_message_content_for_app(app, &message, cwd);
    let message_index = app.api_messages.len();
    app.system_prompt = Some(
        prompts::system_prompt_for_mode_with_context_skills_and_session(
            app.mode,
            &app.workspace,
            None,
            None,
            None,
            prompts::PromptSessionContext {
                user_memory_block: None,
                goal_objective: app.goal.goal_objective.as_deref(),
                project_context_pack_enabled: config.project_context_pack_enabled(),
                locale_tag: app.ui_locale.tag(),
            },
        ),
    );
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
        app.status_message = Some("Context critical; compacting before send...".to_string());
        let _ = engine_handle.send(Op::CompactContext).await;
    }
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    // Persist immediately so abrupt termination can recover this in-flight turn.
    // Offloaded to the persistence actor.
    if let Ok(manager) = SessionManager::default_location() {
        let session = build_session_snapshot(app, &manager);
        persistence_actor::persist(PersistRequest::Checkpoint(session));
    }

    let auto_selection = if should_resolve_auto_model_selection(app) {
        Some(resolve_auto_model_selection(app, config, &message, &content).await)
    } else {
        None
    };

    let effective_model = if app.auto_model {
        auto_selection
            .as_ref()
            .map(|selection| selection.model.clone())
            .unwrap_or_else(|| commands::auto_model_heuristic(&message.display, &app.model))
    } else {
        app.model.clone()
    };

    let auto_controls_reasoning = app.auto_model || app.reasoning_effort == ReasoningEffort::Auto;
    let effective_reasoning_effort = if auto_controls_reasoning {
        let effort = auto_selection
            .as_ref()
            .and_then(|selection| selection.reasoning_effort)
            .unwrap_or_else(|| {
                normalize_auto_routed_effort(crate::auto_reasoning::select(false, &message.display))
            });
        app.last_effective_reasoning_effort = Some(effort);
        Some(effort.as_setting().to_string())
    } else {
        app.last_effective_reasoning_effort = None;
        app.reasoning_effort.api_value().map(str::to_string)
    };

    if let Some(selection) = auto_selection.as_ref() {
        if app.auto_model {
            app.last_effective_model = Some(effective_model.clone());
            let mut status = format!(
                "Auto model selected: {effective_model} via {}",
                selection.source.label()
            );
            if let Some(effort) = app.last_effective_reasoning_effort {
                status.push_str(&format!("; thinking auto: {}", effort.as_setting()));
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
            model: effective_model,
            goal_objective: app.goal.goal_objective.clone(),
            reasoning_effort: effective_reasoning_effort,
            reasoning_effort_auto: auto_controls_reasoning,
            auto_model: app.auto_model,
            allow_shell: app.allow_shell,
            trust_mode: app.trust_mode,
            auto_approve: app.mode == AppMode::Yolo,
            approval_mode: app.approval_mode,
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

fn should_resolve_auto_model_selection(app: &App) -> bool {
    app.auto_model
}

async fn resolve_auto_model_selection(
    app: &App,
    config: &Config,
    message: &QueuedMessage,
    latest_content: &str,
) -> commands::AutoRouteSelection {
    let latest_request = if latest_content.trim().is_empty() {
        message.display.as_str()
    } else {
        latest_content
    };
    commands::resolve_auto_route_with_flash(
        config,
        latest_request,
        &recent_auto_router_context(&app.api_messages),
        if app.auto_model { "auto" } else { "fixed" },
        app.reasoning_effort.as_setting(),
    )
    .await
}

fn normalize_auto_routed_effort(effort: ReasoningEffort) -> ReasoningEffort {
    commands::normalize_auto_route_effort(effort)
}

fn recent_auto_router_context(messages: &[Message]) -> String {
    let mut rows = Vec::new();
    for message in messages.iter().rev().skip(1) {
        if rows.len() >= 6 {
            break;
        }
        let text = content_blocks_text(&message.content);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        rows.push(format!(
            "{}: {}",
            message.role,
            truncate_for_auto_router(text, 900)
        ));
    }
    rows.reverse();
    if rows.is_empty() {
        "No prior context.".to_string()
    } else {
        rows.join("\n")
    }
}

fn content_blocks_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        match block {
            ContentBlock::Text { text, .. } => {
                append_router_text(&mut out, text);
            }
            ContentBlock::Thinking { thinking } => {
                append_router_text(&mut out, thinking);
            }
            ContentBlock::ToolUse { name, .. } => {
                append_router_text(&mut out, &format!("[tool call: {name}]"));
            }
            ContentBlock::ToolResult { content, .. } => {
                append_router_text(&mut out, &format!("[tool result] {content}"));
            }
            _ => {}
        }
    }
    out
}

fn append_router_text(out: &mut String, text: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(text);
}

fn truncate_for_auto_router(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

async fn apply_model_and_compaction_update(
    engine_handle: &EngineHandle,
    compaction: crate::compaction::CompactionConfig,
) {
    let _ = engine_handle
        .send(Op::SetModel {
            model: compaction.model.clone(),
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
/// `~/.deepseek/settings.toml` so it survives a restart, push the change to
/// the running engine via `Op::SetModel`/`Op::SetCompaction`, and surface
/// a one-line status describing what changed.
async fn apply_model_picker_choice(
    app: &mut App,
    engine_handle: &EngineHandle,
    model: String,
    mut effort: crate::tui::app::ReasoningEffort,
    previous_model: String,
    previous_effort: crate::tui::app::ReasoningEffort,
) {
    let model_is_auto = model.trim().eq_ignore_ascii_case("auto");
    if model_is_auto {
        effort = ReasoningEffort::Auto;
    }
    let model_changed = model != previous_model || app.auto_model != model_is_auto;
    let effort_changed = effort != previous_effort;
    if !model_changed && !effort_changed {
        app.status_message = Some(format!(
            "Model unchanged: {model} · thinking {}",
            effort.short_label()
        ));
        return;
    }

    if model_changed {
        app.auto_model = model_is_auto;
        app.last_effective_model = None;
        app.model = model.clone();
        app.update_model_compaction_budget();
        app.clear_model_scoped_telemetry();
    }
    if effort_changed {
        app.reasoning_effort = effort;
        app.last_effective_reasoning_effort = None;
    }

    // Best-effort persist; surface a status warning if the settings file
    // can't be written rather than aborting the in-memory change.
    let mut persist_warning: Option<String> = None;
    match crate::settings::Settings::load() {
        Ok(mut settings) => {
            if model_changed {
                let _ = settings.set("default_model", &model);
                settings.set_model_for_provider(app.api_provider.as_str(), &model);
            }
            if effort_changed {
                let _ = settings.set("reasoning_effort", effort.as_setting());
            }
            if let Err(err) = settings.save() {
                persist_warning = Some(format!("(not persisted: {err})"));
            }
        }
        Err(err) => {
            persist_warning = Some(format!("(not persisted: {err})"));
        }
    }

    if model_changed {
        apply_model_and_compaction_update(engine_handle, app.compaction_config()).await;
    }

    let model_summary = if model_is_auto {
        "auto (per-turn model)".to_string()
    } else {
        model.clone()
    };
    let previous_effort_summary = previous_effort.short_label();
    let effort_summary = if effort == ReasoningEffort::Auto {
        "auto (per-turn thinking)".to_string()
    } else {
        effort.short_label().to_string()
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

/// Apply a `/provider` switch by mutating the in-memory config, validating
/// that credentials exist for the new provider, then respawning the engine
/// so the API client picks up the new base URL/key. When `model_override`
/// is set, it replaces the active model post-switch (already normalized,
/// will be provider-prefixed by `Config::default_model`).
async fn switch_provider(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    target: ApiProvider,
    model_override: Option<String>,
) {
    let previous_provider = app.api_provider;
    let previous_model = app.model.clone();
    let previous_provider_str = config.provider.clone();
    let previous_base_url = config.base_url.clone();
    let previous_default_text_model = config.default_text_model.clone();

    config.provider = Some(target.as_str().to_string());
    if matches!(target, ApiProvider::NvidiaNim)
        && config
            .base_url
            .as_deref()
            .map(|base| !base.contains("integrate.api.nvidia.com"))
            .unwrap_or(true)
    {
        config.base_url = Some(DEFAULT_NVIDIA_NIM_BASE_URL.to_string());
    }
    if matches!(target, ApiProvider::Deepseek)
        && config
            .base_url
            .as_deref()
            .map(|base| base.contains("integrate.api.nvidia.com"))
            .unwrap_or(false)
    {
        config.base_url = None;
    }
    if let Some(ref model) = model_override {
        config.default_text_model = Some(model.clone());
    }

    if let Err(err) = DeepSeekClient::new(config) {
        config.provider = previous_provider_str;
        config.base_url = previous_base_url;
        config.default_text_model = previous_default_text_model;
        app.add_message(HistoryCell::System {
            content: format!(
                "Failed to switch provider to {}: {err}\nProvider unchanged ({}).",
                target.as_str(),
                previous_provider.as_str()
            ),
        });
        return;
    }

    let new_model = config.default_model();
    let cache_scope_changed = previous_provider != target || previous_model != new_model;
    app.api_provider = target;
    app.model = new_model.clone();
    app.update_model_compaction_budget();
    if cache_scope_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
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
            "Provider switched: {} → {}\nModel: {} → {}",
            previous_provider.as_str(),
            target.as_str(),
            previous_model,
            new_model
        ),
    });
    app.status_message = Some(format!("Provider: {}", target.as_str()));

    // Persist the provider choice so it survives restarts.
    if let Ok(mut settings) = crate::settings::Settings::load() {
        settings.default_provider = Some(target.as_str().to_string());
        let _ = settings.save();
    }
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

fn open_context_inspector(app: &mut App) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let content = build_context_inspector_text(app);
    app.view_stack.push(PagerView::from_text(
        "Context inspector",
        &content,
        width.saturating_sub(2),
    ));
}

fn open_file_picker(app: &mut App) {
    let relevance = build_file_picker_relevance(app);
    app.view_stack
        .push(crate::tui::file_picker::FilePickerView::new_with_relevance(
            &app.workspace,
            relevance,
        ));
}

fn build_file_picker_relevance(app: &App) -> crate::tui::file_picker::FilePickerRelevance {
    let mut relevance = crate::tui::file_picker::FilePickerRelevance::default();

    for path in modified_workspace_paths(&app.workspace) {
        relevance.mark_modified(path);
    }

    for record in app.session_context_references.iter().rev().take(64) {
        let reference = &record.reference;
        if reference.source != crate::tui::file_mention::ContextReferenceSource::AtMention {
            continue;
        }
        if !matches!(
            reference.kind,
            crate::tui::file_mention::ContextReferenceKind::File
        ) {
            continue;
        }
        for raw in [&reference.target, &reference.label] {
            if let Some(path) = workspace_file_candidate(raw, &app.workspace) {
                relevance.mark_mentioned(path);
            }
        }
    }

    let mut seen_tool_paths = HashSet::new();
    for detail in app.active_tool_details.values() {
        mark_tool_detail_paths(detail, &app.workspace, &mut seen_tool_paths, &mut relevance);
    }
    let mut rows: Vec<_> = app.tool_details_by_cell.iter().collect();
    rows.sort_by_key(|(idx, _)| std::cmp::Reverse(**idx));
    for (_, detail) in rows.into_iter().take(48) {
        mark_tool_detail_paths(detail, &app.workspace, &mut seen_tool_paths, &mut relevance);
    }

    relevance
}

fn modified_workspace_paths(workspace: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["status", "--short", "--untracked-files=normal"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_git_status_path)
        .filter_map(|path| workspace_file_candidate(&path, workspace))
        .collect()
}

fn parse_git_status_path(line: &str) -> Option<String> {
    if line.len() < 4 {
        return None;
    }
    let raw = line.get(3..)?.trim();
    let raw = raw.rsplit(" -> ").next().unwrap_or(raw).trim();
    let raw = raw.trim_matches('"');
    if raw.is_empty() {
        None
    } else {
        Some(raw.to_string())
    }
}

fn mark_tool_detail_paths(
    detail: &ToolDetailRecord,
    workspace: &Path,
    seen: &mut HashSet<String>,
    relevance: &mut crate::tui::file_picker::FilePickerRelevance,
) {
    let mut budget = 256usize;
    mark_tool_paths_from_value(&detail.input, workspace, seen, relevance, &mut budget);
    if let Some(output) = detail
        .output
        .as_deref()
        .filter(|output| output.len() <= 8_192)
    {
        mark_tool_paths_from_text(output, workspace, seen, relevance, &mut budget);
    }
}

fn mark_tool_paths_from_value(
    value: &serde_json::Value,
    workspace: &Path,
    seen: &mut HashSet<String>,
    relevance: &mut crate::tui::file_picker::FilePickerRelevance,
    budget: &mut usize,
) {
    if *budget == 0 {
        return;
    }
    match value {
        serde_json::Value::String(text) => {
            mark_tool_paths_from_text(text, workspace, seen, relevance, budget);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                mark_tool_paths_from_value(item, workspace, seen, relevance, budget);
                if *budget == 0 {
                    break;
                }
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                mark_tool_paths_from_value(item, workspace, seen, relevance, budget);
                if *budget == 0 {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn mark_tool_paths_from_text(
    text: &str,
    workspace: &Path,
    seen: &mut HashSet<String>,
    relevance: &mut crate::tui::file_picker::FilePickerRelevance,
    budget: &mut usize,
) {
    if *budget == 0 || text.len() > 8_192 {
        return;
    }
    if let Some(path) = workspace_file_candidate(text, workspace)
        && seen.insert(path.clone())
    {
        relevance.mark_tool(path);
        *budget = (*budget).saturating_sub(1);
    }
    for token in text.split_whitespace().take(128) {
        if *budget == 0 {
            break;
        }
        if let Some(path) = workspace_file_candidate(token, workspace)
            && seen.insert(path.clone())
        {
            relevance.mark_tool(path);
            *budget = (*budget).saturating_sub(1);
        }
    }
}

fn workspace_file_candidate(raw: &str, workspace: &Path) -> Option<String> {
    let cleaned = clean_path_token(raw)?;
    let path = Path::new(&cleaned);
    let absolute = if path.is_absolute() {
        PathBuf::from(path)
    } else {
        workspace.join(path)
    };
    if !absolute.is_file() {
        return None;
    }
    let rel = absolute.strip_prefix(workspace).ok()?;
    workspace_path_to_picker_string(rel)
}

fn clean_path_token(raw: &str) -> Option<String> {
    let mut trimmed = raw.trim().trim_matches(|ch: char| {
        ch.is_ascii_whitespace()
            || matches!(
                ch,
                '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
            )
    });
    if let Some(stripped) = trimmed.strip_prefix("./") {
        trimmed = stripped;
    }
    if let Some((before, after)) = trimmed.rsplit_once(':')
        && !before.is_empty()
        && after.chars().all(|ch| ch.is_ascii_digit())
    {
        trimmed = before;
    }
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn workspace_path_to_picker_string(path: &Path) -> Option<String> {
    let mut out = String::new();
    for (idx, component) in path.components().enumerate() {
        if matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        ) {
            return None;
        }
        if idx > 0 {
            out.push('/');
        }
        out.push_str(&component.as_os_str().to_string_lossy());
    }
    if out.is_empty() { None } else { Some(out) }
}

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
            AppAction::SendMessage(content) => {
                let queued = build_queued_message(app, content);
                submit_or_steer_message(app, config, engine_handle, queued).await?;
            }
            AppAction::Rlm {
                prompt,
                model,
                child_model,
                max_depth,
            } => {
                app.status_message = Some("RLM turn starting...".to_string());
                let _ = engine_handle
                    .send(Op::Rlm {
                        content: prompt,
                        model,
                        child_model,
                        max_depth,
                    })
                    .await;
            }
            AppAction::ListSubAgents => {
                let _ = engine_handle.send(Op::ListSubAgents).await;
            }
            AppAction::FetchModels => {
                if crate::config::provider_passes_model_through(config.api_provider()) {
                    app.add_message(HistoryCell::System {
                        content: format!(
                            "/models is not supported by the {} provider.",
                            config.api_provider().display_name()
                        ),
                    });
                } else {
                    app.status_message = Some("Fetching models...".to_string());
                    match fetch_available_models(config).await {
                        Ok(models) => {
                            app.add_message(HistoryCell::System {
                                content: format_available_models_message(&app.model, &models),
                            });
                            app.status_message = Some(format!("Found {} model(s)", models.len()));
                        }
                        Err(error) => {
                            app.add_message(HistoryCell::System {
                                content: format!("Failed to fetch models: {error}"),
                            });
                        }
                    }
                }
            }
            AppAction::CacheWarmup => {
                app.status_message = Some("Warming DeepSeek cache...".to_string());
                match run_cache_warmup(app, config).await {
                    Ok(usage) => {
                        let message = format_cache_warmup_result(&usage);
                        app.add_message(HistoryCell::System {
                            content: message.clone(),
                        });
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
            }
            AppAction::UpdateCompaction(compaction) => {
                apply_model_and_compaction_update(engine_handle, compaction).await;
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
                    )?;
                    match editor_result {
                        Ok(outcome) => {
                            if outcome.requires_engine_sync {
                                apply_model_and_compaction_update(
                                    engine_handle,
                                    app.compaction_config(),
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
                        ));
                }
            }
            AppAction::OpenFeedbackPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::FeedbackPicker) {
                    app.view_stack
                        .push(crate::tui::feedback_picker::FeedbackPickerView::new());
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
            }
            AppAction::Mcp(action) => {
                handle_mcp_ui_action(app, config, action).await;
            }
            AppAction::SwitchProfile { profile } => {
                app.config_profile = Some(profile.clone());
                match Config::load(app.config_path.clone(), Some(&profile)) {
                    Ok(new_config) => {
                        *config = new_config.clone();
                        app.api_provider = config.api_provider();
                        let new_model = config.default_model();
                        app.model = new_model.clone();
                        app.update_model_compaction_budget();
                        app.session.last_prompt_tokens = None;
                        app.session.last_completion_tokens = None;
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

fn open_external_url(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    return Err(anyhow::anyhow!(
        "browser opening is unsupported on this platform"
    ));

    let status = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|err| anyhow::anyhow!("failed to launch browser command: {err}"))?;
    if !status.success() {
        return Err(anyhow::anyhow!(
            "browser command exited with status {status}"
        ));
    }
    Ok(())
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
    let discover = matches!(
        action,
        crate::tui::app::McpUiAction::Validate | crate::tui::app::McpUiAction::Reload
    );

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
            mcp::add_server_config(&path, name.clone(), Some(command), None, args)
                .map(|()| message = Some(format!("Added MCP stdio server '{name}'")))
        }
        crate::tui::app::McpUiAction::AddHttp { name, url } => {
            changed = true;
            mcp::add_server_config(&path, name.clone(), None, Some(url), Vec::new())
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
        mcp::discover_manager_snapshot(&path, network_policy, app.mcp_restart_required).await
    } else {
        mcp::manager_snapshot_from_config(&path, app.mcp_restart_required)
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

fn handle_shell_job_action(app: &mut App, action: crate::tui::app::ShellJobAction) {
    let Some(shell_manager) = app.runtime_services.shell_manager.clone() else {
        add_shell_job_message(app, "Shell job center is not attached.".to_string());
        return;
    };

    let mut manager = match shell_manager.lock() {
        Ok(manager) => manager,
        Err(_) => {
            add_shell_job_message(app, "Shell job center lock is poisoned.".to_string());
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
            Err(err) => add_shell_job_message(app, format!("Shell job lookup failed: {err}")),
        },
        crate::tui::app::ShellJobAction::Poll { id, wait } => {
            match manager.poll_delta(&id, wait, if wait { 5_000 } else { 1_000 }) {
                Ok(delta) => add_shell_job_message(app, format_shell_poll(&delta.result)),
                Err(err) => add_shell_job_message(app, format!("Shell job poll failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::SendStdin { id, input, close } => {
            match manager.write_stdin(&id, &input, close) {
                Ok(()) => match manager.poll_delta(&id, false, 1_000) {
                    Ok(delta) => add_shell_job_message(app, format_shell_poll(&delta.result)),
                    Err(err) => {
                        add_shell_job_message(app, format!("Shell stdin sent; poll failed: {err}"));
                    }
                },
                Err(err) => add_shell_job_message(app, format!("Shell stdin failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::Cancel { id } => match manager.kill(&id) {
            Ok(result) => add_shell_job_message(app, format_shell_poll(&result)),
            Err(err) => add_shell_job_message(app, format!("Shell job cancel failed: {err}")),
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
    let result = commands::execute(input, app);
    // After /logout: clear the in-memory api_key fields so the next
    // onboarding round entering a new key doesn't see the stale value
    // (#343). The on-disk side is handled by clear_api_key() inside
    // commands::config::logout.
    if input.trim().eq_ignore_ascii_case("/logout") {
        config.api_key = None;
        if let Some(providers) = config.providers.as_mut() {
            providers.deepseek.api_key = None;
            providers.deepseek_cn.api_key = None;
            providers.nvidia_nim.api_key = None;
            providers.openrouter.api_key = None;
            providers.novita.api_key = None;
            providers.fireworks.api_key = None;
            providers.sglang.api_key = None;
            providers.vllm.api_key = None;
            providers.ollama.api_key = None;
        }
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

async fn steer_user_message(
    app: &mut App,
    engine_handle: &EngineHandle,
    message: QueuedMessage,
) -> Result<()> {
    let cwd = std::env::current_dir().ok();
    let references = crate::tui::file_mention::context_references_from_input(
        &message.display,
        &app.workspace,
        cwd.clone(),
    );
    let content = queued_message_content_for_app(app, &message, cwd);
    let message_index = app.api_messages.len();

    engine_handle.steer(content.clone()).await?;

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
                app.status_message =
                    Some(format!("Offline: {count} queued — ↑ to edit, /queue list"));
            } else {
                app.status_message = Some(format!("{count} queued — ↑ to edit, /queue list"));
            }
            Ok(())
        }
        // Steer and QueueFollowUp are now only reached via Ctrl+Enter override.
        SubmitDisposition::Steer => {
            if let Err(err) = steer_user_message(app, engine_handle, message.clone()).await {
                app.queue_message(message);
                app.status_message = Some(format!(
                    "Steer failed ({err}); {} queued — ↑ to edit, /queue list",
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
            app.set_mode(AppMode::Agent);
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
            app.set_mode(AppMode::Yolo);
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
            app.set_mode(AppMode::Agent);
            app.add_message(HistoryCell::System {
                content: "Exited Plan mode. Switched to Agent mode.".to_string(),
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
///   no engine path produces these yet but the bucket renders identically).
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
    let body_height = size.height.saturating_sub(header_height + footer_height);
    let slash_menu_entries = visible_slash_menu_entries(app, SLASH_MENU_LIMIT);
    let mention_menu_entries =
        crate::tui::file_mention::visible_mention_menu_entries(app, MENTION_MENU_LIMIT);
    if !mention_menu_entries.is_empty() && app.mention_menu_selected >= mention_menu_entries.len() {
        app.mention_menu_selected = mention_menu_entries.len().saturating_sub(1);
    }
    let context_usage = context_usage_snapshot(app);
    let composer_max_height = body_height
        .saturating_sub(MIN_CHAT_HEIGHT)
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

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),   // Header
            Constraint::Min(1),                  // Chat area
            Constraint::Length(preview_height),  // Pending input preview (0 if empty)
            Constraint::Length(composer_height), // Composer
            Constraint::Length(footer_height),   // Footer
        ])
        .split(size);

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
            crate::config::ApiProvider::NvidiaNim => Some("NIM"),
            crate::config::ApiProvider::Openai => Some("OpenAI"),
            crate::config::ApiProvider::Openrouter => Some("OR"),
            crate::config::ApiProvider::Novita => Some("Novita"),
            crate::config::ApiProvider::Fireworks => Some("Fireworks"),
            crate::config::ApiProvider::Sglang => Some("SGLang"),
            crate::config::ApiProvider::Vllm => Some("vLLM"),
            crate::config::ApiProvider::Ollama => Some("Ollama"),
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
            app.turn_started_at,
            &app.status_indicator,
        ));
        let header_widget = HeaderWidget::new(header_data);
        let buf = f.buffer_mut();
        header_widget.render(chunks[0], buf);
    }

    // Render chat + sidebar + optional file-tree pane
    {
        // Defensive backstop (#400): fill the entire body area with ink
        // background before any sub-widgets render, so cells that end up
        // uncovered by layout splits (e.g. after file-tree toggle or
        // resize) don't retain stale content from a previous frame.
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(chunks[1], f.buffer_mut());

        let mut sidebar_area = None;

        // When the file-tree pane is visible and the terminal is wide
        // enough, reserve the left ~25% for the file tree.
        let mut chat_area =
            if app.file_tree.is_some() && chunks[1].width >= SIDEBAR_VISIBLE_MIN_WIDTH {
                let split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
                    .split(chunks[1]);
                let tree_area = split[0];
                let remaining = split[1];

                // Render the file-tree pane.
                if let Some(ref mut state) = app.file_tree {
                    super::file_tree::render_file_tree(f, tree_area, state, app.ui_theme.mode);
                }

                remaining
            } else {
                chunks[1]
            };

        if chat_area.width >= SIDEBAR_VISIBLE_MIN_WIDTH {
            let preferred_sidebar = (u32::from(chat_area.width)
                * u32::from(app.sidebar_width_percent.clamp(10, 50))
                / 100) as u16;
            let sidebar_width = preferred_sidebar
                .max(24)
                .min(chat_area.width.saturating_sub(40));
            if sidebar_width >= 20 {
                let split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(1), Constraint::Length(sidebar_width)])
                    .split(chat_area);
                chat_area = split[0];
                sidebar_area = Some(split[1]);
            }
        }

        let chat_widget = ChatWidget::new(app, chat_area);
        let buf = f.buffer_mut();
        chat_widget.render(chat_area, buf);

        if let Some(sidebar_area) = sidebar_area {
            super::sidebar::render_sidebar(f, sidebar_area, app);
        }
    }

    // Render pending-input preview (queued/steered messages, if any).
    if preview_height > 0 {
        let buf = f.buffer_mut();
        pending_preview.render(chunks[2], buf);
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
        composer_widget.render(chunks[3], buf);
        composer_widget.cursor_pos(chunks[3])
    };
    if let Some(cursor_pos) = cursor_pos {
        f.set_cursor_position(cursor_pos);
    }

    // Render footer
    render_footer(f, chunks[4], app);
    // Toast stack overlay (#439): when multiple status toasts are queued,
    // surface the older ones as a 1-2 line strip above the footer so a
    // burst of events isn't collapsed to a single visible message.
    render_toast_stack_overlay(f, size, chunks[4], app);

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
    let _ = terminal.backend_mut().write_all(BEGIN_SYNC_UPDATE);

    // Run fallible draw operations in a closure so END_SYNC_UPDATE is
    // always sent even if an intermediate step fails. Without this, a
    // failing `?` would return early and leave the terminal stuck in
    // synchronized-update mode (screen frozen).
    let result = (|| -> Result<()> {
        if full_repaint {
            terminal.backend_mut().write_all(TERMINAL_ORIGIN_RESET)?;
            terminal.backend_mut().flush()?;
            terminal.clear()?;
        }
        terminal.draw(|f| render(f, app))?;
        Ok(())
    })();

    // Always end the synchronized update, regardless of success or failure.
    let _ = terminal.backend_mut().write_all(END_SYNC_UPDATE);
    let _ = terminal.backend_mut().flush();
    result
}

fn draw_app_frame(terminal: &mut AppTerminal, app: &mut App) -> Result<()> {
    draw_app_frame_inner(terminal, app, false)
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
            } => {
                if decision == ReviewDecision::ApprovedForSession {
                    // Store both the tool name (backward compat) and the
                    // approval key (fingerprint-based).
                    app.approval_session_approved.insert(tool_name.clone());
                    app.approval_session_approved.insert(approval_key.clone());
                }

                match decision {
                    ReviewDecision::Approved | ReviewDecision::ApprovedForSession => {
                        let _ = engine_handle.approve_tool_call(tool_id).await;
                    }
                    ReviewDecision::Denied | ReviewDecision::Abort => {
                        // Cache the denial so the model retry-loop doesn't
                        // re-prompt for the exact same approval_key (#360).
                        // Only the key (per-call unique) is stored — NOT
                        // the tool_name, which would block all future
                        // invocations of the same tool type (#1377).
                        if !timed_out {
                            app.approval_session_denied.insert(approval_key);
                        }
                        let _ = engine_handle.deny_tool_call(tool_id).await;
                    }
                }

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
                        let recovered = apply_loaded_session(app, &session);
                        let _ = engine_handle
                            .send(Op::SyncSession {
                                session_id: app.current_session_id.clone(),
                                messages: app.api_messages.clone(),
                                system_prompt: app.system_prompt.clone(),
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
                                &session_id[..8.min(session_id.len())]
                            ));
                        }
                    }
                    Err(err) => {
                        app.status_message =
                            Some(format!("Failed to load session {session_id}: {err}"));
                    }
                }
            }
            ViewEvent::SessionDeleted { session_id, title } => {
                app.status_message = Some(format!(
                    "Deleted session {} ({})",
                    &session_id[..8.min(session_id.len())],
                    title
                ));
            }
            ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            } => {
                let result = commands::set_config_value(app, &key, &value, persist);
                if let Some(msg) = result.message {
                    app.add_message(HistoryCell::System { content: msg });
                }

                if let Some(action) = result.action {
                    match action {
                        AppAction::UpdateCompaction(compaction) => {
                            apply_model_and_compaction_update(engine_handle, compaction).await;
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
                    match commands::persist_status_items(&items) {
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
                effort,
                previous_model,
                previous_effort,
            } => {
                apply_model_picker_choice(
                    app,
                    engine_handle,
                    model,
                    effort,
                    previous_model,
                    previous_effort,
                )
                .await;
            }
            ViewEvent::ProviderPickerApplied { provider } => {
                switch_provider(app, engine_handle, config, provider, None).await;
            }
            ViewEvent::ProviderPickerApiKeySubmitted { provider, api_key } => {
                apply_provider_picker_api_key(app, engine_handle, config, provider, api_key).await;
            }
            ViewEvent::ModeSelected { mode } => {
                let msg = commands::switch_mode(app, mode);
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
            ViewEvent::ContextMenuSelected { action } => {
                handle_context_menu_action(app, action);
            }
            ViewEvent::ShellControlBackground => {
                request_foreground_shell_background(app);
            }
            ViewEvent::ShellControlCancel => {
                app.backtrack.reset();
                engine_handle.cancel();
                app.is_loading = false;
                app.dispatch_started_at = None;
                app.streaming_state.reset();
                app.runtime_turn_status = None;
                app.finalize_active_cell_as_interrupted();
                app.finalize_streaming_assistant_as_interrupted();
                app.status_message = Some("Request cancelled".to_string());
            }
        }
    }

    Ok(false)
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

/// Persist the typed API key to `~/.deepseek/config.toml`, refresh the
/// in-memory config so the engine can see it, then switch to the provider.
async fn apply_provider_picker_api_key(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    provider: ApiProvider,
    api_key: String,
) {
    use crate::config::{ProviderConfig, ProvidersConfig, save_api_key_for};

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
            ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
            ApiProvider::Openai => &mut providers.openai,
            ApiProvider::Openrouter => &mut providers.openrouter,
            ApiProvider::Novita => &mut providers.novita,
            ApiProvider::Fireworks => &mut providers.fireworks,
            ApiProvider::Sglang => &mut providers.sglang,
            ApiProvider::Vllm => &mut providers.vllm,
            ApiProvider::Ollama => &mut providers.ollama,
        };
        entry.api_key = Some(api_key);
    }

    switch_provider(app, engine_handle, config, provider, None).await;
}

fn apply_loaded_session(app: &mut App, session: &SavedSession) -> bool {
    let (messages, recovered_draft) = recover_interrupted_user_tail(&session.messages);
    app.api_messages = messages;
    app.clear_history();
    app.tool_cells.clear();
    app.tool_details_by_cell.clear();
    app.active_cell = None;
    app.active_tool_details.clear();
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
    app.model.clone_from(&session.metadata.model);
    app.update_model_compaction_budget();
    app.workspace.clone_from(&session.metadata.workspace);
    app.session.total_tokens = u32::try_from(session.metadata.total_tokens).unwrap_or(u32::MAX);
    app.session.total_conversation_tokens = app.session.total_tokens;
    app.session.session_cost = 0.0;
    app.session.session_cost_cny = 0.0;
    app.session.subagent_cost = 0.0;
    app.session.subagent_cost_cny = 0.0;
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
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    app.session.turn_cache_history.clear();
    app.current_session_id = Some(session.metadata.id.clone());
    app.session_artifacts = session.artifacts.clone();
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

fn refresh_workspace_context_if_needed(app: &mut App, now: Instant, allow_refresh: bool) {
    // Drain the async cell result into the live field first, so the render
    // path always reads the latest value (#399 S1).
    if let Ok(mut cell) = app.workspace_context_cell.lock()
        && let Some(ctx) = cell.take()
    {
        app.workspace_context = Some(ctx);
    }

    if app
        .workspace_context_refreshed_at
        .is_some_and(|refreshed_at| {
            now.duration_since(refreshed_at) < Duration::from_secs(WORKSPACE_CONTEXT_REFRESH_SECS)
        })
    {
        return;
    }

    if !allow_refresh {
        return;
    }

    // Offload git query to a background thread when a Tokio runtime is
    // available. Fall back to synchronous execution for tests and other
    // non-async contexts (#399 S1).
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let ctx = app.workspace_context_cell.clone();
        let workspace = app.workspace.clone();
        handle.spawn_blocking(move || {
            let result = collect_workspace_context(&workspace);
            if let Ok(mut guard) = ctx.lock() {
                *guard = result;
            }
        });
    } else {
        // No runtime — run synchronously so tests and one-shot callers
        // still get a result immediately.
        app.workspace_context = collect_workspace_context(&app.workspace);
    }
    app.workspace_context_refreshed_at = Some(now);
}

#[derive(Debug, Default, Clone, Copy)]
struct WorkspaceChangeSummary {
    staged: usize,
    modified: usize,
    untracked: usize,
    conflicts: usize,
}

impl WorkspaceChangeSummary {
    fn is_clean(&self) -> bool {
        self.staged == 0 && self.modified == 0 && self.untracked == 0 && self.conflicts == 0
    }
}

fn collect_workspace_context(workspace: &Path) -> Option<String> {
    let branch = workspace_git_branch(workspace)?;
    let summary = workspace_git_change_summary(workspace)?;

    let mut parts = Vec::new();
    if summary.staged > 0 {
        parts.push(format!("{} staged", summary.staged));
    }
    if summary.modified > 0 {
        parts.push(format!("{} modified", summary.modified));
    }
    if summary.untracked > 0 {
        parts.push(format!("{} untracked", summary.untracked));
    }
    if summary.conflicts > 0 {
        parts.push(format!("{} conflicts", summary.conflicts));
    }

    let status = if summary.is_clean() {
        "clean".to_string()
    } else {
        parts.join(", ")
    };

    Some(format!("{branch} | {status}"))
}

fn workspace_git_branch(workspace: &Path) -> Option<String> {
    let branch = run_git_query(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let branch = branch.trim().to_string();
    if branch == "HEAD" || branch.is_empty() {
        let short_hash = run_git_query(workspace, &["rev-parse", "--short", "HEAD"]).ok()?;
        let short_hash = short_hash.trim();
        if short_hash.is_empty() {
            return None;
        }
        return Some(format!("detached:{short_hash}"));
    }
    Some(branch)
}

fn workspace_git_change_summary(workspace: &Path) -> Option<WorkspaceChangeSummary> {
    let status = run_git_query(
        workspace,
        &["status", "--short", "--untracked-files=normal"],
    )
    .ok()?;

    if status.trim().is_empty() {
        return Some(WorkspaceChangeSummary::default());
    }

    let mut summary = WorkspaceChangeSummary::default();
    for line in status.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let mut chars = line.chars();
        let staged = chars.next()?;
        let modified = chars.next().unwrap_or(' ');

        if staged == ' ' && modified == ' ' {
            continue;
        }
        if staged == '?' && modified == '?' {
            summary.untracked = summary.untracked.saturating_add(1);
            continue;
        }

        if staged == 'U' || modified == 'U' {
            summary.conflicts = summary.conflicts.saturating_add(1);
        }
        if staged != ' ' && staged != '?' {
            summary.staged = summary.staged.saturating_add(1);
        }
        if modified != ' ' && modified != '?' {
            summary.modified = summary.modified.saturating_add(1);
        }
    }

    Some(summary)
}

fn run_git_query(workspace: &Path, args: &[&str]) -> std::io::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other("git command failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(terminal.backend_mut(), DisableFocusChange)?;
    disable_raw_mode()?;
    if use_alt_screen {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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
) -> Result<()> {
    enable_raw_mode()?;
    if use_alt_screen {
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    }
    recover_terminal_modes(
        terminal.backend_mut(),
        use_mouse_capture,
        use_bracketed_paste,
    );
    reset_terminal_viewport(terminal)?;
    Ok(())
}

fn reset_terminal_viewport(terminal: &mut AppTerminal) -> Result<()> {
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
    let _ = terminal.backend_mut().write_all(BEGIN_SYNC_UPDATE);

    let result = (|| -> Result<()> {
        terminal.backend_mut().write_all(TERMINAL_ORIGIN_RESET)?;
        terminal.backend_mut().flush()?;
        terminal.clear()?;
        Ok(())
    })();

    // Always end the synchronized update, regardless of success or failure.
    let _ = terminal.backend_mut().write_all(END_SYNC_UPDATE);
    let _ = terminal.backend_mut().flush();
    result
}

fn push_keyboard_enhancement_flags<W: Write>(writer: &mut W) {
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
fn recover_terminal_modes<W: Write>(
    writer: &mut W,
    use_mouse_capture: bool,
    use_bracketed_paste: bool,
) {
    push_keyboard_enhancement_flags(writer);
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

fn status_color(level: StatusToastLevel) -> ratatui::style::Color {
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
fn render_toast_stack_overlay(f: &mut Frame, full_area: Rect, footer_area: Rect, app: &mut App) {
    let toasts = app.active_status_toasts(TOAST_STACK_MAX_VISIBLE);
    if toasts.len() < 2 || footer_area.y == 0 {
        return;
    }
    // Drop the most recent (rendered inline by the footer), keep the rest.
    let extra = toasts.len() - 1;
    let stack_height = extra.min(TOAST_STACK_MAX_VISIBLE - 1) as u16;
    let max_above = footer_area.y.min(full_area.height);
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

fn render_footer(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // Pull in the toast first so we don't re-borrow `app` mutably mid-build,
    // then build the FooterProps once. The widget itself is a pure render —
    // it owns no `App` knowledge; all width-aware layout lives in the widget.
    //
    // The quit-confirmation prompt takes precedence over normal status toasts
    // because it represents a transient instruction the user must respond to
    // within ~2s. Mirrors codex-rs's `FooterMode::QuitShortcutReminder`.
    let quit_prompt = if app.quit_is_armed() {
        Some(FooterToast {
            text: crate::localization::tr(
                app.ui_locale,
                crate::localization::MessageId::FooterPressCtrlCAgain,
            )
            .to_string(),
            color: palette::STATUS_WARNING,
        })
    } else {
        None
    };
    let toast = quit_prompt.or_else(|| {
        app.active_status_toast().map(|toast| FooterToast {
            text: toast.text,
            color: status_color(toast.level),
        })
    });

    // Drive every cluster from the user's configured `status_items`. Mode
    // and Model are always rendered by `FooterProps` itself (their position
    // is structural — cluster gating is handled by the widget), so we only
    // gate the optional clusters here. If a variant is missing from
    // `status_items`, its span vec stays empty and the footer hides it.
    let mut props = render_footer_from(app, &app.status_items, toast);
    // FooterProps is mut so the working-strip animation can layer on top.

    // Animate the spacer between the left status line and the right-hand
    // chips whenever a turn is live: model loading/streaming, compacting, or
    // sub-agents in flight. The spout strip is gated on `fancy_animations`
    // (the "do I want a whale at all" knob); `low_motion` now governs only
    // streaming pacing (typewriter vs upstream), not the spout. Dot-pulse
    // counter ticks every 400 ms so `working` → `working...` reads at a
    // calm pace regardless of motion mode.
    if footer_working_strip_active(app) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let dot_frame = now_ms / 400;
        // Surface one compact live status row in the footer whenever a turn
        // is live. Tool turns get the current action plus active/done counts;
        // non-tool work falls back to the existing dot-pulse label.
        props.state_label = active_subagent_status_label(app)
            .or_else(|| active_tool_status_label(app))
            .unwrap_or_else(|| crate::tui::widgets::footer_working_label(dot_frame, app.ui_locale));
        props.state_color = palette::DEEPSEEK_SKY;

        // Water-spout frame source: wall-clock milliseconds. The sine-wave
        // math in `footer_working_strip_glyph_at` was tuned for this cadence
        // (`t = frame / 1000.0`, primary term × 8.0 ≈ 1.3 Hz at 1 ms ticks),
        // so frame must advance at ~1000 units/sec to produce the intended
        // animation feel. `fancy_animations = false` hides the strip
        // entirely; the textual `working...` pulse still keeps a heartbeat
        // regardless.
        if app.fancy_animations {
            props.working_strip_frame = Some(now_ms);
        }
    } else if props.state_label == "ready"
        && let Some(label) = selected_detail_footer_label(app)
    {
        props.state_label = label;
        props.state_color = palette::TEXT_MUTED;
    }

    let widget = FooterWidget::new(props);
    let buf = f.buffer_mut();
    widget.render(area, buf);
}

/// Whether the footer should animate the water-spout strip. Driven by the
/// underlying live-work flags so the strip stays visible for the *entire*
/// turn — not just the moments where bytes are streaming. `is_loading` can
/// flicker off between LLM rounds within a single turn (tool execution,
/// reasoning replay, capacity refresh, etc.), so we ALSO gate on the turn
/// itself still being in flight via `runtime_turn_status == "in_progress"`.
/// Without that, the user sees the strip vanish for seconds at a time even
/// though the agent is still working.
fn footer_working_strip_active(app: &App) -> bool {
    let turn_in_progress = app.runtime_turn_status.as_deref() == Some("in_progress");
    app.is_loading || app.is_compacting || running_agent_count(app) > 0 || turn_in_progress
}

fn is_noisy_subagent_progress(status: &str) -> bool {
    let status = status.trim().to_ascii_lowercase();
    status.contains("requesting model response")
}

fn subagent_objective_summary(app: &App, id: &str) -> Option<String> {
    app.subagent_cache
        .iter()
        .find(|agent| agent.agent_id == id)
        .map(|agent| summarize_tool_output(&agent.assignment.objective))
        .filter(|summary| !summary.is_empty())
}

fn friendly_subagent_progress(app: &App, id: &str, status: &str) -> String {
    if !is_noisy_subagent_progress(status) {
        return summarize_tool_output(status);
    }

    if let Some(summary) = subagent_objective_summary(app, id) {
        return format!("working on {summary}");
    }
    if let Some(existing) = app.agent_progress.get(id)
        && !is_noisy_subagent_progress(existing)
        && existing != "working"
    {
        return existing.clone();
    }
    "working".to_string()
}

fn active_subagent_status_label(app: &App) -> Option<String> {
    let running = running_agent_count(app);
    let fanout = active_fanout_counts(app);
    let (display_running, total) = if let Some((fanout_running, fanout_total)) = fanout {
        if fanout_running == 0 {
            return None;
        }
        (fanout_running, fanout_total)
    } else {
        if running == 0 {
            return None;
        }
        (running, running)
    };
    let detail = app
        .subagent_cache
        .iter()
        .find(|agent| matches!(agent.status, SubAgentStatus::Running))
        .map(|agent| summarize_tool_output(&agent.assignment.objective))
        .filter(|summary| !summary.is_empty())
        .or_else(|| {
            app.agent_progress
                .values()
                .find(|value| !is_noisy_subagent_progress(value) && value.as_str() != "working")
                .cloned()
        })
        .unwrap_or_else(|| "working".to_string());
    let detail = truncate_line_to_width(&detail, 34);
    let elapsed = app
        .agent_activity_started_at
        .or(app.turn_started_at)
        .map(|started| format!("{}s", started.elapsed().as_secs()));

    let mut parts = vec![format!("agents {display_running}/{total}"), detail];
    if let Some(elapsed) = elapsed {
        parts.push(elapsed);
    }
    parts.push("Alt+4".to_string());
    Some(parts.join(" \u{00B7} "))
}

#[derive(Default)]
struct ActiveToolStatusSnapshot {
    primary_running: Option<String>,
    primary_any: Option<String>,
    running: usize,
    completed: usize,
    started_at: Option<Instant>,
}

impl ActiveToolStatusSnapshot {
    fn record(&mut self, label: String, status: ToolStatus, started_at: Option<Instant>) {
        if self.primary_any.is_none() {
            self.primary_any = Some(label.clone());
        }
        if status == ToolStatus::Running {
            self.running += 1;
            if self.primary_running.is_none() {
                self.primary_running = Some(label);
            }
        } else {
            self.completed += 1;
        }
        if let Some(started) = started_at {
            self.started_at = Some(match self.started_at {
                Some(current) => current.min(started),
                None => started,
            });
        }
    }

    fn total(&self) -> usize {
        self.running + self.completed
    }
}

fn active_tool_status_label(app: &App) -> Option<String> {
    let active = app.active_cell.as_ref()?;
    if active.is_empty() {
        return None;
    }

    let mut snapshot = ActiveToolStatusSnapshot::default();
    for cell in active.entries() {
        collect_active_tool_status(cell, &mut snapshot);
    }
    if snapshot.total() == 0 {
        return None;
    }

    let primary = snapshot
        .primary_running
        .or(snapshot.primary_any)
        .unwrap_or_else(|| "tools".to_string());
    let primary = truncate_line_to_width(&primary, 30);
    let elapsed = snapshot
        .started_at
        .or(app.turn_started_at)
        .map(|started| format!("{}s", started.elapsed().as_secs()));

    let mut parts = vec![
        primary,
        format!("{} active", snapshot.running),
        format!("{} done", snapshot.completed),
    ];
    if let Some(elapsed) = elapsed {
        parts.push(elapsed);
    }
    if active_foreground_shell_running(app) {
        parts.push("Ctrl+B shell".to_string());
    }
    parts.push(tool_details_shortcut_label().to_string());
    Some(parts.join(" \u{00B7} "))
}

fn open_shell_control(app: &mut App) {
    if !app.is_loading || !active_foreground_shell_running(app) {
        app.status_message = Some("No foreground shell command to control".to_string());
        return;
    }

    app.view_stack.push(ShellControlView::new());
    app.status_message = Some("Shell control opened".to_string());
}

fn request_foreground_shell_background(app: &mut App) {
    if !app.is_loading || !active_foreground_shell_running(app) {
        app.status_message = Some("No foreground shell command to background".to_string());
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

fn active_foreground_shell_running(app: &App) -> bool {
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

fn terminal_pause_has_live_owner(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|cell| {
            matches!(
                cell,
                HistoryCell::Tool(ToolCell::Exec(exec)) if exec.status == ToolStatus::Running
            )
        })
    })
}

fn collect_active_tool_status(cell: &HistoryCell, snapshot: &mut ActiveToolStatusSnapshot) {
    let HistoryCell::Tool(tool) = cell else {
        return;
    };
    match tool {
        ToolCell::Exec(exec) => snapshot.record(
            format!("run {}", one_line_summary(&exec.command, 80)),
            exec.status,
            exec.started_at,
        ),
        ToolCell::Exploring(explore) => {
            for entry in &explore.entries {
                snapshot.record(
                    format!("read {}", one_line_summary(&entry.label, 80)),
                    entry.status,
                    None,
                );
            }
        }
        ToolCell::PlanUpdate(plan) => {
            snapshot.record("update plan".to_string(), plan.status, None);
        }
        ToolCell::PatchSummary(patch) => {
            snapshot.record(format!("patch {}", patch.path), patch.status, None);
        }
        ToolCell::Review(review) => {
            let target = one_line_summary(&review.target, 80);
            let label = if target.is_empty() {
                "review".to_string()
            } else {
                format!("review {target}")
            };
            snapshot.record(label, review.status, None);
        }
        ToolCell::DiffPreview(diff) => {
            snapshot.record(format!("diff {}", diff.title), ToolStatus::Success, None);
        }
        ToolCell::Mcp(mcp) => snapshot.record(format!("tool {}", mcp.tool), mcp.status, None),
        ToolCell::ViewImage(image) => snapshot.record(
            format!("image {}", image.path.display()),
            ToolStatus::Success,
            None,
        ),
        ToolCell::WebSearch(search) => {
            snapshot.record(format!("search {}", search.query), search.status, None);
        }
        ToolCell::Generic(generic) => {
            // Sub-agent dispatch represents itself through the DelegateCard
            // + Agents sidebar. Counting it again here would duplicate the
            // status. RLM is different today: it is a foreground tool call,
            // so keep it in the live tool footer until the async RLM
            // workbench lands (#513).
            if generic.name == "agent_spawn" {
                return;
            }
            snapshot.record(format!("tool {}", generic.name), generic.status, None);
        }
    }
}

fn one_line_summary(text: &str, max_width: usize) -> String {
    truncate_line_to_width(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        max_width,
    )
}

/// Build [`FooterProps`] from a user-configured `status_items` slice.
///
/// Variants are routed to their structural cluster: `Mode` and `Model` are
/// always emitted (the widget needs them to lay out the line correctly even
/// when the user toggled them off the picker — we honour the toggle by
/// blanking their visible content rather than collapsing the layout).
/// `Cost` and `Status` belong in the left cluster; the rest in the right.
///
/// A variant absent from `items` produces an empty span vec, which the
/// footer widget already hides cleanly. This keeps the renderer fully
/// data-driven without changing `FooterProps`'s public shape.
fn render_footer_from(
    app: &App,
    items: &[crate::config::StatusItem],
    toast: Option<FooterToast>,
) -> FooterProps {
    use crate::config::StatusItem as S;
    let has = |item: S| items.contains(&item);

    let (state_label, state_color) = if has(S::Status) {
        footer_state_label(app)
    } else {
        // "ready" is the sentinel the widget uses to skip the status segment;
        // pair it with theme text_muted for visual neutrality.
        ("ready", app.ui_theme.text_muted)
    };

    let coherence = if has(S::Coherence) {
        footer_coherence_spans(app)
    } else {
        Vec::new()
    };
    let agents = if has(S::Agents) {
        crate::tui::widgets::footer_agents_chip(running_agent_count(app), app.ui_locale)
    } else {
        Vec::new()
    };
    let reasoning_replay = if has(S::ReasoningReplay) {
        footer_reasoning_replay_spans(app)
    } else {
        Vec::new()
    };
    let cache = if has(S::Cache) {
        footer_cache_spans(app)
    } else {
        Vec::new()
    };
    let cost = if has(S::Cost) {
        footer_cost_spans(app)
    } else {
        Vec::new()
    };

    // Build the props; `Mode` and `Model` toggles modulate downstream by
    // blanking the rendered text rather than restructuring the widget — the
    // user is opting out of the chip, not destroying the bar.
    let mut props = FooterProps::from_app(
        app,
        toast,
        state_label,
        state_color,
        coherence,
        agents,
        reasoning_replay,
        cache,
        cost,
    );
    if !has(S::Mode) {
        props.mode_label = "";
    }
    if !has(S::Model) {
        props.model.clear();
    }

    // Right-cluster extension chips: append in `items` order so user
    // ordering is preserved across the new variants.
    let mut extra: Vec<Span<'static>> = Vec::new();
    for item in items {
        let chip = match *item {
            S::ContextPercent => footer_context_percent_spans(app),
            S::GitBranch => footer_git_branch_spans(app),
            S::LastToolElapsed | S::RateLimit => Vec::new(),
            _ => continue,
        };
        if chip.is_empty() {
            continue;
        }
        if !extra.is_empty() {
            extra.push(Span::raw("  "));
        }
        extra.extend(chip);
    }
    if !extra.is_empty() {
        // Stack into the cache slot — last existing right-cluster pipe — so
        // they appear adjacent without changing FooterProps's API. Keep
        // existing cache spans first so cache hit rate stays before the
        // user-added extras.
        if !props.cache.is_empty() {
            props.cache.push(Span::raw("  "));
        }
        props.cache.extend(extra);
    }

    props
}

fn footer_git_branch_spans(app: &App) -> Vec<Span<'static>> {
    let Some(branch) = workspace_git_branch(&app.workspace) else {
        return Vec::new();
    };
    vec![Span::styled(
        branch,
        Style::default().fg(app.ui_theme.text_muted),
    )]
}

/// Spans for the "context %" footer chip. Mirrors the header colour ramp so
/// the two surfaces stay visually consistent when both are enabled.
fn footer_context_percent_spans(app: &App) -> Vec<Span<'static>> {
    let Some((_, _, percent)) = context_usage_snapshot(app) else {
        return Vec::new();
    };
    let color = if percent >= 95.0 {
        palette::STATUS_ERROR
    } else if percent >= 85.0 {
        palette::STATUS_WARNING
    } else {
        palette::TEXT_MUTED
    };
    vec![Span::styled(
        format!("active ctx {percent:.0}%"),
        Style::default().fg(color),
    )]
}

fn footer_cost_spans(app: &App) -> Vec<Span<'static>> {
    let displayed_cost = app.displayed_session_cost_for_currency(app.cost_currency);
    if !should_show_footer_cost(displayed_cost) {
        return Vec::new();
    }
    vec![Span::styled(
        app.format_cost_amount(displayed_cost),
        Style::default().fg(palette::TEXT_MUTED),
    )]
}

fn should_show_footer_cost(displayed_cost: f64) -> bool {
    displayed_cost.is_finite() && displayed_cost > 0.0
}

/// Test-only helper retained as a parity reference for `FooterWidget`'s
/// auxiliary-span composition. Production rendering is performed by the
/// widget itself; the existing footer parity tests still exercise this
/// function directly to guard against drift.
#[allow(dead_code)]
fn footer_auxiliary_spans(app: &App, max_width: usize) -> Vec<Span<'static>> {
    // Context % is already shown in the header signal bar — don't
    // duplicate it in the footer. The footer carries unique info only:
    // coherence, in-flight sub-agents, reasoning replay tokens, cache hit
    // rate, and session cost.
    let coherence_spans = footer_coherence_spans(app);
    let agents_spans =
        crate::tui::widgets::footer_agents_chip(running_agent_count(app), app.ui_locale);
    let replay_spans = footer_reasoning_replay_spans(app);
    let cache_spans = footer_cache_spans(app);
    let cost_spans = footer_cost_spans(app);

    let parts: Vec<&Vec<Span<'static>>> = [
        &coherence_spans,
        &agents_spans,
        &replay_spans,
        &cache_spans,
        &cost_spans,
    ]
    .iter()
    .filter(|spans| !spans.is_empty())
    .copied()
    .collect();

    // Try to fit as many parts as possible, dropping from the end.
    for end in (0..=parts.len()).rev() {
        let mut combined = Vec::new();
        for (i, part) in parts[..end].iter().enumerate() {
            if i > 0 {
                combined.push(Span::raw("  "));
            }
            combined.extend(part.iter().cloned());
        }
        if spans_width(&combined) <= max_width {
            return combined;
        }
    }
    Vec::new()
}

fn footer_coherence_spans(app: &App) -> Vec<Span<'static>> {
    // Only surface coherence when the engine is actively intervening — the
    // user-facing signal is "we're doing something different now," not
    // "your conversation is getting complex," which the context-percent
    // header already covers. `GettingCrowded` is just a soft hint, so we
    // suppress it; the active interventions get their own visible label.
    let (label, color) = match app.coherence_state {
        CoherenceState::Healthy | CoherenceState::GettingCrowded => return Vec::new(),
        CoherenceState::RefreshingContext => ("refreshing context", palette::STATUS_WARNING),
        CoherenceState::VerifyingRecentWork => ("verifying", palette::DEEPSEEK_SKY),
        CoherenceState::ResettingPlan => ("resetting plan", palette::STATUS_ERROR),
    };

    vec![Span::styled(label.to_string(), Style::default().fg(color))]
}

fn footer_cache_spans(app: &App) -> Vec<Span<'static>> {
    if app.session.last_prompt_tokens.is_none() && app.session.last_completion_tokens.is_none() {
        return Vec::new();
    };
    let Some(hit_tokens) = app.session.last_prompt_cache_hit_tokens else {
        return vec![Span::styled(
            "Cache: unavailable",
            Style::default().fg(palette::TEXT_MUTED),
        )];
    };
    let miss_tokens = app
        .session
        .last_prompt_cache_miss_tokens
        .unwrap_or_else(|| {
            app.session
                .last_prompt_tokens
                .unwrap_or(0)
                .saturating_sub(hit_tokens)
        });
    let total = hit_tokens.saturating_add(miss_tokens);
    let percent = if total == 0 {
        0.0
    } else {
        (f64::from(hit_tokens) / f64::from(total) * 100.0).clamp(0.0, 100.0)
    };
    // Threshold-based coloring for cache hit rate (#396):
    //   >80%: green (good cache utilization)
    //   40-80%: yellow/warning
    //   <40%: red/dimmed (poor cache)
    let color = if percent > 80.0 {
        palette::STATUS_SUCCESS
    } else if percent >= 40.0 {
        palette::STATUS_WARNING
    } else {
        palette::STATUS_ERROR
    };
    vec![Span::styled(
        format!(
            "Cache: {:.1}% hit | hit {hit_tokens} | miss {miss_tokens}",
            percent
        ),
        Style::default().fg(color),
    )]
}

/// Render a footer chip showing the size of the `reasoning_content` block
/// replayed on the most recent thinking-mode tool-calling turn (#30).
///
/// Stays hidden when the count is zero (non-thinking models, first turn, or
/// turns with no tool calls). When replay tokens dominate the input budget
/// (>50%), the chip turns warning-coloured so users notice that thinking
/// replay is the main consumer of context.
fn footer_reasoning_replay_spans(app: &App) -> Vec<Span<'static>> {
    let Some(replay) = app.session.last_reasoning_replay_tokens else {
        return Vec::new();
    };
    if replay == 0 {
        return Vec::new();
    }
    let label = format!("rsn {}", format_token_count_compact(u64::from(replay)));
    let color = match app.session.last_prompt_tokens {
        Some(input) if input > 0 && f64::from(replay) / f64::from(input) > 0.5 => {
            palette::STATUS_WARNING
        }
        _ => palette::TEXT_MUTED,
    };
    vec![Span::styled(label, Style::default().fg(color))]
}

#[allow(dead_code)]
fn footer_toast_spans(
    toast: &crate::tui::app::StatusToast,
    max_width: usize,
) -> Vec<Span<'static>> {
    let truncated = truncate_line_to_width(&toast.text, max_width.max(1));
    vec![Span::styled(
        truncated,
        Style::default().fg(status_color(toast.level)),
    )]
}

#[allow(dead_code)]
fn footer_status_line_spans(app: &App, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 {
        return Vec::new();
    }

    let (mode_label, mode_color) = footer_mode_style(app);
    let (status_label, status_color) = footer_state_label(app);
    let sep = " \u{00B7} ";
    let show_status = status_label != "ready";

    let fixed_width = mode_label.width()
        + sep.width()
        + if show_status {
            sep.width() + status_label.width()
        } else {
            0
        };

    if max_width <= mode_label.width() {
        return vec![Span::styled(
            truncate_line_to_width(mode_label, max_width),
            Style::default().fg(mode_color),
        )];
    }

    let model_budget = max_width.saturating_sub(fixed_width).max(1);
    let model_label = truncate_line_to_width(&app.model, model_budget);

    let mut spans = vec![
        Span::styled(mode_label.to_string(), Style::default().fg(mode_color)),
        Span::styled(sep.to_string(), Style::default().fg(app.ui_theme.text_dim)),
        Span::styled(model_label, Style::default().fg(app.ui_theme.text_hint)),
    ];

    if show_status {
        spans.push(Span::styled(
            sep.to_string(),
            Style::default().fg(app.ui_theme.text_dim),
        ));
        spans.push(Span::styled(
            status_label.to_string(),
            Style::default().fg(status_color),
        ));
    }

    spans
}

fn footer_state_label(app: &App) -> (&'static str, ratatui::style::Color) {
    if app.is_compacting {
        return ("compacting \u{238B}", app.ui_theme.status_warning);
    }
    // Note: we deliberately do NOT show a "thinking" label for `is_loading`.
    // The animated water-spout strip in the footer's spacer is the visual
    // signal that the model is live; "thinking" was misleading because it
    // fired for every kind of in-flight work (tool calls, streaming, etc.),
    // not strictly reasoning. Sub-agents still surface "working" because
    // that's a distinct lifecycle the user can act on (open `/agents`).
    if running_agent_count(app) > 0 {
        return ("working", app.ui_theme.status_working);
    }
    if app.queued_draft.is_some() {
        return ("draft", app.ui_theme.text_muted);
    }

    if !app.view_stack.is_empty() {
        return ("overlay", app.ui_theme.text_muted);
    }

    if !app.input.is_empty() {
        return ("draft", app.ui_theme.text_muted);
    }

    ("ready", app.ui_theme.status_ready)
}

#[allow(dead_code)]
fn footer_mode_style(app: &App) -> (&'static str, ratatui::style::Color) {
    let label = app.mode.as_setting();
    let color = match app.mode {
        crate::tui::app::AppMode::Agent => app.ui_theme.mode_agent,
        crate::tui::app::AppMode::Yolo => app.ui_theme.mode_yolo,
        crate::tui::app::AppMode::Plan => app.ui_theme.mode_plan,
    };
    (label, color)
}

fn format_token_count_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[allow(dead_code)]
fn format_context_budget(used: i64, max: u32) -> String {
    let max_u64 = u64::from(max);
    let max_i64 = i64::from(max);

    if used > max_i64 {
        return format!(
            ">{}/{}",
            format_token_count_compact(max_u64),
            format_token_count_compact(max_u64)
        );
    }

    let used_u64 = u64::try_from(used.max(0)).unwrap_or(0);
    format!(
        "{}/{}",
        format_token_count_compact(used_u64),
        format_token_count_compact(max_u64)
    )
}

#[allow(dead_code)]
fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
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
        .map(|(cell_index, _)| cell_index);

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

fn context_usage_snapshot(app: &App) -> Option<(i64, u32, f64)> {
    let max = context_window_for_model(app.effective_model_for_budget())?;
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

    if percent < CONTEXT_WARNING_THRESHOLD_PERCENT {
        return;
    }

    let recommendation = if app.auto_compact {
        "Auto-compaction is enabled."
    } else {
        "Consider /compact or /clear."
    };

    if percent >= CONTEXT_CRITICAL_THRESHOLD_PERCENT {
        app.status_message = Some(format!(
            "Context critical: {:.0}% ({used}/{max} tokens). {recommendation}",
            percent
        ));
        return;
    }

    if app.status_message.is_none() {
        app.status_message = Some(format!(
            "Context high: {:.0}% ({used}/{max} tokens). {recommendation}",
            percent
        ));
    }
}

fn should_auto_compact_before_send(app: &App) -> bool {
    if !app.auto_compact {
        return false;
    }
    context_usage_snapshot(app)
        .map(|(_, _, pct)| pct >= CONTEXT_CRITICAL_THRESHOLD_PERCENT)
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

pub(crate) fn truncate_line_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    // For very small budgets, take chars until we exceed the *display* width.
    // Counting characters instead of widths (the previous behavior) overran
    // the budget for any double-width grapheme and contributed to mid-character
    // sidebar artifacts on resize (issue #65).
    if max_width <= 3 {
        let mut out = String::new();
        let mut width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > max_width {
                break;
            }
            out.push(ch);
            width += ch_width;
        }
        return out;
    }

    let mut out = String::new();
    let mut width = 0usize;
    let limit = max_width.saturating_sub(3);
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

fn handle_mouse_event(app: &mut App, mouse: MouseEvent) -> Vec<ViewEvent> {
    if app.view_stack.top_kind() == Some(ModalKind::ContextMenu) {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right)) {
            app.view_stack.pop();
            open_context_menu(app, mouse);
            return Vec::new();
        }
        return app.view_stack.handle_mouse(mouse);
    }

    if !app.view_stack.is_empty() {
        app.needs_redraw = true;
        return app.view_stack.handle_mouse(mouse);
    }

    match mouse.kind {
        MouseEventKind::ScrollUp => {
            let update = app.viewport.mouse_scroll.on_scroll(ScrollDirection::Up);
            app.viewport.pending_scroll_delta += update.delta_lines;
            if update.delta_lines != 0 {
                app.user_scrolled_during_stream = true;
                app.needs_redraw = true;
            }
        }
        MouseEventKind::ScrollDown => {
            let update = app.viewport.mouse_scroll.on_scroll(ScrollDirection::Down);
            app.viewport.pending_scroll_delta += update.delta_lines;
            if update.delta_lines != 0 {
                app.user_scrolled_during_stream = true;
                app.needs_redraw = true;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.viewport.transcript_scrollbar_dragging = false;
            app.viewport.selection_autoscroll = None;

            // Click on the transcript scrollbar gutter starts a scrollbar
            // drag so the visible thumb remains interactive for users who
            // prefer mouse-based navigation.
            if mouse_hits_transcript_scrollbar(app, mouse) {
                app.viewport.transcript_scrollbar_dragging = true;
                return Vec::new();
            }

            if mouse_hits_rect(mouse, app.viewport.jump_to_latest_button_area) {
                app.scroll_to_bottom();
                return Vec::new();
            }

            if let Some(point) = selection_point_from_mouse(app, mouse) {
                app.viewport.transcript_selection.anchor = Some(point);
                app.viewport.transcript_selection.head = Some(point);
                app.viewport.transcript_selection.dragging = true;

                if app.is_loading
                    && app.viewport.transcript_scroll.is_at_tail()
                    && let Some(anchor) = TranscriptScroll::anchor_for(
                        app.viewport.transcript_cache.line_meta(),
                        app.viewport.last_transcript_top,
                    )
                {
                    app.viewport.transcript_scroll = anchor;
                }
            } else if app.viewport.transcript_selection.is_active() {
                app.viewport.transcript_selection.clear();
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.viewport.transcript_scrollbar_dragging {
                scroll_transcript_to_mouse_row(app, mouse.row);
                return Vec::new();
            }

            if app.viewport.transcript_selection.dragging {
                update_selection_drag(app, mouse);
            }
        }
        MouseEventKind::Up(MouseButton::Left) if app.viewport.transcript_scrollbar_dragging => {
            app.viewport.transcript_scrollbar_dragging = false;
            app.viewport.selection_autoscroll = None;
            app.needs_redraw = true;
        }
        MouseEventKind::Up(MouseButton::Left) if app.viewport.transcript_selection.dragging => {
            app.viewport.transcript_selection.dragging = false;
            app.viewport.selection_autoscroll = None;
            if selection_has_content(app) {
                copy_active_selection(app);
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            open_context_menu(app, mouse);
        }
        _ => {}
    }

    Vec::new()
}

fn mouse_hits_transcript_scrollbar(app: &App, mouse: MouseEvent) -> bool {
    let Some(area) = app.viewport.last_transcript_area else {
        return false;
    };
    if area.width <= 1 || app.viewport.last_transcript_total <= app.viewport.last_transcript_visible
    {
        return false;
    }

    let scrollbar_col = area.x.saturating_add(area.width.saturating_sub(1));
    mouse.column == scrollbar_col
        && mouse.row >= area.y
        && mouse.row < area.y.saturating_add(area.height)
}

fn scroll_transcript_to_mouse_row(app: &mut App, row: u16) -> bool {
    let Some(area) = app.viewport.last_transcript_area else {
        return false;
    };
    let total = app.viewport.last_transcript_total;
    let visible = app.viewport.last_transcript_visible;
    if area.height == 0 || total <= visible {
        return false;
    }

    let max_start = total.saturating_sub(visible);
    if max_start == 0 {
        app.scroll_to_bottom();
        return true;
    }

    let max_row = usize::from(area.height.saturating_sub(1));
    let relative_row = usize::from(row.saturating_sub(area.y)).min(max_row);
    let numerator = relative_row
        .saturating_mul(max_start)
        .saturating_add(max_row / 2);
    // Round to the nearest transcript offset so short thumbs still feel
    // responsive on compact terminals.
    let top = numerator.checked_div(max_row).unwrap_or(0);

    app.viewport.transcript_scroll = if top >= max_start {
        TranscriptScroll::to_bottom()
    } else {
        TranscriptScroll::at_line(top)
    };
    app.viewport.pending_scroll_delta = 0;
    app.user_scrolled_during_stream = !app.viewport.transcript_scroll.is_at_tail();
    app.needs_redraw = true;
    true
}

/// Cadence between auto-scroll ticks while drag-selecting past the
/// transcript edge (#1163). 30 ms ≈ 33 lines/sec, comparable to the feel
/// of a steady scroll-wheel drag.
const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(30);

/// Update the transcript selection while the left button is dragging.
/// When the mouse leaves the transcript rect vertically, arm
/// `selection_autoscroll` so the main loop can advance the viewport on a
/// fixed cadence; when the mouse returns inside, disarm it.
fn update_selection_drag(app: &mut App, mouse: MouseEvent) {
    if let Some(point) = selection_point_from_mouse(app, mouse) {
        app.viewport.transcript_selection.head = Some(point);
        app.viewport.selection_autoscroll = None;
        return;
    }

    let Some(area) = app.viewport.last_transcript_area else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }

    let direction = if mouse.row < area.y {
        -1
    } else if mouse.row >= area.y.saturating_add(area.height) {
        1
    } else {
        // Outside horizontally only — leave selection head where it is.
        return;
    };

    let max_col = area.x.saturating_add(area.width.saturating_sub(1));
    let column = mouse.column.clamp(area.x, max_col);

    // Fire on the next tick immediately by setting `next_tick` to now.
    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        direction,
        column,
        next_tick: Instant::now(),
    });
    app.needs_redraw = true;
}

/// Advance the drag-edge auto-scroll one step if its cadence has elapsed.
/// Called once per main-loop iteration.
fn tick_selection_autoscroll(app: &mut App) {
    let Some(state) = app.viewport.selection_autoscroll else {
        return;
    };

    if !app.viewport.transcript_selection.dragging {
        app.viewport.selection_autoscroll = None;
        return;
    }

    let Some(area) = app.viewport.last_transcript_area else {
        return;
    };
    if area.height == 0 {
        return;
    }

    let now = Instant::now();
    if now < state.next_tick {
        return;
    }

    app.viewport.pending_scroll_delta = app
        .viewport
        .pending_scroll_delta
        .saturating_add(state.direction);
    app.user_scrolled_during_stream = true;

    let edge_row = if state.direction < 0 {
        area.y
    } else {
        area.y.saturating_add(area.height.saturating_sub(1))
    };
    if let Some(point) = selection_point_from_position(
        area,
        state.column,
        edge_row,
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_total,
        app.viewport.last_transcript_padding_top,
    ) {
        app.viewport.transcript_selection.head = Some(point);
    }

    app.viewport.selection_autoscroll = Some(SelectionAutoscroll {
        next_tick: now + SELECTION_AUTOSCROLL_INTERVAL,
        ..state
    });
    app.needs_redraw = true;
}

fn mouse_hits_rect(mouse: MouseEvent, area: Option<Rect>) -> bool {
    let Some(area) = area else {
        return false;
    };

    mouse.column >= area.x
        && mouse.column < area.x.saturating_add(area.width)
        && mouse.row >= area.y
        && mouse.row < area.y.saturating_add(area.height)
}

fn open_context_menu(app: &mut App, mouse: MouseEvent) {
    let entries = build_context_menu_entries(app, mouse);
    if entries.is_empty() {
        return;
    }
    app.view_stack
        .push(ContextMenuView::new(entries, mouse.column, mouse.row));
    app.needs_redraw = true;
}

fn build_context_menu_entries(app: &App, mouse: MouseEvent) -> Vec<ContextMenuEntry> {
    let mut entries = Vec::new();

    if selection_has_content(app) {
        entries.push(ContextMenuEntry {
            label: "Copy selection".to_string(),
            description: "write selected transcript text".to_string(),
            action: ContextMenuAction::CopySelection,
        });
        entries.push(ContextMenuEntry {
            label: "Open selection".to_string(),
            description: "show selected text in pager".to_string(),
            action: ContextMenuAction::OpenSelection,
        });
        entries.push(ContextMenuEntry {
            label: "Clear selection".to_string(),
            description: String::new(),
            action: ContextMenuAction::ClearSelection,
        });
    }

    if let Some(filtered_cell_index) = transcript_cell_index_from_mouse(app, mouse) {
        // Convert filtered index → original virtual index using the
        // mapping built in ChatWidget::new. When no cells are collapsed
        // this is an identity mapping.
        let cell_index = app
            .collapsed_cell_map
            .get(filtered_cell_index)
            .copied()
            .unwrap_or(filtered_cell_index);

        let target = detail_target_label(app, cell_index)
            .map(|label| truncate_line_to_width(&label, 28))
            .unwrap_or_else(|| "message".to_string());
        entries.push(ContextMenuEntry {
            label: "Open details".to_string(),
            description: target,
            action: ContextMenuAction::OpenDetails { cell_index },
        });
        entries.push(ContextMenuEntry {
            label: "Copy message".to_string(),
            description: "write clicked transcript cell".to_string(),
            action: ContextMenuAction::CopyCell { cell_index },
        });
        entries.push(ContextMenuEntry {
            label: "Open in editor".to_string(),
            description: "open file:line in $EDITOR".to_string(),
            action: ContextMenuAction::OpenFileAtLine { cell_index },
        });
        // Hide/show cell toggle.
        if app.collapsed_cells.contains(&cell_index) {
            entries.push(ContextMenuEntry {
                label: "Show cell".to_string(),
                description: "unhide this transcript cell".to_string(),
                action: ContextMenuAction::ShowCell { cell_index },
            });
        } else {
            entries.push(ContextMenuEntry {
                label: "Hide cell".to_string(),
                description: "collapse this transcript cell".to_string(),
                action: ContextMenuAction::HideCell { cell_index },
            });
        }
    }

    // When cells are hidden, offer a way to show them all.
    if !app.collapsed_cells.is_empty() {
        let count = app.collapsed_cells.len();
        entries.push(ContextMenuEntry {
            label: format!("Show hidden ({count})"),
            description: "unhide all collapsed cells".to_string(),
            action: ContextMenuAction::ShowAllHidden,
        });
    }

    entries.push(ContextMenuEntry {
        label: "Paste".to_string(),
        description: "insert clipboard into composer".to_string(),
        action: ContextMenuAction::Paste,
    });
    entries.push(ContextMenuEntry {
        label: "Command palette".to_string(),
        description: "commands, skills, and tools".to_string(),
        action: ContextMenuAction::OpenCommandPalette,
    });
    entries.push(ContextMenuEntry {
        label: "Context inspector".to_string(),
        description: "active context and cache hints".to_string(),
        action: ContextMenuAction::OpenContextInspector,
    });
    entries.push(ContextMenuEntry {
        label: "Help".to_string(),
        description: "keybindings and commands".to_string(),
        action: ContextMenuAction::OpenHelp,
    });

    entries
}

fn transcript_cell_index_from_mouse(app: &App, mouse: MouseEvent) -> Option<usize> {
    let point = selection_point_from_mouse(app, mouse)?;
    app.viewport
        .transcript_cache
        .line_meta()
        .get(point.line_index)
        .and_then(|meta| meta.cell_line())
        .map(|(cell_index, _)| cell_index)
}

fn handle_context_menu_action(app: &mut App, action: ContextMenuAction) {
    match action {
        ContextMenuAction::CopySelection => {
            copy_active_selection(app);
        }
        ContextMenuAction::OpenSelection => {
            if !open_pager_for_selection(app) {
                app.status_message = Some("No selection to open".to_string());
            }
        }
        ContextMenuAction::ClearSelection => {
            app.viewport.transcript_selection.clear();
            app.status_message = Some("Selection cleared".to_string());
        }
        ContextMenuAction::CopyCell { cell_index } => {
            copy_cell_to_clipboard(app, cell_index);
        }
        ContextMenuAction::OpenDetails { cell_index } => {
            if !open_details_pager_for_cell(app, cell_index) {
                app.status_message = Some("No details available for that line".to_string());
            }
        }
        ContextMenuAction::Paste => {
            app.paste_from_clipboard();
        }
        ContextMenuAction::OpenCommandPalette => {
            app.view_stack
                .push(CommandPaletteView::new(build_command_palette_entries(
                    app.ui_locale,
                    &app.skills_dir,
                    &app.workspace,
                    &app.mcp_config_path,
                    app.mcp_snapshot.as_ref(),
                )));
        }
        ContextMenuAction::OpenContextInspector => {
            open_context_inspector(app);
        }
        ContextMenuAction::OpenHelp => {
            app.view_stack.push(HelpView::new_for_locale(app.ui_locale));
        }
        ContextMenuAction::OpenFileAtLine { cell_index } => {
            let width = app
                .viewport
                .last_transcript_area
                .map(|area| area.width)
                .unwrap_or(80);
            let text = history_cell_to_text(
                app.cell_at_virtual_index(cell_index)
                    .unwrap_or(&HistoryCell::System {
                        content: String::new(),
                    }),
                width,
            );
            if crate::tui::history::try_open_file_at_line(&text, &app.workspace) {
                app.status_message = Some("Opened file in editor".to_string());
            } else {
                app.status_message = Some("No file:line pattern found in selection".to_string());
            }
        }
        ContextMenuAction::HideCell { cell_index } => {
            app.collapsed_cells.insert(cell_index);
            app.status_message = Some("Cell hidden".to_string());
        }
        ContextMenuAction::ShowCell { cell_index } => {
            app.collapsed_cells.remove(&cell_index);
            app.status_message = Some("Cell shown".to_string());
        }
        ContextMenuAction::ShowAllHidden => {
            let count = app.collapsed_cells.len();
            app.collapsed_cells.clear();
            app.status_message = Some(format!("{count} hidden cell(s) restored"));
        }
    }
    app.needs_redraw = true;
}

fn selection_point_from_mouse(app: &App, mouse: MouseEvent) -> Option<TranscriptSelectionPoint> {
    selection_point_from_position(
        app.viewport.last_transcript_area?,
        mouse.column,
        mouse.row,
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_total,
        app.viewport.last_transcript_padding_top,
    )
}

fn selection_point_from_position(
    area: Rect,
    column: u16,
    row: u16,
    transcript_top: usize,
    transcript_total: usize,
    padding_top: usize,
) -> Option<TranscriptSelectionPoint> {
    if column < area.x
        || column >= area.x + area.width
        || row < area.y
        || row >= area.y + area.height
    {
        return None;
    }

    if transcript_total == 0 {
        return None;
    }

    let row = row.saturating_sub(area.y) as usize;
    if row < padding_top {
        return None;
    }
    let row = row.saturating_sub(padding_top);

    let col = column.saturating_sub(area.x) as usize;
    let line_index = transcript_top
        .saturating_add(row)
        .min(transcript_total.saturating_sub(1));

    Some(TranscriptSelectionPoint {
        line_index,
        column: col,
    })
}

fn selection_has_content(app: &App) -> bool {
    selection_to_text(app).is_some_and(|text| !text.is_empty())
}

/// Branches taken by the Ctrl+C key handler. The order encodes priority and is
/// the unit-tested contract for #1337 / #1367: a transcript selection always
/// wins (so users learn that Ctrl+C copies when there's something to copy);
/// otherwise an active turn is interrupted; otherwise the quit-arm flow runs.
#[derive(Debug, PartialEq, Eq)]
enum CtrlCDisposition {
    CopySelection,
    CancelTurn,
    ConfirmExit,
    ArmExit,
}

fn ctrl_c_disposition(app: &App) -> CtrlCDisposition {
    if selection_has_content(app) {
        CtrlCDisposition::CopySelection
    } else if app.is_loading {
        CtrlCDisposition::CancelTurn
    } else if app.quit_is_armed() {
        CtrlCDisposition::ConfirmExit
    } else {
        CtrlCDisposition::ArmExit
    }
}

fn copy_active_selection(app: &mut App) {
    if !app.viewport.transcript_selection.is_active() {
        return;
    }
    if let Some(text) = selection_to_text(app).filter(|text| !text.is_empty()) {
        if app.clipboard.write_text(&text).is_ok() {
            app.status_message = Some("Selection copied".to_string());
        } else {
            app.status_message = Some("Copy failed".to_string());
        }
    } else {
        app.viewport.transcript_selection.clear();
        app.status_message = Some("No selection to copy".to_string());
    }
}

fn selection_to_text(app: &App) -> Option<String> {
    let (start, end) = app.viewport.transcript_selection.ordered_endpoints()?;
    let lines = app.viewport.transcript_cache.lines();
    if lines.is_empty() {
        return None;
    }
    let end_index = end.line_index.min(lines.len().saturating_sub(1));
    let start_index = start.line_index.min(end_index);

    let mut selected_lines = Vec::new();
    #[allow(clippy::needless_range_loop)]
    for line_index in start_index..=end_index {
        // Rail-prefix decorations are stored as cache metadata rather than
        // detected from glyphs, so new decoration types are covered without
        // changes to the copy path (#1163).
        let rail_width = app.viewport.transcript_cache.rail_prefix_width(line_index);
        // Convert the rendered line to plain text (strips OSC-8), then
        // slice off the rail prefix so subsequent column offsets operate
        // on content-only text.
        let full_text = line_to_plain(&lines[line_index]);
        let line_text = if rail_width > 0 {
            slice_text(&full_text, rail_width, text_display_width(&full_text))
        } else {
            full_text
        };
        let line_width = text_display_width(&line_text);
        // Selection coordinates are recorded in rendered-column space, which
        // includes the visual rail prefix. Add rail_width back so the column
        // window maps correctly into the rail-stripped text.
        let (raw_col_start, raw_col_end) = if start_index == end_index {
            (start.column, end.column)
        } else if line_index == start_index {
            (start.column, line_width.saturating_add(rail_width))
        } else if line_index == end_index {
            (0, end.column)
        } else {
            (0, line_width.saturating_add(rail_width))
        };

        let col_start = raw_col_start.saturating_sub(rail_width).min(line_width);
        let col_end = raw_col_end.saturating_sub(rail_width).min(line_width);

        let slice = slice_text(&line_text, col_start, col_end);
        selected_lines.push(slice);
    }
    Some(selected_lines.join("\n"))
}

fn open_pager_for_selection(app: &mut App) -> bool {
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

/// Open a pager showing the full thinking block. Targets the cell at the
/// current selection if it's a Thinking cell; otherwise falls back to the
/// most recent Thinking cell across the virtual transcript (history +
/// in-flight `active_cell`). Bound to Ctrl+O so users can read reasoning
/// content that's been collapsed in calm-mode rendering.
///
/// The virtual-index lookup matters: after `ThinkingComplete` fires the
/// finalized thinking entry sits in `active_cell` with `streaming = false`
/// until the active cell flushes to history. During that window the
/// transcript already renders the "thinking collapsed; press Ctrl+O for
/// full text" affordance, so the handler must address active-cell entries
/// or the affordance becomes a lie.
fn open_thinking_pager(app: &mut App) -> bool {
    let selected_cell = app
        .viewport
        .transcript_selection
        .ordered_endpoints()
        .and_then(|(start, _)| {
            app.viewport
                .transcript_cache
                .line_meta()
                .get(start.line_index)
                .and_then(|meta| meta.cell_line())
                .map(|(cell_index, _)| cell_index)
        })
        .filter(|&idx| {
            matches!(
                app.cell_at_virtual_index(idx),
                Some(crate::tui::history::HistoryCell::Thinking { .. })
            )
        });

    let target_idx = selected_cell.or_else(|| {
        (0..app.virtual_cell_count()).rev().find(|&idx| {
            matches!(
                app.cell_at_virtual_index(idx),
                Some(crate::tui::history::HistoryCell::Thinking { .. })
            )
        })
    });

    let Some(idx) = target_idx else {
        app.status_message = Some("No thinking blocks to expand".to_string());
        return true;
    };

    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    let text = {
        let Some(cell) = app.cell_at_virtual_index(idx) else {
            app.status_message = Some("No thinking blocks to expand".to_string());
            return true;
        };
        history_cell_to_text(cell, width)
    };
    app.view_stack.push(PagerView::from_text(
        "Thinking",
        &text,
        width.saturating_sub(2),
    ));
    true
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

fn open_details_pager_for_cell(app: &mut App, cell_index: usize) -> bool {
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

fn copy_cell_to_clipboard(app: &mut App, cell_index: usize) -> bool {
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
            .map(|(cell_index, _)| cell_index);
    }

    app.detail_cell_index_for_viewport(
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_visible.max(1),
        app.viewport.transcript_cache.line_meta(),
    )
    .or_else(|| app.history.len().checked_sub(1))
}

fn selected_detail_footer_label(app: &App) -> Option<String> {
    if app.viewport.transcript_selection.is_active() {
        return None;
    }
    let cell_index = app.detail_cell_index_for_viewport(
        app.viewport.last_transcript_top,
        app.viewport.last_transcript_visible.max(1),
        app.viewport.transcript_cache.line_meta(),
    )?;
    let label = detail_target_label(app, cell_index)?;
    Some(format!(
        "{} details: {}",
        tool_details_shortcut_label(),
        truncate_line_to_width(&label, 34)
    ))
}

fn detail_target_label(app: &App, cell_index: usize) -> Option<String> {
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
        HistoryCell::Tool(ToolCell::Generic(generic)) => Some(format!("tool {}", generic.name)),
        HistoryCell::SubAgent(_) => Some("sub-agent".to_string()),
        _ => None,
    }
}

fn is_copy_shortcut(key: &KeyEvent) -> bool {
    let is_c = matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'));
    if !is_c {
        return false;
    }

    if key.modifiers.contains(KeyModifiers::SUPER) {
        return true;
    }

    key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::SHIFT)
}

fn is_file_tree_toggle_shortcut(key: &KeyEvent) -> bool {
    let is_shifted_e = matches!(key.code, KeyCode::Char('E'))
        || (matches!(key.code, KeyCode::Char('e')) && key.modifiers.contains(KeyModifiers::SHIFT));
    if !is_shifted_e {
        return false;
    }

    let has_forbidden_modifier =
        key.modifiers.contains(KeyModifiers::ALT) || key.modifiers.contains(KeyModifiers::SUPER);
    let ctrl_shift_e = key.modifiers.contains(KeyModifiers::CONTROL) && !has_forbidden_modifier;

    let cmd_shift_e = key.modifiers.contains(KeyModifiers::SUPER)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT);

    ctrl_shift_e || cmd_shift_e
}

fn tool_details_shortcut_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "\u{2325}+V"
    } else {
        "Alt+V"
    }
}

/// Modifier predicate for the v0.8.30 family of `Alt+<letter>` transcript-
/// nav shortcuts (`Alt+G` / `Alt+Shift+G` / `Alt+[` / `Alt+]` / `Alt+?` /
/// `Alt+L` / `Alt+V`). Requires `Alt` and disallows `Ctrl` / `Super` so the
/// bindings don't collide with platform clipboard / window-management
/// shortcuts. `Shift` is permitted so the capital-letter forms work on
/// any keyboard layout that produces them as `Alt+Shift+key`.
///
/// Plain `Char` events (no modifier, or modifier=`Shift` alone for the
/// uppercase form) fall through to text insertion, which is the whole
/// point — typing "good morning" no longer eats the first `g`.
fn alt_nav_modifiers(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::ALT)
        && !modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::SUPER)
}

fn is_macos_option_v_legacy_key(key: &KeyEvent) -> bool {
    is_macos_option_v_legacy_key_for_platform(key, cfg!(target_os = "macos"))
}

fn is_macos_option_v_legacy_key_for_platform(key: &KeyEvent, is_macos: bool) -> bool {
    is_macos && key.modifiers.is_empty() && matches!(key.code, KeyCode::Char('\u{221A}'))
}

fn is_paste_shortcut(key: &KeyEvent) -> bool {
    let is_v = matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'));
    let is_legacy_ctrl_v = matches!(key.code, KeyCode::Char('\u{16}'));
    if !is_v && !is_legacy_ctrl_v {
        return false;
    }

    if is_legacy_ctrl_v {
        return true;
    }

    // Cmd+V on macOS
    if key.modifiers.contains(KeyModifiers::SUPER) {
        return true;
    }

    // Ctrl+V on Linux/Windows
    key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_text_input_key(key: &KeyEvent) -> bool {
    if matches!(key.code, KeyCode::Char(c) if c.is_control()) {
        return false;
    }

    !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
}

fn is_ctrl_h_backspace(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('h'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::SUPER)
}

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

#[cfg(test)]
mod tests;
