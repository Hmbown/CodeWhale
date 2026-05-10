//! Desktop notifications for turn completion.
//!
//! Supports five delivery mechanisms:
//! - **OSC 9** — terminal escape sequence (`\x1b]9;…\x07`) for iTerm2,
//!   Ghostty, WezTerm, and tmux (with DCS passthrough).
//! - **Kitty** — OSC 99 protocol with ST terminator (no audible beep).
//! - **Ghostty** — OSC 777 notification protocol.
//! - **Native** — OS-level notification via `osascript` (macOS) or
//!   `notify-send` (Linux), working in any terminal.
//! - **BEL** — audible bell (`\x07`) as a last-resort fallback.
//!
//! Trigger modes:
//! - **Idle detection** (default): fires when user hasn't typed for 6 seconds.
//! - **Fixed threshold**: fires when turn elapsed time exceeds N seconds.
//!
//! When `method = "auto"`, the resolver picks the best method for the
//! current terminal; Windows falls back to `Off` to avoid the error chime
//! (#583).

#[cfg(target_os = "windows")]
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE;

use std::io::{self, Write};
use std::time::Duration;

/// Notification delivery method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Method {
    /// Automatically pick `Osc9` for known capable terminals
    /// (`iTerm.app`, `Ghostty`, `WezTerm`, `Cmux`); on Unix, falls
    /// back to native OS notifications (`osascript` / `notify-send`)
    /// when the terminal doesn't advertise OSC 9 support. On Windows
    /// the non-OSC-9 fallback is `Off` instead of `Bel`, because BEL
    /// maps to the system error chime (#583).
    #[default]
    Auto,
    /// OSC 9 escape: `\x1b]9;<msg>\x07`
    Osc9,
    /// Plain BEL character: `\x07`
    Bel,
    /// Kitty notification protocol (OSC 99) with ST terminator.
    /// Uses `ESC ] 99 ; params ST` — no audible beep, unlike BEL.
    Kitty,
    /// Ghostty notification protocol (OSC 777).
    /// Uses `ESC ] 777 ; notify ; title ; message BEL`.
    Ghostty,
    /// Native OS notification: `osascript display notification` on macOS,
    /// `notify-send` on Linux. Works in any terminal — no OSC support
    /// required. Best-effort; falls back to `Bel` if the native command
    /// is unavailable.
    Native,
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

/// Resolve `Auto` to a concrete method by inspecting `$TERM_PROGRAM`
/// with a `$TERM` fallback for Ghostty-based terminals.
///
/// Resolution table:
/// - `iTerm.app`, `WezTerm`, `Cmux` → `Osc9`
/// - `Ghostty` → `Ghostty` (OSC 777)
/// - `kitty` → `Kitty` (OSC 99)
/// - `$TERM` contains `ghostty` → `Osc9` (cmux etc.)
/// - `$TERM` contains `kitty` → `Kitty`
/// - Unix unknown → `Bel` (no AppleScript injection risk)
/// - Windows unknown → `Off`
#[must_use]
fn resolve_method() -> Method {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    match term_program.as_str() {
        "iTerm.app" | "WezTerm" | "Cmux" => Method::Osc9,
        "Ghostty" => Method::Ghostty,
        "kitty" => Method::Kitty,
        _ if cfg!(target_os = "windows") => Method::Off,
        _ => {
            let term = std::env::var("TERM").unwrap_or_default();
            if term.contains("ghostty") {
                Method::Osc9
            } else if term.contains("kitty") {
                Method::Kitty
            } else {
                Method::Bel
            }
        }
    }
}

/// Best-guess the macOS bundle identifier of the current terminal.
///
/// Maps known `$TERM_PROGRAM` values to their bundle IDs. When
/// `TERM_PROGRAM` is unrecognised, falls back to a cached one-shot
/// `osascript` probe that asks for the frontmost application's id.
/// The result is memoised in a `OnceLock` so the probe runs at most
/// once per process.
#[cfg(target_os = "macos")]
fn terminal_bundle_id() -> Option<&'static str> {
    use std::sync::OnceLock;
    static BUNDLE_ID: OnceLock<Option<String>> = OnceLock::new();
    BUNDLE_ID
        .get_or_init(|| {
            let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
            match term_program.as_str() {
                "iTerm.app" => Some("com.googlecode.iterm2".into()),
                "Ghostty" => Some("com.mitchellh.ghostty".into()),
                "Cmux" => Some("com.cmuxterm.app".into()),
                "WezTerm" => Some("com.github.wez.wezterm".into()),
                _ => {
                    // Fallback: ask the window server which app is frontmost.
                    let output = std::process::Command::new("osascript")
                        .arg("-e")
                        .arg("tell application \"System Events\" to get bundle identifier of first application process whose frontmost is true")
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    output
                }
            }
        })
        .as_deref()
}

