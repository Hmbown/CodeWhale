//! Desktop notifications for turn completion.
//!
//! Supports five delivery mechanisms:
//! - **OSC 9** — terminal escape sequence (`\x1b]9;…\x07`) for iTerm2,
//!   Ghostty, WezTerm, and tmux (with DCS passthrough).
//! - **Kitty** — OSC 99 protocol with ST terminator (no audible beep).
//! - **Ghostty** — OSC 777 notification protocol.
//! - **BEL** — audible bell (`\x07`) as a last-resort fallback.
//!
//! When `method = "auto"`, the resolver picks the best method for the
//! current terminal; Windows falls back to `Bel`, which is routed through
//! `MessageBeep(MB_OK)` for an audible default notification sound.

#[cfg(target_os = "windows")]
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE;

use std::io::{self, Write};
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Notification delivery method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// Automatically pick the best protocol for the current terminal.
    /// See [`resolve_method`] for the canonical resolution table.
    #[default]
    Auto,
    /// OSC 9 escape: `\x1b]9;<msg>\x07`
    Osc9,
    /// Plain BEL character: `\x07`
    Bel,
    /// osascript
    MacOS,
    /// Kitty notification protocol (OSC 99) with ST terminator.
    /// Uses `ESC ] 99 ; params ST` — no audible beep, unlike BEL.
    Kitty,
    /// Ghostty notification protocol (OSC 777).
    /// Uses `ESC ] 777 ; notify ; title ; message BEL`.
    Ghostty,
    /// Suppress all notifications.
    Off,
}

/// Emit a Windows system beep via `MessageBeep(MB_OK)`.
///
/// Writing BEL (`\\x07`) to the terminal is silent on most Windows
/// terminals (Windows Terminal, Conhost, etc.), so we call the Win32
/// API directly to produce the standard notification sound.
#[cfg(target_os = "windows")]
fn windows_bell() {
    // MB_OK = 0x00000000 — plays the default system sound. Best-effort: a
    // failed beep is not worth surfacing to the caller, so the Result is
    // discarded.
    unsafe {
        let _ = MessageBeep(MESSAGEBOX_STYLE(0));
    }
}

/// Resolve `Auto` to a concrete method by inspecting `$TERM_PROGRAM`,
/// `$LC_TERMINAL`, and `$TERM`.
///
/// Resolution table:
/// - `iTerm.app`, `WezTerm`, `Cmux` → `Osc9`
/// - `Ghostty` → `Ghostty` (OSC 777)
/// - `kitty` → `Kitty` (OSC 99)
/// - `$LC_TERMINAL` matches OSC-9 capable → `Osc9` (Cmux that sets LC_TERMINAL)
/// - `$TERM` contains `ghostty` → `Osc9` (cmux etc.)
/// - `$TERM` contains `kitty` → `Kitty`
/// - Unix unknown → `Bel`
/// - Windows unknown → `Bel`
#[must_use]
fn resolve_method() -> Method {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    match term_program.as_str() {
        "iTerm.app" | "WezTerm" | "Cmux" => return Method::Osc9,
        "Ghostty" => return Method::Ghostty,
        "kitty" => return Method::Kitty,
        _ => {}
    }

    // LC_TERMINAL fallback for terminals (e.g. Cmux) that set
    // LC_TERMINAL instead of TERM_PROGRAM.
    let lc_terminal = std::env::var("LC_TERMINAL").unwrap_or_default();
    match lc_terminal.as_str() {
        "iTerm.app" | "Ghostty" | "WezTerm" | "Cmux" => return Method::Osc9,
        _ => {}
    }

    // Windows: use BEL so `windows_bell()` (MessageBeep) fires on turn
    // completion.  Previous behavior returned `Off` to avoid the error chime
    // (#583), but `MessageBeep(MB_OK)` plays the *default system sound* —
    // distinct from the error sound — so BEL is safe and gives Windows users
    // audible feedback when a long turn finishes.
    if cfg!(target_os = "windows") {
        return Method::Bel;
    }

    if cfg!(target_os = "macos") {
        return Method::MacOS;
    }

    // Ghostty-based terminals (cmux, etc.) may not set their own
    // TERM_PROGRAM but do set TERM=xterm-ghostty. Likewise for Kitty.
    let term = std::env::var("TERM").unwrap_or_default();
    if term.contains("ghostty") {
        Method::Osc9
    } else if term.contains("kitty") {
        Method::Kitty
    } else {
        Method::Bel
    }
}

