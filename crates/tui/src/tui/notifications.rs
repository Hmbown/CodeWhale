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
//!
//! Every mechanism is fed a [`NotificationPayload`] — a typed, bounded,
//! redaction-aware value — rather than a free-form `String` (#4834). See
//! [`crate::tui::notification_payload`] for the per-kind disclosure
//! policy.

#[cfg(target_os = "windows")]
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE;

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::{AtomicU8, AtomicU64};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub use super::notification_payload::NotificationPayload;

#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;

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
    /// macOS Notification Center via `osascript`.
    ///
    /// Only reachable through [`Method::Auto`], and only on the macOS
    /// terminals that expose no notification escape of their own (Apple
    /// Terminal, the VS Code and JetBrains embedded terminals, plain tmux
    /// without `LC_TERMINAL`). iTerm2, WezTerm, Ghostty, and kitty are
    /// matched earlier in [`resolve_method`] and never get here.
    ///
    /// Known limitation (#4834): `display notification` is a Standard
    /// Additions command, so the banner is attributed to the *bundled*
    /// host process. `/usr/bin/osascript` is unbundled, so macOS credits
    /// `com.apple.ScriptEditor2` — which is what supplies the Script
    /// Editor icon and owns the System Settings → Notifications entry
    /// (alert style, previews, Do Not Disturb). `display notification`
    /// takes no icon parameter; fixing the attribution requires shipping
    /// a real `.app` bundle, not a change in this file.
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

/// Emit a notification to `sink` if the elapsed time meets or exceeds
/// `threshold`, and `method` is not `Off`.
///
/// This variant takes a `W: Write` sink for testability.
pub fn notify_done_to<W: Write>(
    method: Method,
    in_tmux: bool,
    payload: &NotificationPayload,
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

    // "I get no notifications" and "the wrong app posted it" (#4834) are
    // both diagnosed by knowing which kind resolved to which mechanism.
    tracing::debug!(
        kind = ?payload.kind(),
        method = ?effective,
        in_tmux,
        "emitting desktop notification"
    );

    // macOS Notification Center: handled via osascript, not terminal escapes.
    #[cfg(target_os = "macos")]
    if Method::MacOS == effective {
        macos_display_notification(payload);
        return;
    }

    let bytes = build_escape(effective, in_tmux, &payload.render_inline());
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

/// Emit a notification to **stdout** if `elapsed >= threshold`.
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
    payload: &NotificationPayload,
    threshold: Duration,
    elapsed: Duration,
) {
    notify_done_to(
        method,
        in_tmux,
        payload,
        threshold,
        elapsed,
        &mut io::stdout(),
    );
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
/// Build the OSC 9;4 taskbar-progress sequence. Split from the write so the
/// bytes can be asserted without depending on whether the test runner owns a
/// terminal.
#[must_use]
fn taskbar_progress_sequence(state: u8, progress: Option<u8>) -> String {
    match progress {
        Some(pct) => format!("\x1b]9;4;{state};{pct}\x07"),
        None => format!("\x1b]9;4;{state}\x07"),
    }
}

/// Build the OSC 0 window-title sequence. Split from the write for the same
/// reason as [`taskbar_progress_sequence`].
#[must_use]
fn terminal_title_sequence(title: &str) -> String {
    format!("\x1b]0;{title}\x07")
}

/// Whether raw terminal control sequences may be written to stdout.
///
/// OSC 9;4 (taskbar progress) and OSC 0 (window title) are *control* bytes,
/// not content. A terminal that understands them renders nothing visible; a
/// pipe, a file, or a CI log renders them literally, so `cargo test` output
/// and redirected sessions pick up stray `]9;4;1]0;` noise. Gate on stdout
/// actually being a TTY — there is no one to control otherwise.
fn stdout_accepts_control_sequences() -> bool {
    use std::io::IsTerminal;
    io::stdout().is_terminal()
}

pub fn set_taskbar_progress(state: u8, progress: Option<u8>) {
    if !stdout_accepts_control_sequences() {
        return;
    }
    let seq = taskbar_progress_sequence(state, progress);
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

/// Shared flag controlling the title activity marker. Set to `true` by
/// `start_title_animation()`, cleared by `stop_title_animation()`.
static TITLE_ANIMATION_RUNNING: AtomicBool = AtomicBool::new(false);
/// Focus reporting starts enabled before the event loop begins, so treating
/// the terminal as focused is the safe default: never flood window chrome
/// unless the terminal has explicitly reported `FocusLost` or motion is on.
static TERMINAL_FOCUSED: AtomicBool = AtomicBool::new(true);
/// When false, the title keeps a static whale + state (reduced motion /
/// status animation off) instead of cycling frames.
static TITLE_MOTION_ENABLED: AtomicBool = AtomicBool::new(true);
/// Invalidates a previous animation worker when a new turn starts or ends.
static TITLE_ANIMATION_GENERATION: AtomicU64 = AtomicU64::new(0);
static TITLE_ANIMATION_BASE: OnceLock<Mutex<String>> = OnceLock::new();
static TITLE_ACTIVITY_VERB: OnceLock<Mutex<String>> = OnceLock::new();
/// Whale frames restored from #1871 (`cd357de0c`). Cycle slowly so the
/// terminal title communicates life without competing with in-app spinners.
const TITLE_FRAME_HOLD: Duration = Duration::from_millis(800);
const TITLE_WHALE_FRAMES: &[&str] = &["🐳", "🐋", "🐳", "🐋"];

fn title_animation_base() -> &'static Mutex<String> {
    TITLE_ANIMATION_BASE.get_or_init(|| Mutex::new("Codewhale".to_string()))
}

fn title_activity_verb() -> &'static Mutex<String> {
    TITLE_ACTIVITY_VERB.get_or_init(|| Mutex::new("working…".to_string()))
}