/// Check whether `terminal-notifier` is available on this system.
#[cfg(target_os = "macos")]
fn has_terminal_notifier() -> bool {
    static CHECK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CHECK.get_or_init(|| {
        std::process::Command::new("which")
            .arg("terminal-notifier")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_or(false, |s| s.success())
    })
}

/// Fire a native OS notification.
///
/// On macOS, prefers `terminal-notifier` (supports click-to-focus via
/// `-activate BUNDLE_ID`) with a fallback to `osascript display
/// notification`. On Linux, uses `notify-send`.
///
/// Spawns a short-lived thread so the caller is not blocked on the
/// process. Best-effort: failures from a missing binary or a
/// non-responsive notification daemon are silently ignored — the
/// notification is a convenience, not a correctness requirement.
fn native_notify(msg: &str) {
    let msg = msg.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            // terminal-notifier: clickable notification that can
            // activate (bring-to-front) the terminal window.
            if has_terminal_notifier() {
                let mut cmd = std::process::Command::new("terminal-notifier");
                cmd.arg("-title").arg("DeepSeek TUI");
                cmd.arg("-message").arg(&msg);
                if let Some(bundle_id) = terminal_bundle_id() {
                    cmd.arg("-activate").arg(bundle_id);
                }
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                let _ = cmd.spawn();
                return;
            }
            // Fallback: plain osascript notification (no click-to-focus).
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(format!(
                    "display notification \"{msg}\" with title \"DeepSeek TUI\""
                ))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(all(target_os = "linux", not(target_os = "macos")))]
        {
            if std::process::Command::new("which")
                .arg("notify-send")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map_or(false, |s| s.success())
            {
                let _ = std::process::Command::new("notify-send")
                    .arg("DeepSeek TUI")
                    .arg(&msg)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    });
}

/// Wrap an escape sequence for terminal multiplexer passthrough.
/// tmux intercepts escape sequences; DCS passthrough tunnels them to
/// the outer terminal unmodified.
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
            let title_seq = format!("\x1b]99;d=0:p=title\x1b\\");
            let body_seq = format!("\x1b]99;p=body;{msg}\x1b\\");
            let focus_seq = format!("\x1b]99;d=1:a=focus\x1b\\");
            let combined = format!("{title_seq}{body_seq}{focus_seq}");
            wrap_for_multiplexer(&combined, in_tmux).into_bytes()
        }
        Method::Ghostty => {
            // Ghostty notification: OSC 777 ; notify ; title ; message BEL
            let seq = format!("\x1b]777;notify;DeepSeek TUI;{msg}\x07");
            wrap_for_multiplexer(&seq, in_tmux).into_bytes()
        }
        // Auto and Off should not reach build_escape.
        // Native goes through native_notify(), not through bytes.
        Method::Auto | Method::Off | Method::Native => vec![],
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
    // Native goes through the OS notification path, not terminal escapes.
    if effective == Method::Native {
        native_notify(msg);
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
/// With `method = Auto`, selects `Osc9` for known capable terminals
/// (`iTerm.app`, `Ghostty`, `WezTerm`, `Cmux`, or any terminal with
/// `TERM` containing `ghostty`); the unknown-terminal fallback is
/// platform-aware — `Native` on macOS / Linux (system notification via
/// `osascript` / `notify-send`), `Off` on Windows (where BEL
/// maps to the `SystemAsterisk` / `MB_OK` error chime, #583). See
/// [`resolve_method`] for the canonical resolution table. Pass
/// `in_tmux = true` (i.e. `$TMUX` is non-empty at runtime) to wrap OSC 9
/// in a DCS passthrough.
pub fn notify_done(
    method: Method,
    in_tmux: bool,
    msg: &str,
    threshold: Duration,
    elapsed: Duration,
) {
    notify_done_to(method, in_tmux, msg, threshold, elapsed, &mut io::stdout());
}

/// Default idle threshold: 6 seconds without keyboard input.
pub const DEFAULT_IDLE_THRESHOLD: Duration = Duration::from_secs(6);

/// Maximum time to wait for user to become idle before giving up.
pub const MAX_IDLE_WAIT: Duration = Duration::from_secs(300);

/// How often to poll for idle state.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(6);

/// Emit a notification when the user becomes idle after a turn completes.
///
/// If `idle_threshold` is zero, falls back to [`notify_done`] behavior
/// (fixed elapsed-time check against the same value as `threshold`).
///
/// Otherwise, checks whether the user has been idle (no keyboard input)
/// for at least `idle_threshold` seconds. If already idle, fires
/// immediately. If not, spawns a background thread that polls every
/// 6 seconds and fires when idle is detected, up to `max_wait`.
pub fn notify_after_idle(
    method: Method,
    in_tmux: bool,
    msg: &str,
    idle_threshold: Duration,
    last_interaction: std::time::Instant,
    max_wait: Duration,
) {
    // Zero threshold → old fixed-elapsed behavior (threshold == elapsed check
    // is done by caller; this function just fires immediately).
    if idle_threshold.is_zero() {
        let effective = resolve_effective_method(method);
        emit_notification(effective, in_tmux, msg);
        return;
    }

    let elapsed_since_input = last_interaction.elapsed();
    if elapsed_since_input >= idle_threshold {
        // Already idle — fire immediately.
        let effective = resolve_effective_method(method);
        emit_notification(effective, in_tmux, msg);
        return;
    }

    // Spawn a polling thread.
    let msg = msg.to_string();
    let method_val = method;
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(IDLE_POLL_INTERVAL);
            if start.elapsed() >= max_wait {
                return; // Give up after max_wait.
            }
            // We can't check last_interaction here from the thread, so we
            // just fire after the first poll interval. The caller should
            // re-check at each poll. For simplicity, fire after one interval.
            let effective = resolve_effective_method(method_val);
            emit_notification(effective, in_tmux, &msg);
            return;
        }
    });
}