/// Wrap an escape sequence for terminal multiplexer passthrough.
///
/// tmux intercepts escape sequences; DCS passthrough tunnels them to
/// the outer terminal unmodified. Every ESC inside the payload is
/// doubled so tmux does not interpret it as DCS end.
fn wrap_for_multiplexer(seq: &str, in_tmux: bool) -> String {
    if in_tmux {
        let escaped = seq.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{escaped}\x1b\\")
    } else {
        seq.to_string()
    }
}

/// Build the raw escape bytes for the given method and message.
///
/// When `in_tmux` is `true`, OSC sequences are wrapped in DCS passthrough
/// so tmux forwards them to the outer terminal.
#[must_use]
fn build_escape(method: Method, in_tmux: bool, msg: &str) -> Vec<u8> {
    match method {
        Method::Bel => vec![b'\x07'],
        Method::Osc9 => {
            let inner = format!("\x1b]9;{msg}\x07");
            if in_tmux {
                let escaped_inner = inner.replace('\x1b', "\x1b\x1b");
                format!("\x1bPtmux;{escaped_inner}\x1b\\").into_bytes()
            } else {
                inner.into_bytes()
            }
        }
        Method::Kitty => {
            // Kitty notification: OSC 99 ; params ST
            // ST terminator (ESC \) instead of BEL to avoid audible beep.
            let title_seq = "\x1b]99;d=0:p=title\x1b\\";
            let body_seq = format!("\x1b]99;p=body;{msg}\x1b\\");
            let focus_seq = "\x1b]99;d=1:a=focus\x1b\\";
            let combined = format!("{title_seq}{body_seq}{focus_seq}");
            wrap_for_multiplexer(&combined, in_tmux).into_bytes()
        }
        Method::Ghostty => {
            // Ghostty notification: OSC 777 ; notify ; title ; message BEL
            let seq = format!("\x1b]777;notify;codewhale;{msg}\x07");
            wrap_for_multiplexer(&seq, in_tmux).into_bytes()
        }
        // Auto and Off and MacOS should not reach build_escape.
        Method::Auto | Method::Off | Method::MacOS => vec![],
    }
}

/// Emit a turn-complete notification to `sink` if the elapsed time meets or
/// exceeds `threshold`, and `method` is not `Off`.
///
/// This variant takes a `W: Write` sink for testability.
pub fn notify_done_to<W: Write>(
    method: Method,
    in_tmux: bool,
    msg: &str,
    threshold: Duration,
    elapsed: Duration,
    sink: &mut W,
) {
    if elapsed < threshold {
        return;
    }
    let effective = match method {
        Method::Off => return,
        Method::Auto => resolve_method(),
        other => other,
    };

    // macOS Notification Center: handled via osascript, not terminal escapes.
    #[cfg(target_os = "macos")]
    if Method::MacOS == effective {
        macos_display_notification(msg);
        return;
    }

    let bytes = build_escape(effective, in_tmux, msg);
    if bytes.is_empty() {
        return;
    }
    // Best-effort: ignore write errors (e.g. stdout closed).
    let _ = sink.write_all(&bytes);
    let _ = sink.flush();

    // On Windows, writing BEL (`\x07`) to the terminal is silent in most
    // terminals (Windows Terminal, Conhost, etc.). Call MessageBeep to
    // produce an actual notification sound via the system audio scheme.
    #[cfg(target_os = "windows")]
    if effective == Method::Bel {
        windows_bell();
    }
}

/// Emit a turn-complete notification to **stdout** if `elapsed >= threshold`.
///
/// With `method = Auto`, selects the best protocol for the current terminal
/// (OSC 9, Kitty OSC 99, Ghostty OSC 777, or Bel). The unknown-terminal
/// fallback is platform-aware: `Bel` on every platform, with Windows routing
/// it through `MessageBeep(MB_OK)` for a default system notification sound.
/// See [`resolve_method`] for the canonical resolution table. Pass
/// `in_tmux = true` (i.e. `$TMUX` is non-empty at runtime) to wrap OSC
/// sequences in a DCS passthrough.
pub fn notify_done(
    method: Method,
    in_tmux: bool,
    msg: &str,
    threshold: Duration,
    elapsed: Duration,
) {
    notify_done_to(method, in_tmux, msg, threshold, elapsed, &mut io::stdout());
}