/// Configure whether the title whale cycles frames.
///
/// Call once at startup (and whenever motion settings change). Reduced motion
/// and `status_indicator = "off"` both freeze the title to a single whale.
pub fn set_title_motion_enabled(enabled: bool) {
    TITLE_MOTION_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Update the truthful activity verb shown next to the title whale
/// (`working…`, `reasoning…`, `using tool…`, `verifying…`, `waiting on you…`).
pub fn set_title_activity_verb(verb: &str) {
    let verb = verb.trim();
    if verb.is_empty() {
        return;
    }
    if let Ok(mut slot) = title_activity_verb().lock() {
        if slot.as_str() == verb {
            return;
        }
        verb.clone_into(&mut *slot);
    }
    if !TITLE_ANIMATION_RUNNING.load(Ordering::SeqCst) {
        return;
    }
    let base = title_animation_base()
        .lock()
        .map_or_else(|_| "Codewhale".to_string(), |base| base.clone());
    set_terminal_title(&title_activity_label(
        &base,
        Duration::ZERO,
        TERMINAL_FOCUSED.load(Ordering::SeqCst),
        TITLE_MOTION_ENABLED.load(Ordering::SeqCst),
    ));
}

#[must_use]
fn title_activity_label(base: &str, elapsed: Duration, focused: bool, motion: bool) -> String {
    let verb = title_activity_verb()
        .lock()
        .map_or_else(|_| "working…".to_string(), |v| v.clone());
    let body = if verb.is_empty() {
        base.to_string()
    } else {
        verb
    };
    // Static title when motion is off or the window is focused: one whale +
    // state, no competing spinner in the focused app chrome.
    if !motion || focused {
        return format!("🐳 {body}");
    }
    let frame = TITLE_WHALE_FRAMES
        [(elapsed.as_millis() / TITLE_FRAME_HOLD.as_millis()) as usize % TITLE_WHALE_FRAMES.len()];
    format!("{frame} {body}")
}

/// Write OSC 0 (set window title) sequence.
fn set_terminal_title(title: &str) {
    if !stdout_accepts_control_sequences() {
        return;
    }
    let seq = terminal_title_sequence(title);
    let mut stdout = io::stdout();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();
}

/// Tracks whether the completion marker was set, so
/// `reset_title_on_interaction()` can skip redundant writes.
static COMPLETION_MARKER_SHOWN: AtomicBool = AtomicBool::new(false);

/// Mark the terminal title as active with the animated whale + state verb.
///
/// While focused (or under reduced motion), the title stays a static whale
/// with the current verb. After `FocusLost` with motion enabled, the whale
/// frames cycle so alt-tabbed sessions still communicate progress.
pub fn start_title_animation(original: &str) {
    if let Ok(mut base) = title_animation_base().lock() {
        original.clone_into(&mut base);
    }
    if let Ok(mut verb) = title_activity_verb().lock()
        && verb.is_empty()
    {
        "working…".clone_into(&mut *verb);
    }
    COMPLETION_MARKER_SHOWN.store(false, Ordering::SeqCst);
    TITLE_ANIMATION_RUNNING.store(true, Ordering::SeqCst);
    let generation = TITLE_ANIMATION_GENERATION
        .fetch_add(1, Ordering::SeqCst)
        .saturating_add(1);
    let focused = TERMINAL_FOCUSED.load(Ordering::SeqCst);
    let motion = TITLE_MOTION_ENABLED.load(Ordering::SeqCst);
    set_terminal_title(&title_activity_label(
        original,
        Duration::ZERO,
        focused,
        motion,
    ));

    let base = original.to_string();
    std::thread::spawn(move || {
        let started_at = std::time::Instant::now();
        loop {
            std::thread::sleep(TITLE_FRAME_HOLD);
            if !TITLE_ANIMATION_RUNNING.load(Ordering::SeqCst)
                || TITLE_ANIMATION_GENERATION.load(Ordering::SeqCst) != generation
            {
                break;
            }
            let motion = TITLE_MOTION_ENABLED.load(Ordering::SeqCst);
            // Only advance frames when unfocused + motion is on. Focused
            // windows keep the static whale so the title is not a second
            // spinner competing with in-app activity chrome.
            if motion && !TERMINAL_FOCUSED.load(Ordering::SeqCst) {
                set_terminal_title(&title_activity_label(
                    &base,
                    started_at.elapsed(),
                    false,
                    true,
                ));
            }
        }
    });
}

/// Update the focus gate used by the title activity signal.
///
/// Focus gain immediately restores the steady whale + verb. Focus loss emits
/// the first animation frame immediately, then the worker advances it at the
/// debounced whale cadence.
pub fn set_terminal_focused(focused: bool) {
    TERMINAL_FOCUSED.store(focused, Ordering::SeqCst);
    if !TITLE_ANIMATION_RUNNING.load(Ordering::SeqCst) {
        return;
    }
    let base = title_animation_base()
        .lock()
        .map_or_else(|_| "Codewhale".to_string(), |base| base.clone());
    let motion = TITLE_MOTION_ENABLED.load(Ordering::SeqCst);
    set_terminal_title(&title_activity_label(
        &base,
        Duration::ZERO,
        focused,
        motion,
    ));
}

/// Stop the title animation and show a completion marker.
///
/// Sets the title to `✓ done` so alt-tabbed users see at a glance that
/// processing finished. The marker is overwritten on the next turn by
/// [`start_title_animation`].
pub fn stop_title_animation() {
    TITLE_ANIMATION_RUNNING.store(false, Ordering::SeqCst);
    TITLE_ANIMATION_GENERATION.fetch_add(1, Ordering::SeqCst);
    // Always show the completion marker so quiet-sound modes still communicate
    // finish state in the window title; interaction clears it.
    COMPLETION_MARKER_SHOWN.store(true, Ordering::SeqCst);
    set_terminal_title("✓ done");
    play_completion_sound();
}

/// Stop the title animation without playing the completion sound.
///
/// Cancellation and failed turns should return the terminal title to rest
/// without presenting them as completed work.
pub fn stop_title_animation_quietly() {
    TITLE_ANIMATION_RUNNING.store(false, Ordering::SeqCst);
    TITLE_ANIMATION_GENERATION.fetch_add(1, Ordering::SeqCst);
    COMPLETION_MARKER_SHOWN.store(false, Ordering::SeqCst);
    set_terminal_title("Codewhale");
}

/// Clear the completion marker from the title when the user interacts.
///
/// Call this on every user input event (key press, mouse click) so the
/// marker doesn't persist once the user is back at the terminal.
pub fn reset_title_on_interaction() {
    if COMPLETION_MARKER_SHOWN.swap(false, Ordering::SeqCst) {
        set_terminal_title("Codewhale");
    }
}

/// Completion sound mode (0 = off, 1 = beep, 2 = bell, 3 = file).
static COMPLETION_SOUND_MODE: AtomicU8 = AtomicU8::new(1);
static COMPLETION_SOUND_FILE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
#[cfg(not(target_os = "windows"))]
static COMPLETION_SOUND_FILE_UNSUPPORTED_WARNED: AtomicBool = AtomicBool::new(false);
static COMPLETION_SOUND_FILE_MISSING_WARNED: AtomicBool = AtomicBool::new(false);

fn completion_sound_file_slot() -> &'static Mutex<Option<PathBuf>> {
    COMPLETION_SOUND_FILE.get_or_init(|| Mutex::new(None))
}

