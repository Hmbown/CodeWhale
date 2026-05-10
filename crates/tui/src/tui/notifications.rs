//! OSC 9 / BEL desktop notifications for long agent-turn completion.
//!
//! Writes a terminal escape to the provided sink (or stdout for the public
//! API) when a turn takes longer than the configured threshold. Supports
//! tmux DCS passthrough so OSC 9 reaches the outer terminal even when
//! running inside a tmux session.

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
    /// (`iTerm.app`, `Ghostty`, `WezTerm`); fall back to `Bel` on
    /// macOS / Linux. On Windows the fallback is `Off` instead of
    /// `Bel`, because the OS audio stack maps `\x07` to the
    /// `SystemAsterisk` / `MB_OK` chime — the same sound used by
    /// application error popups (#583). Windows users who want an
    /// audible cue can opt in by setting
    /// `[notifications].method = "bel"` explicitly.
    #[default]
    Auto,
    /// OSC 9 escape: `\x1b]9;<msg>\x07`
    Osc9,
    /// Plain BEL character: `\x07`
    Bel,
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

/// Resolve `Auto` to a concrete method by inspecting `$TERM_PROGRAM` and
/// `$LC_TERMINAL`.
///
/// Known OSC-9 capable programs: `iTerm.app`, `Ghostty`, `WezTerm`, `Cmux`
/// (these resolve to `Osc9` on every platform, including Windows
/// when running inside WezTerm).
///
/// The probe order is: `$TERM_PROGRAM` first, then `$LC_TERMINAL` as a
/// fallback for terminals (e.g. Cmux) that set `LC_TERMINAL` instead of
/// `TERM_PROGRAM`. If neither env var matches a known OSC-9 capable terminal,
/// the fallback is platform-dependent:
/// - **macOS / Linux / other Unix:** `Bel` (a single `\x07` byte).
/// - **Windows:** `Off`. BEL is mapped by the Windows audio stack
///   to `SystemAsterisk` / `MB_OK`, the same chime used by
///   application error popups, so it sounds like an error
///   notification even though the turn completed successfully (#583).
///   Users can opt back in with `[notifications].method = "bel"` or
///   pick a known OSC-9 terminal.
///
/// Terminals that set neither env var (e.g. Cmux in some configurations)
/// can force OSC 9 with `[notifications].method = "osc9"` in the config file.
#[must_use]
fn resolve_method() -> Method {
    const OSC9_TERMINALS: &[&str] = &["iTerm.app", "Ghostty", "WezTerm", "Cmux"];

    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if OSC9_TERMINALS.contains(&term_program.as_str()) {
        return Method::Osc9;
    }

    let lc_terminal = std::env::var("LC_TERMINAL").unwrap_or_default();
    if OSC9_TERMINALS.contains(&lc_terminal.as_str()) {
        return Method::Osc9;
    }

    if cfg!(target_os = "windows") {
        Method::Off
    } else {
        Method::Bel
    }
}

/// Build the raw escape bytes for the given method and message.
///
/// When `in_tmux` is `true` and the method is `Osc9`, the sequence is
/// wrapped in a DCS passthrough so tmux forwards it to the outer terminal:
/// `\x1bPtmux;\x1b<OSC-9>\x1b\\`
#[must_use]
fn build_escape(method: Method, in_tmux: bool, msg: &str) -> Vec<u8> {
    match method {
        Method::Bel => vec![b'\x07'],
        Method::Osc9 => {
            let inner = format!("\x1b]9;{msg}\x07");
            if in_tmux {
                // DCS passthrough: every ESC inside the payload must be
                // doubled so tmux does not interpret it as DCS end.
                let escaped_inner = inner.replace('\x1b', "\x1b\x1b");
                format!("\x1bPtmux;{escaped_inner}\x1b\\").into_bytes()
            } else {
                inner.into_bytes()
            }
        }
        // Auto and Off should not reach build_escape.
        Method::Auto | Method::Off => vec![],
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
/// (`iTerm.app`, `Ghostty`, `WezTerm`); the unknown-terminal fallback is
/// platform-aware — `Bel` on macOS / Linux, `Off` on Windows (where BEL
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
    #[cfg(not(target_os = "windows"))]
    fn auto_detect_picks_bel_for_unknown_on_unix() {
        let _lock = env_lock();
        let prev_tp = std::env::var_os("TERM_PROGRAM");
        let prev_lc = std::env::var_os("LC_TERMINAL");
        // SAFETY: test-only; serialised by env_lock().
        // Clear LC_TERMINAL so the LC_TERMINAL fallback probe does not
        // accidentally pick up an OSC-9 capable terminal from the test runner
        // environment and shadow the Bel fallback we're trying to verify.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "xterm-256color");
            std::env::remove_var("LC_TERMINAL");
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