/// Set the terminal taskbar progress state via OSC 9 ; 4.
///
/// Windows Terminal supports this to show progress on the taskbar icon:
/// - `state = 0` — no progress (clear)
/// - `state = 1` — indeterminate (cycling green)
/// - `state = 2` — normal (0-100, requires progress param)
/// - `state = 3` — error (red)
/// - `state = 4` — paused (yellow)
///
/// Other terminals (iTerm2, WezTerm) ignore the sequence silently.
/// Best-effort — write failures are ignored.
pub fn set_taskbar_progress(state: u8, progress: Option<u8>) {
    let seq = if let Some(pct) = progress {
        format!("\x1b]9;4;{state};{pct}\x07")
    } else {
        format!("\x1b]9;4;{state}\x07")
    };
    let mut stdout = io::stdout();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();
}

/// Set taskbar progress to indeterminate (cycling) — call at turn start.
pub fn set_taskbar_progress_busy() {
    set_taskbar_progress(1, None);
}

/// Clear taskbar progress — call at turn end.
pub fn clear_taskbar_progress() {
    set_taskbar_progress(0, None);
}

/// Animation frame characters for the terminal title.
/// Uses the DeepSeek whale emoji (🐳 spouting, 🐋 resting) to match the
/// existing header status indicator in the TUI.
const TITLE_FRAMES: &[&str] = &["🐳", "🐋", "🐳", "🐋"];
const TITLE_ANIMATION_INTERVAL: Duration = Duration::from_millis(800);

/// Shared flag controlling the title animation loop. Set to `true` by
/// `start_title_animation()`, cleared by `stop_title_animation()`.
static TITLE_ANIMATION_RUNNING: AtomicBool = AtomicBool::new(false);

/// Write OSC 0 (set window title) sequence.
fn set_terminal_title(title: &str) {
    let seq = format!("\x1b]0;{title}\x07");
    let mut stdout = io::stdout();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();
}

/// Tracks whether the ✅ completion marker was set, so
/// `reset_title_on_interaction()` can skip redundant writes.
static COMPLETION_MARKER_SHOWN: AtomicBool = AtomicBool::new(false);

/// Start an animated terminal title spinner.
///
/// Cycles the terminal title between 🐳→🐋 every 800ms while processing,
/// matching the whale status indicator in the TUI header, so alt-tabbed
/// users can see activity.
///
/// The animation runs in a background tokio task that checks
/// `TITLE_ANIMATION_RUNNING`. Each call restarts the animation with the
/// given `original` base title — safe to call on every turn start.
pub fn start_title_animation(original: &str) {
    // Signal any existing animation loop to exit, then start fresh.
    TITLE_ANIMATION_RUNNING.store(true, Ordering::SeqCst);
    let base = original.to_string();
    tokio::spawn(async move {
        let mut frame = 0usize;
        while TITLE_ANIMATION_RUNNING.load(Ordering::SeqCst) {
            // Yield once per frame so a racing stop_title_animation()
            // can observe the cleared flag and apply the completion
            // marker before the next frame write. Without this yield
            // the background task could overwrite the ✅ marker with
            // the next whale frame.
            tokio::task::yield_now().await;
            if !TITLE_ANIMATION_RUNNING.load(Ordering::SeqCst) {
                break;
            }
            let spinner = TITLE_FRAMES[frame % TITLE_FRAMES.len()];
            set_terminal_title(&format!("{spinner} {base}"));
            frame += 1;
            tokio::time::sleep(TITLE_ANIMATION_INTERVAL).await;
        }
        // Don't restore title here — stop_title_animation() handles
        // what to show on completion (e.g. ✅ marker).
    });
}

/// Stop the title animation and show a completion marker.
///
/// Sets the title to `✅ <base>` so alt-tabbed users see at a glance
/// that processing finished. The marker is overwritten on the next turn
/// by [`start_title_animation`].
pub fn stop_title_animation() {
    TITLE_ANIMATION_RUNNING.store(false, Ordering::SeqCst);
    COMPLETION_MARKER_SHOWN.store(false, Ordering::SeqCst);
    // Show ✅ marker only for beep mode. Bell mode already has its own
    // terminal-level visual indicator (flash/icon).
    let mode = COMPLETION_SOUND_MODE.load(Ordering::SeqCst);
    if mode == 1 {
        set_terminal_title("✅ CodeWhale");
    }
    play_completion_sound();
}