fn set_completion_sound(mode: crate::config::CompletionSound, sound_file: Option<PathBuf>) {
    let val = match mode {
        crate::config::CompletionSound::Off => 0u8,
        crate::config::CompletionSound::Beep => 1u8,
        crate::config::CompletionSound::Bell => 2u8,
        crate::config::CompletionSound::File => 3u8,
    };
    COMPLETION_SOUND_MODE.store(val, Ordering::SeqCst);
    if let Ok(mut slot) = completion_sound_file_slot().lock() {
        if sound_file.is_some() {
            COMPLETION_SOUND_FILE_MISSING_WARNED.store(false, Ordering::SeqCst);
        }
        *slot = sound_file;
    }
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
        3 => {
            file_sound();
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

fn configured_sound_file() -> Option<PathBuf> {
    completion_sound_file_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
}

#[cfg(target_os = "windows")]
fn play_sound_file(path: &Path) {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // Best-effort and async: notification sound failure should not block or
    // fail a completed agent turn.
    unsafe {
        let _ = PlaySoundW(
            PCWSTR(wide.as_ptr()),
            None,
            SND_FILENAME | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn play_sound_file(_path: &Path) {
    if !COMPLETION_SOUND_FILE_UNSUPPORTED_WARNED.swap(true, Ordering::SeqCst) {
        tracing::warn!("completion_sound = \"file\" is currently supported on Windows only");
    }
}

fn file_sound() {
    if let Some(path) = configured_sound_file() {
        play_sound_file(&path);
    } else if !COMPLETION_SOUND_FILE_MISSING_WARNED.swap(true, Ordering::SeqCst) {
        tracing::warn!("completion_sound = \"file\" requires [notifications].sound_file");
    }
}

#[cfg(test)]
fn completion_sound_state_for_tests() -> (crate::config::CompletionSound, Option<PathBuf>) {
    let mode = match COMPLETION_SOUND_MODE.load(Ordering::SeqCst) {
        0 => crate::config::CompletionSound::Off,
        1 => crate::config::CompletionSound::Beep,
        2 => crate::config::CompletionSound::Bell,
        3 => crate::config::CompletionSound::File,
        _ => crate::config::CompletionSound::Off,
    };
    (mode, configured_sound_file())
}

/// Show a macOS Notification Center alert via `osascript`.
///
/// Runs on a dedicated background thread so the caller is not blocked.
///
/// The notification includes:
/// - **Title**: "Codewhale"
/// - **Subtitle**: [`NotificationPayload::headline`] (≤ 80 chars)
/// - **Body**: [`NotificationPayload::body`] (≤ 322 chars: a ≤ 120-char
///   detail, a separator, and a ≤ 200-char preview)
/// - **Sound**: Default macOS notification sound
///
/// Both fields arrive already sanitized, redacted, and character-bounded
/// by [`NotificationPayload`]; this function does not re-derive them from
/// free-form text (#4834).
///
/// **Security**: The message is passed to `osascript` as a command-line
/// argument via `ARGV`, never embedded inline in the AppleScript source.
/// AppleScript does not treat backslash as an escape inside double-quoted
/// string literals, so the previous `\"` approach would terminate the
/// string at the `"` and leave any text between unbalanced quotes
/// evaluated as raw AppleScript code — a code-injection vector for
/// AI-generated notification text. Passing via `ARGV` avoids this
/// entirely because the message is never parsed as AppleScript syntax.
/// Keep it that way.
///
/// **Attribution**: the banner is posted on behalf of `osascript`, which
/// is unbundled, so macOS attributes it to `com.apple.ScriptEditor2`. See
/// [`Method::MacOS`] — that is not fixable from here.
///
/// This is best-effort: if `osascript` is not available (e.g. headless SSH
/// session) the error is logged via `tracing::warn!` instead of silently
/// swallowed.
#[cfg(target_os = "macos")]
fn macos_display_notification(payload: &NotificationPayload) {
    let (subtitle, body) = macos_notification_parts(payload);

    // Spawn on a background thread so we don't block the caller.
    // osascript itself is fast (~50 ms), but spawning a subprocess
    // synchronously from an async context steals a tokio thread.
    let _ = std::thread::Builder::new()
        .name("osascript-notif".into())
        .spawn(move || {
            // Build AppleScript that receives the message via ARGV
            // instead of inline string interpolation. AppleScript does
            // not treat backslash as an escape inside double-quoted
            // string literals, so `\"` would terminate the string at
            // the `"` and leave a dangling `\`. Passing the message as
            // a command-line argument avoids any injection risk.
            let args = [
                "-e".to_string(),
                "on run argv".to_string(),
                "-e".to_string(),
                "set theBody to item 1 of argv".to_string(),
                "-e".to_string(),
                "set theSubtitle to item 2 of argv".to_string(),
                "-e".to_string(),
                "display notification theBody with title \"Codewhale\" subtitle theSubtitle sound name \"default\"".to_string(),
                "-e".to_string(),
                "end run".to_string(),
                "--".to_string(),
                body,
                subtitle,
            ];

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

/// Split a payload into the `(subtitle, body)` pair `display notification`
/// wants. Both halves are already bounded and redacted by the payload
/// constructors, so this is a projection, not a sanitizer.
#[cfg(target_os = "macos")]
fn macos_notification_parts(payload: &NotificationPayload) -> (String, String) {
    (payload.headline().to_string(), payload.body())
}

// ── Per-turn notification composition ────────────────────────────────
//
// The helpers below decide *whether* to notify on a completed turn and
// *what message* to put in the body. The low-level dispatcher is
// `notify_done`; everything in this block sits in front of it.

use crate::localization::{Locale, MessageId, tr};
use crate::models::{ContentBlock, Message};
use crate::tools::subagent::SubAgentStatus;
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
    set_completion_sound(notif.completion_sound, notif.sound_file);
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

/// Build the notification payload for a completed turn. Prefers the live
/// streaming text the user just saw; falls back to the latest assistant
/// message in `api_messages` if streaming text is empty (for example, the
/// turn finished entirely through tool output). When `include_summary` is
/// true, an elapsed/cost suffix is appended to the headline.
///
/// The assistant text becomes the payload's *preview*, which means it is
/// redacted and capped at 200 characters before it can reach the OS.
pub fn completed_turn_payload(
    app: &App,
    current_streaming_text: &str,
    include_summary: bool,
    turn_elapsed: Duration,
    turn_cost: Option<crate::pricing::CostEstimate>,
) -> NotificationPayload {
    let headline = completion_status(
        &tr(app.ui_locale, MessageId::NotificationTurnComplete),
        include_summary,
        turn_elapsed,
        turn_cost.map(|cost| crate::pricing::format_cost_estimate(cost, app.cost_currency)),
    );

    let preview =
        text_summary(current_streaming_text).or_else(|| latest_assistant_text(&app.api_messages));

    NotificationPayload::turn_complete(&headline).with_preview(preview.as_deref())
}

/// Compose a notification payload for a terminal sub-agent outcome. The
/// agent id is always the detail line; the child's first human-readable
/// summary line, when there is one, becomes the (redacted, bounded)
/// preview. The headline reflects the actual status so a Stop/failed
/// worker is never announced as successfully complete (#4408).
pub fn subagent_terminal_payload(
    locale: Locale,
    id: &str,
    result: &str,
    status: &SubAgentStatus,
    include_summary: bool,
    elapsed: Duration,
) -> NotificationPayload {
    let result_line = result
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("<codewhale:subagent.done>"));
    let label = match status {
        SubAgentStatus::Completed => MessageId::NotificationSubagentComplete,
        SubAgentStatus::Failed(_) => MessageId::NotificationSubagentFailed,
        SubAgentStatus::Interrupted(_) => MessageId::NotificationSubagentInterrupted,
        SubAgentStatus::Cancelled => MessageId::NotificationSubagentCancelled,
        SubAgentStatus::BudgetExhausted => MessageId::NotificationSubagentBudgetExhausted,
        SubAgentStatus::Running => MessageId::NotificationSubagentComplete,
    };
    let headline = completion_status(&tr(locale, label), include_summary, elapsed, None);
    let preview = result_line.and_then(text_summary);

    NotificationPayload::subagent_terminal(&headline, id).with_preview(preview.as_deref())
}

fn completion_status(
    label: &str,
    include_summary: bool,
    elapsed: Duration,
    cost: Option<String>,
) -> String {
    if !include_summary {
        return label.to_string();
    }

    let human = crate::elapsed::format_elapsed_secs(elapsed.as_secs());
    match cost {
        Some(cost) => format!("{label} ({human}, {cost})"),
        None => format!("{label} ({human})"),
    }
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
                    ContentBlock::ImageUrl { .. } => None,
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

    #[test]
    fn title_whale_is_static_when_focused_or_motion_disabled() {
        if let Ok(mut verb) = title_activity_verb().lock() {
            "working…".clone_into(&mut *verb);
        }
        assert_eq!(
            title_activity_label("Codewhale", Duration::ZERO, true, true),
            "🐳 working…"
        );
        assert_eq!(
            title_activity_label("Codewhale", Duration::ZERO, false, false),
            "🐳 working…"
        );
        assert_eq!(
            title_activity_label("Codewhale", Duration::ZERO, false, true),
            "🐳 working…"
        );
        assert_eq!(
            title_activity_label("Codewhale", Duration::from_millis(800), false, true),
            "🐋 working…"
        );
    }

    #[test]
    fn title_whale_frames_are_the_restored_emoji_pair() {
        assert_eq!(TITLE_WHALE_FRAMES, &["🐳", "🐋", "🐳", "🐋"]);
        assert_eq!(TITLE_FRAME_HOLD, Duration::from_millis(800));
    }

    /// Serialise tests that mutate process-global environment or notification
    /// sound state while the test harness runs them in parallel threads.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Escape-protocol tests care about the bytes, not the composition
    /// policy, so they go through the least-privileged constructor.
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
            &NotificationPayload::input_needed(msg),
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

    /// #4847 follow-up: OSC 9;4 and OSC 0 are *control* bytes, not content.
    /// A terminal renders nothing visible; a pipe, a file, or a CI log renders
    /// them literally — which is why `cargo test` output carried stray
    /// `]9;4;1]0;` noise. The write is now gated on stdout being a TTY; these
    /// assertions pin the bytes themselves so the gate cannot be "fixed" by
    /// quietly changing what gets emitted.
    #[test]
    fn control_sequences_have_the_exact_documented_bytes() {
        assert_eq!(taskbar_progress_sequence(1, None), "\x1b]9;4;1\x07");
        assert_eq!(taskbar_progress_sequence(1, Some(42)), "\x1b]9;4;1;42\x07");
        assert_eq!(taskbar_progress_sequence(0, None), "\x1b]9;4;0\x07");
        assert_eq!(
            terminal_title_sequence("🐳 working…"),
            "\x1b]0;🐳 working…\x07"
        );
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

    /// The subtitle is the localized status headline and the body is
    /// everything else. Previously this was re-derived by splitting a
    /// free-form string on its first newline; now it is a projection of
    /// the typed payload, so the split cannot drift from what the
    /// composer intended (#4834).
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_keeps_localized_status_as_subtitle() {
        let payload = NotificationPayload::turn_complete("ターン完了 (1m 5s)")
            .with_preview(Some("完了しました。"));

        let (subtitle, body) = macos_notification_parts(&payload);

        assert_eq!(subtitle, "ターン完了 (1m 5s)");
        assert_eq!(body, "完了しました。");
    }

    /// The preview is capped at `PREVIEW_MAX_CHARS` *inclusive* of the
    /// ellipsis, so the string handed to `osascript` never exceeds the
    /// declared bound.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_notification_truncates_preview() {
        let payload = NotificationPayload::turn_complete("Turn complete")
            .with_preview(Some(&"assistant preview ".repeat(40)));

        let (subtitle, body) = macos_notification_parts(&payload);

        assert_eq!(subtitle, "Turn complete");
        assert!(body.starts_with("assistant preview"));
        assert!(body.ends_with("..."));
        assert_eq!(
            body.chars().count(),
            super::super::notification_payload::PREVIEW_MAX_CHARS
        );
    }

    /// #4834: an approval banner is the one place a raw shell command
    /// used to reach Notification Center. Pin the macOS projection, not
    /// just the payload, so a future refactor of either half is caught.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_approval_notification_never_carries_the_command() {
        let payload = NotificationPayload::approval_needed("Approval needed", "bash");

        let (subtitle, body) = macos_notification_parts(&payload);

        assert_eq!(subtitle, "Approval needed");
        assert_eq!(body, "bash");
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
    fn settings_installs_custom_completion_sound_file() {
        let _lock = env_lock();
        let config: crate::config::Config = toml::from_str(
            r#"
            [notifications]
            completion_sound = "file"
            sound_file = "E:\\google\\downloads\\xm4114.wav"
            "#,
        )
        .expect("custom completion sound config should parse");

        let _ = settings(&config);

        let (mode, file) = completion_sound_state_for_tests();
        assert_eq!(mode, crate::config::CompletionSound::File);
        assert_eq!(
            file.as_deref(),
            Some(std::path::Path::new("E:\\google\\downloads\\xm4114.wav"))
        );
    }

    #[test]
    fn setting_valid_sound_file_resets_missing_file_warning_latch() {
        let _lock = env_lock();
        COMPLETION_SOUND_FILE_MISSING_WARNED.store(true, Ordering::SeqCst);

        set_completion_sound(
            crate::config::CompletionSound::File,
            Some(std::path::PathBuf::from(
                "E:\\google\\downloads\\xm4114.wav",
            )),
        );

        assert!(!COMPLETION_SOUND_FILE_MISSING_WARNED.load(Ordering::SeqCst));

        set_completion_sound(crate::config::CompletionSound::File, None);
        file_sound();

        assert!(COMPLETION_SOUND_FILE_MISSING_WARNED.load(Ordering::SeqCst));

        set_completion_sound(crate::config::CompletionSound::Beep, None);
        COMPLETION_SOUND_FILE_MISSING_WARNED.store(false, Ordering::SeqCst);
    }
}