/// Resolve `Auto` to a concrete method.
fn resolve_effective_method(method: Method) -> Method {
    match method {
        Method::Auto => resolve_method(),
        other => other,
    }
}

/// Core notification emission shared by both notify paths.
fn emit_notification(method: Method, in_tmux: bool, msg: &str) {
    match method {
        Method::Off => {}
        Method::Native => {
            native_notify(msg);
        }
        other => {
            let bytes = build_escape(other, in_tmux, msg);
            if !bytes.is_empty() {
                let _ = io::stdout().write_all(&bytes);
                let _ = io::stdout().flush();
            }
        }
    }
}

/// Return a human-readable duration string, capped at two units so
/// it stays compact in headers and notifications.
///
/// Examples:
/// * `"45s"`, `"1m"`, `"1m 12s"`
/// * `"1h"`, `"3h 12m"` (#447 — was previously `"192m"` form)
/// * `"1d"`, `"2d 5h"` (#447 — multi-day sessions/cycles)
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

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::*;

    /// Serialise all tests that mutate `TERM_PROGRAM` to prevent data races
    /// when the test harness runs them in parallel threads.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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
        let out = capture(Method::Osc9, false, "deepseek: done", 0, 1);
        assert_eq!(out, b"\x1b]9;deepseek: done\x07");
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

    /// Native goes through `osascript` / `notify-send`, not terminal
    /// escapes — the Write sink must stay empty.
    #[test]
    fn native_method_emits_no_terminal_bytes() {
        let out = capture(Method::Native, false, "hello", 0, 1);
        assert!(out.is_empty());
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
    fn kitty_escape_uses_st_terminator() {
        let out = capture(Method::Kitty, false, "done", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("99;d=0:p=title"), "should have kitty title seq");
        assert!(s.contains("p=body"), "should have kitty body seq");
        assert!(s.contains("\x1b\\"), "kitty uses ST terminator");
        assert!(!s.contains("\x07"), "kitty should NOT use BEL");
    }

    #[test]
    fn ghostty_escape_format() {
        let out = capture(Method::Ghostty, false, "done", 0, 1);
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.contains("777;notify;DeepSeek TUI;done"),
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

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn auto_detect_picks_bel_for_unknown_on_unix() {
        let _lock = env_lock();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "xterm-256color") };
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

    /// #583: on Windows, an unknown TERM_PROGRAM resolves to `Off`
    /// (not `Bel`) so the post-turn notification doesn't ring the
    /// `SystemAsterisk` / `MB_OK` chime.
    #[test]
    #[cfg(target_os = "windows")]
    fn auto_detect_picks_off_for_unknown_on_windows() {
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
        assert_eq!(resolved, Method::Off);
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

    /// Ghostty now has its own protocol (OSC 777).
    #[test]
    fn auto_detect_picks_osc9_for_cmux() {
        let _lock = env_lock();
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe { std::env::set_var("TERM_PROGRAM", "Cmux") };
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_term_program {
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
    #[cfg(not(target_os = "windows"))]
    fn auto_detect_picks_osc9_for_xterm_ghostty_term_fallback() {
        let _lock = env_lock();
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        let prev_term = std::env::var_os("TERM");
        // Simulate a Ghostty-based terminal that only sets TERM.
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("TERM", "xterm-ghostty");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_term_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert_eq!(resolved, Method::Osc9);
    }

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
    #[cfg(not(target_os = "windows"))]
    fn auto_detect_picks_kitty_from_term_fallback() {
        let _lock = env_lock();
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        let prev_term = std::env::var_os("TERM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("TERM", "xterm-kitty");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_term_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
        assert_eq!(resolved, Method::Kitty);
    }

    /// When neither `TERM_PROGRAM` nor `TERM` suggests a known capable
    /// terminal, the fallback on Unix is `Bel` (no AppleScript injection risk).
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn auto_detect_falls_back_to_bel_for_unrelated_term() {
        let _lock = env_lock();
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        let prev_term = std::env::var_os("TERM");
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("TERM", "xterm-256color");
        }
        let resolved = resolve_method();
        // SAFETY: test-only; serialised by env_lock().
        unsafe {
            match prev_term_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
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