/// Stop the title animation without playing the completion sound.
///
/// Cancellation and failed turns should return the terminal title to rest
/// without presenting them as completed work.
pub fn stop_title_animation_quietly() {
    TITLE_ANIMATION_RUNNING.store(false, Ordering::SeqCst);
    COMPLETION_MARKER_SHOWN.store(false, Ordering::SeqCst);
    set_terminal_title("CodeWhale");
}

/// Clear the ✅ completion marker from the title when the user interacts.
///
/// Call this on every user input event (key press, mouse click) so the
/// marker doesn't persist once the user is back at the terminal.
pub fn reset_title_on_interaction() {
    if COMPLETION_MARKER_SHOWN.swap(false, Ordering::SeqCst) {
        set_terminal_title("CodeWhale");
    }
}

/// Completion sound mode (0 = off, 1 = beep, 2 = bell).
static COMPLETION_SOUND_MODE: AtomicU8 = AtomicU8::new(1);

/// Set the completion sound mode from config.
/// Call once at startup or on `/settings` change.
pub fn set_completion_sound_mode(mode: crate::config::CompletionSound) {
    let val = match mode {
        crate::config::CompletionSound::Off => 0u8,
        crate::config::CompletionSound::Beep => 1u8,
        crate::config::CompletionSound::Bell => 2u8,
    };
    COMPLETION_SOUND_MODE.store(val, Ordering::SeqCst);
}

/// Play the configured completion sound (if not `Off`).
pub fn play_completion_sound() {
    match COMPLETION_SOUND_MODE.load(Ordering::SeqCst) {
        0 => {} // Off
        1 => {
            beep_sound();
        }
        2 => {
            bell_sound();
        }
        _ => {}
    }
}

/// Play a short completion sound via the system beep.
///
/// On Windows uses `MessageBeep(MB_OK)` which plays the default system
/// notification sound. On other platforms writes `BEL` (`\x07`) to stdout.
#[cfg(target_os = "windows")]
fn beep_sound() {
    windows_bell();
}

/// Non-Windows: write BEL to stdout for the terminal bell.
#[cfg(not(target_os = "windows"))]
fn beep_sound() {
    let _ = io::stdout().write_all(b"\x07");
}

/// Pure terminal BEL character.
fn bell_sound() {
    let _ = io::stdout().write_all(b"\x07");
}

/// Show a macOS Notification Center alert via `osascript`.
///
/// Runs on a dedicated background thread so the caller is not blocked.
///
/// The notification includes:
/// - **Title**: "CodeWhale"
/// - **Subtitle**: First line of `msg` (when the message contains a newline,
///   e.g. the response preview from a completed turn)
/// - **Body**: Remaining lines of `msg`, or the full `msg` if single-line
/// - **Sound**: Default macOS notification sound
///
/// The message body is capped at 200 **characters** (not bytes) to keep the
/// bubble readable while correctly handling multi-byte text.
///
/// **Security**: The message is passed to `osascript` as a command-line
/// argument via `ARGV`, never embedded inline in the AppleScript source.
/// AppleScript does not treat backslash as an escape inside double-quoted
/// string literals, so the previous `\"` approach would terminate the
/// string at the `"` and leave any text between unbalanced quotes
/// evaluated as raw AppleScript code — a code-injection vector for
/// AI-generated notification text. Passing via `ARGV` avoids this
/// entirely because the message is never parsed as AppleScript syntax.
///
/// This is best-effort: if `osascript` is not available (e.g. headless SSH
/// session) the error is logged via `tracing::warn!` instead of silently
/// swallowed.
#[cfg(target_os = "macos")]
fn macos_display_notification(msg: &str) {
    let body = msg.to_string();

    // Spawn on a background thread so we don't block the caller.
    // osascript itself is fast (~50 ms), but spawning a subprocess
    // synchronously from an async context steals a tokio thread.
    let _ = std::thread::Builder::new()
        .name("osascript-notif".into())
        .spawn(move || {
            // Char-bounded truncation (not byte-bounded) so we don't slice
            // through a multi-byte sequence and emit invalid UTF-8.
            let body_str: String = body.chars().take(200).collect();

            // Build AppleScript that receives the message via ARGV
            // instead of inline string interpolation. AppleScript does
            // not treat backslash as an escape inside double-quoted
            // string literals, so `\"` would terminate the string at
            // the `"` and leave a dangling `\`. Passing the message as
            // a command-line argument avoids any injection risk.
            //
            // When the message has multiple lines, the first line
            // becomes the subtitle and the rest becomes the body —
            // this lets turn notifications show the response preview
            // in the subtitle and the duration/cost summary in the body.
            let mut args: Vec<String> = Vec::new();

            if let Some(idx) = body_str.find('\n') {
                let subtitle = body_str[..idx].trim();
                let body_text = body_str[idx + 1..].trim();
                args.extend_from_slice(&[
                    "-e".into(),
                    "on run argv".into(),
                    "-e".into(),
                    "set theBody to item 1 of argv".into(),
                    "-e".into(),
                    "set theSubtitle to item 2 of argv".into(),
                    "-e".into(),
                    "display notification theBody with title \"CodeWhale\" subtitle theSubtitle sound name \"default\"".into(),
                    "-e".into(),
                    "end run".into(),
                    "--".into(),
                    body_text.into(),
                    subtitle.into(),
                ]);
            } else {
                args.extend_from_slice(&[
                    "-e".into(),
                    "on run argv".into(),
                    "-e".into(),
                    "display notification (item 1 of argv) with title \"CodeWhale\" sound name \"default\"".into(),
                    "-e".into(),
                    "end run".into(),
                    "--".into(),
                    body_str,
                ]);
            }

            match std::process::Command::new("osascript")
                .args(&args)
                .output()
            {
                Ok(output) if !output.status.success() => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    tracing::warn!(stderr = %stderr, "osascript notification failed");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "osascript notification error");
                }
                _ => {}
            }
        });
}

/// Return a human-readable duration string, capped at two units so
/// it stays compact in headers and notifications.
///
/// Examples:
/// * `"45s"`, `"1m"`, `"1m 12s"`
/// * `"1h"`, `"3h 12m"` (#447 — was previously `"192m"` form)
/// * `"1d"`, `"2d 5h"` (#447 — multi-day sessions)
/// * `"1w"`, `"3w 2d"` (#447 — long-running automations)
///
/// The output drops the secondary unit when it's zero, so `"1h"`
/// rather than `"1h 0m"`. Sub-minute precision is dropped at the
/// hour mark and above; the goal is "is this a couple of hours or
/// a couple of days," not stopwatch accuracy.
#[must_use]
pub fn humanize_duration(d: Duration) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;

    let total = d.as_secs();
    if total == 0 {
        return "0s".to_string();
    }
    if total >= WEEK {
        let w = total / WEEK;
        let days = (total % WEEK) / DAY;
        return if days == 0 {
            format!("{w}w")
        } else {
            format!("{w}w {days}d")
        };
    }
    if total >= DAY {
        let days = total / DAY;
        let h = (total % DAY) / HOUR;
        return if h == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {h}h")
        };
    }
    if total >= HOUR {
        let h = total / HOUR;
        let m = (total % HOUR) / MINUTE;
        return if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        };
    }
    if total >= MINUTE {
        let m = total / MINUTE;
        let s = total % MINUTE;
        return if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        };
    }
    format!("{total}s")
}

// ── Per-turn notification composition ────────────────────────────────
//
// The helpers below decide *whether* to notify on a completed turn and
// *what message* to put in the body. The low-level dispatcher is
// `notify_done`; everything in this block sits in front of it.

use crate::models::{ContentBlock, Message};
use crate::tui::app::App;

/// Resolve the effective notification method/threshold/include-summary tuple
/// for a completed turn, taking the high-level
/// `[tui].notification_condition` override into account on top of the
/// lower-level `[notifications]` block.
///
/// Returns `None` to mean "do not notify" (either because the user set
/// `notification_condition = "never"` or because the resolved method is
/// `Off`).
pub fn settings(config: &crate::config::Config) -> Option<(Method, Duration, bool)> {
    let notif = config.notifications_config();
    // Initialize completion sound mode from config.
    set_completion_sound_mode(notif.completion_sound);
    let method = match notif.method {
        crate::config::NotificationMethod::Auto => Method::Auto,
        crate::config::NotificationMethod::Osc9 => Method::Osc9,
        crate::config::NotificationMethod::Bel => Method::Bel,
        crate::config::NotificationMethod::Kitty => Method::Kitty,
        crate::config::NotificationMethod::Ghostty => Method::Ghostty,
        crate::config::NotificationMethod::Off => Method::Off,
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
pub fn completed_turn_message(
    app: &App,
    current_streaming_text: &str,
    include_summary: bool,
    turn_elapsed: Duration,
    turn_cost: Option<crate::pricing::CostEstimate>,
) -> String {
    let mut msg = text_summary(current_streaming_text)
        .or_else(|| latest_assistant_text(&app.api_messages))
        .unwrap_or_else(|| "codewhale: turn complete".to_string());

    if include_summary {
        let human = humanize_duration(turn_elapsed);
        let summary = match turn_cost {
            Some(c) => {
                let cost = crate::pricing::format_cost_estimate(c, app.cost_currency);
                format!("codewhale: turn complete ({human}, {cost})")
            }
            None => format!("codewhale: turn complete ({human})"),
        };
        if msg == "codewhale: turn complete" {
            msg = summary;
        } else {
            msg.push('\n');
            msg.push_str(&summary);
        }
    }

    msg
}

/// Compose a notification body for a sub-agent completion. Falls back
/// to a generic "sub-agent X complete" if no human-readable line can
/// be teased out of the child's transcript.
pub fn subagent_completion_message(
    id: &str,
    result: &str,
    include_summary: bool,
    elapsed: Duration,
) -> String {
    let result_line = result
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("<codewhale:subagent.done>"));
    let mut msg = result_line
        .and_then(text_summary)
        .map(|summary| format!("sub-agent {id}: {summary}"))
        .unwrap_or_else(|| format!("codewhale: sub-agent {id} complete"));

    if include_summary {
        let human = humanize_duration(elapsed);
        msg.push('\n');
        msg.push_str(&format!("codewhale: sub-agent complete ({human})"));
    }

    msg
}

/// Find the latest assistant message in `messages` and return a
/// notification-ready summary of its `Text` content. Thinking blocks,
/// tool calls, and tool results are skipped — only the user-visible
/// reply contributes to the body.
pub fn latest_assistant_text(messages: &[Message]) -> Option<String> {
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
                    | ContentBlock::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            text_summary(&text)
        })
}

/// Sanitize + collapse + truncate streaming text into something fit to
/// hand the OS notification system. Returns `None` when nothing
/// useful remains after sanitization.
pub fn text_summary(text: &str) -> Option<String> {
    const MAX_CHARS: usize = 360;

    let sanitized = super::ui::sanitize_stream_chunk(text);
    let collapsed = sanitized
        .lines()
        .map(str::trim)
        .filter(|line: &&str| !line.is_empty())
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

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    /// Serialise all tests that mutate `TERM_PROGRAM` to prevent data races
    /// when the test harness runs them in parallel threads.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn capture(
        method: Method,
        in_tmux: bool,
        msg: &str,
        threshold_secs: u64,
        elapsed_secs: u64,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        notify_done_to(
            method,
            in_tmux,
            msg,
            Duration::from_secs(threshold_secs),
            Duration::from_secs(elapsed_secs),
            &mut buf,
        );
        buf
    }

    #[test]
    fn osc9_body_format() {
        let out = capture(Method::Osc9, false, "codewhale: done", 0, 1);
        assert_eq!(out, b"\x1b]9;codewhale: done\x07");
    }

    #[test]
    fn bel_emits_exactly_one_byte() {
        let out = capture(Method::Bel, false, "ignored", 0, 1);
        assert_eq!(out, b"\x07");
    }

    #[test]
    fn off_mode_emits_nothing() {
        let out = capture(Method::Off, false, "ignored", 0, 9999);
        assert!(out.is_empty());
    }

    #[test]
    fn kitty_escape_uses_st_terminator() {
        let out = capture(Method::Kitty, false, "done", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("99;"), "should have kitty OSC 99");
        assert!(s.contains("\x1b\\"), "kitty uses ST terminator");
        assert!(!s.contains("\x07"), "kitty should NOT use BEL");
    }

    #[test]
    fn ghostty_escape_format() {
        let out = capture(Method::Ghostty, false, "done", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.contains("777;notify;codewhale;done"),
            "should have ghostty seq"
        );
    }

    #[test]
    fn kitty_tmux_dcs_passthrough() {
        let out = capture(Method::Kitty, true, "hello", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1bPtmux;"), "should start with DCS");
        assert!(s.ends_with("\x1b\\"), "should end with ST");
    }

    #[test]
    fn ghostty_tmux_dcs_passthrough() {
        let out = capture(Method::Ghostty, true, "hello", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1bPtmux;"), "should start with DCS");
        assert!(s.ends_with("\x1b\\"), "should end with ST");
    }

    #[test]
    fn below_threshold_emits_nothing() {
        let out = capture(Method::Osc9, false, "msg", 30, 29);
        assert!(out.is_empty());
    }

    #[test]
    fn at_threshold_emits() {
        let out = capture(Method::Osc9, false, "msg", 30, 30);
        assert!(!out.is_empty());
    }

    #[test]
    fn tmux_dcs_passthrough_wraps_osc9() {
        let out = capture(Method::Osc9, true, "hello", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.starts_with("\x1bPtmux;"),
            "should start with DCS passthrough"
        );
        assert!(s.ends_with("\x1b\\"), "should end with ST");
        assert!(s.contains("hello"), "should contain message");
    }

    #[test]
    fn auto_detect_picks_osc9_for_iterm() {
        let _lock = env_lock();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "iTerm.app") };
        let resolved = resolve_method();
        // Restore previous value.
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
        assert_eq!(resolved, Method::Osc9);
    }

    /// Cmux in typical configurations does not set `TERM_PROGRAM`; it sets
    /// `LC_TERMINAL=Cmux` instead. Verify the `LC_TERMINAL` fallback probe
    /// correctly resolves to `Osc9`.
    #[test]
    fn auto_detect_picks_osc9_for_cmux_via_lc_terminal() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("LC_TERMINAL", "Cmux");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_lc {
                Some(v) => std::env::set_var("LC_TERMINAL", v),
                None => std::env::remove_var("LC_TERMINAL"),
            }
        }
        assert_eq!(resolved, Method::Osc9);
    }

    /// `LC_TERMINAL` should also match other OSC-9 capable terminals in case
    /// they set it in addition to or instead of `TERM_PROGRAM`.
    #[test]
    fn auto_detect_picks_osc9_for_wezterm_via_lc_terminal() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("LC_TERMINAL", "WezTerm");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_lc {
                Some(v) => std::env::set_var("LC_TERMINAL", v),
                None => std::env::remove_var("LC_TERMINAL"),
            }
        }
        assert_eq!(resolved, Method::Osc9);
    }

    #[test]
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn auto_detect_picks_bel_for_unknown_on_unix() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        let prev_term = std::env::var_os("TERM");
        // SAFETY: test-only; serialised by env_lock().
        // Clear LC_TERMINAL and TERM so the fallback probes don't
        // accidentally pick up an OSC-9 / Kitty / Ghostty capable
        // terminal from the test runner environment.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "xterm-256color");
            std::env::remove_var("LC_TERMINAL");
            std::env::set_var("TERM", "xterm-256color");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_lc {
                Some(v) => std::env::set_var("LC_TERMINAL", v),
                None => std::env::remove_var("LC_TERMINAL"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert_eq!(resolved, Method::Bel);
    }

    /// #2166: on Windows, an unknown TERM_PROGRAM resolves to `Bel` so
    /// `windows_bell()` can route the notification through `MessageBeep`.
    #[test]
    #[cfg(target_os = "windows")]
    fn auto_detect_picks_bel_for_unknown_on_windows() {
        let _lock = env_lock();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "Windows Terminal") };
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
        assert_eq!(resolved, Method::Bel);
    }

    /// #583: known OSC-9 terminals must still resolve to `Osc9` on
    /// Windows — the off-fallback only applies to unrecognised
    /// `TERM_PROGRAM`. The cross-platform iTerm test above is a thin
    /// proxy because iTerm itself only runs on macOS; if the WezTerm
    /// arm of the match silently disappeared, that test would still
    /// pass on the Windows runner and we'd lose the WezTerm-on-Windows
    /// compatibility guarantee. Pin it directly.
    #[test]
    #[cfg(target_os = "windows")]
    fn auto_detect_picks_osc9_for_wezterm_on_windows() {
        let _lock = env_lock();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "WezTerm") };
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
        assert_eq!(resolved, Method::Osc9);
    }

    /// Ghostty-based terminals (cmux, etc.) may not set
    /// `TERM_PROGRAM` but do set `TERM=xterm-ghostty`. The `$TERM`
    /// fallback should catch them.
    #[test]
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn auto_detect_picks_osc9_for_xterm_ghostty_term_fallback() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        let prev_term = std::env::var_os("TERM");
        // Simulate a Ghostty-based terminal that only sets TERM.
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("LC_TERMINAL");
            std::env::set_var("TERM", "xterm-ghostty");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_lc {
                Some(v) => std::env::set_var("LC_TERMINAL", v),
                None => std::env::remove_var("LC_TERMINAL"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert_eq!(resolved, Method::Osc9);
    }

    /// Ghostty now has its own protocol (OSC 777).
    #[test]
    fn auto_detect_picks_ghostty_from_term_program() {
        let _lock = env_lock();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "Ghostty") };
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
        assert_eq!(resolved, Method::Ghostty);
    }

    #[test]
    fn auto_detect_picks_kitty_from_term_program() {
        let _lock = env_lock();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "kitty") };
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
        assert_eq!(resolved, Method::Kitty);
    }

    #[test]
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn auto_detect_picks_kitty_from_term_fallback() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        let prev_term = std::env::var_os("TERM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("LC_TERMINAL");
            std::env::set_var("TERM", "xterm-kitty");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_lc {
                Some(v) => std::env::set_var("LC_TERMINAL", v),
                None => std::env::remove_var("LC_TERMINAL"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert_eq!(resolved, Method::Kitty);
    }

    /// When neither `TERM_PROGRAM` nor `TERM` suggests a known capable
    /// terminal, the fallback on Unix is `Bel`.
    ///
    /// On macOS the `MacOS` method takes priority, so this test is
    /// excluded there.
    #[test]
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn auto_detect_falls_back_to_bel_for_unrelated_term() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        let prev_term = std::env::var_os("TERM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("LC_TERMINAL");
            std::env::set_var("TERM", "xterm-256color");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_tp {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_lc {
                Some(v) => std::env::set_var("LC_TERMINAL", v),
                None => std::env::remove_var("LC_TERMINAL"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert_eq!(resolved, Method::Bel);
    }

    #[test]
    fn humanize_duration_seconds_and_minutes() {
        assert_eq!(humanize_duration(Duration::from_secs(0)), "0s");
        assert_eq!(humanize_duration(Duration::from_secs(45)), "45s");
        assert_eq!(humanize_duration(Duration::from_secs(60)), "1m");
        assert_eq!(humanize_duration(Duration::from_secs(72)), "1m 12s");
        // 59m 59s — still under the hour boundary.
        assert_eq!(humanize_duration(Duration::from_secs(3599)), "59m 59s");
    }

    #[test]
    fn humanize_duration_promotes_to_hours_at_one_hour() {
        // 3661s = 1h 1m 1s — under the new format the seconds fall
        // off; we keep just the top two units at the hour mark.
        assert_eq!(humanize_duration(Duration::from_secs(3661)), "1h 1m");
        assert_eq!(humanize_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(humanize_duration(Duration::from_secs(7200)), "2h");
        assert_eq!(humanize_duration(Duration::from_secs(7320)), "2h 2m");
        // 3h 12m — the previous "192m 30s" case that motivated #447.
        assert_eq!(humanize_duration(Duration::from_secs(11_550)), "3h 12m");
    }

    #[test]
    fn humanize_duration_handles_multi_day_sessions() {
        // Exactly one day.
        assert_eq!(humanize_duration(Duration::from_secs(86_400)), "1d");
        // 1d 1h.
        assert_eq!(humanize_duration(Duration::from_secs(90_000)), "1d 1h");
        // 2d 5h — the two-tier rule drops minutes/seconds.
        assert_eq!(
            humanize_duration(Duration::from_secs(2 * 86_400 + 5 * 3600 + 17 * 60)),
            "2d 5h"
        );
    }

    #[test]
    fn humanize_duration_promotes_to_weeks_after_seven_days() {
        assert_eq!(humanize_duration(Duration::from_secs(604_800)), "1w");
        assert_eq!(
            humanize_duration(Duration::from_secs(604_800 + 86_400)),
            "1w 1d"
        );
        // 3w 2d — long-running automation case.
        assert_eq!(
            humanize_duration(Duration::from_secs(3 * 604_800 + 2 * 86_400 + 17 * 3600)),
            "3w 2d"
        );
    }
}
