//! Lightweight verbose logging helpers for the CLI.

use std::sync::atomic::{AtomicBool, Ordering};

use colored::Colorize;

use crate::palette;
static VERBOSE: AtomicBool = AtomicBool::new(false);
static REQUESTED_VERBOSE: AtomicBool = AtomicBool::new(false);
static IN_ALT_SCREEN: AtomicBool = AtomicBool::new(false);

fn alt_screen_verbose_state(
    requested_verbose: bool,
    in_alt_screen: bool,
    is_windows: bool,
) -> bool {
    if is_windows && in_alt_screen {
        false
    } else {
        requested_verbose
    }
}

/// Enable or disable verbose logging output.
pub fn set_verbose(enabled: bool) {
    REQUESTED_VERBOSE.store(enabled, Ordering::SeqCst);
    VERBOSE.store(
        alt_screen_verbose_state(enabled, IN_ALT_SCREEN.load(Ordering::SeqCst), cfg!(windows)),
        Ordering::SeqCst,
    );
}

/// Suppress verbose CLI logging while the TUI owns the alt-screen.
pub fn suppress_for_tui_alt_screen() {
    let requested = REQUESTED_VERBOSE.load(Ordering::SeqCst);
    IN_ALT_SCREEN.store(true, Ordering::SeqCst);
    VERBOSE.store(
        alt_screen_verbose_state(requested, true, cfg!(windows)),
        Ordering::SeqCst,
    );
}

/// Restore the user's requested verbosity after leaving the alt-screen.
pub fn restore_after_tui_alt_screen() {
    let requested = REQUESTED_VERBOSE.load(Ordering::SeqCst);
    IN_ALT_SCREEN.store(false, Ordering::SeqCst);
    VERBOSE.store(
        alt_screen_verbose_state(requested, false, cfg!(windows)),
        Ordering::SeqCst,
    );
}

/// Return true when `DEEPSEEK_LOG_LEVEL` requests verbose output.
///
/// Note: `RUST_LOG` is intentionally NOT checked here — it controls the
/// `tracing` subscriber filter in `runtime_log.rs` (file logging) and
/// should not gate CLI verbose output. On Windows, where stderr is not
/// redirected to the log file, coupling the two causes tracing log
/// messages to leak into the TUI alt-screen.
#[must_use]
pub fn env_requests_verbose_logging() -> bool {
    std::env::var("DEEPSEEK_LOG_LEVEL")
        .ok()
        .is_some_and(|value| log_value_enables_verbose(&value))
}

fn log_value_enables_verbose(value: &str) -> bool {
    value.split(',').any(|directive| {
        let level = directive
            .rsplit('=')
            .next()
            .unwrap_or(directive)
            .trim()
            .to_ascii_lowercase();
        matches!(level.as_str(), "trace" | "debug" | "info")
    })
}

/// Check whether verbose logging is enabled.
#[must_use]
pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::SeqCst)
}

/// Emit a verbose info message (no-op when verbosity is disabled).
pub fn info(message: impl AsRef<str>) {
    if is_verbose() {
        let (r, g, b) = palette::DEEPSEEK_SKY_RGB;
        eprintln!("{} {}", "info".truecolor(r, g, b).bold(), message.as_ref());
    }
}

/// Emit a verbose warning message (no-op when verbosity is disabled).
pub fn warn(message: impl AsRef<str>) {
    if is_verbose() {
        let (r, g, b) = palette::DEEPSEEK_SKY_RGB;
        eprintln!("{} {}", "warn".truecolor(r, g, b).bold(), message.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn logging_state_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn lock_logging_state_test() -> MutexGuard<'static, ()> {
        logging_state_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reset_logging_state() {
        IN_ALT_SCREEN.store(false, Ordering::SeqCst);
        VERBOSE.store(false, Ordering::SeqCst);
        REQUESTED_VERBOSE.store(false, Ordering::SeqCst);
    }

    #[test]
    fn log_value_parser_accepts_common_rust_log_directives() {
        assert!(log_value_enables_verbose("debug"));
        assert!(log_value_enables_verbose("codewhale_cli=debug"));
        assert!(log_value_enables_verbose(
            "warn,codewhale_tui::client=trace"
        ));
        assert!(!log_value_enables_verbose("warn"));
        assert!(!log_value_enables_verbose("codewhale_tui=off"));
    }

    #[test]
    fn alt_screen_verbose_state_suppresses_verbose_on_windows() {
        assert!(!alt_screen_verbose_state(true, true, true));
        assert!(!alt_screen_verbose_state(false, true, true));
    }

    #[test]
    fn alt_screen_verbose_state_restores_requested_state_off_alt_screen() {
        assert!(alt_screen_verbose_state(true, false, true));
        assert!(!alt_screen_verbose_state(false, false, true));
    }

    #[test]
    fn alt_screen_verbose_state_is_noop_off_windows() {
        assert!(alt_screen_verbose_state(true, true, false));
        assert!(!alt_screen_verbose_state(false, true, false));
    }

    #[test]
    fn set_verbose_respects_alt_screen_suppression() {
        let _guard = lock_logging_state_test();
        reset_logging_state();
        IN_ALT_SCREEN.store(true, Ordering::SeqCst);
        set_verbose(true);
        assert!(REQUESTED_VERBOSE.load(Ordering::SeqCst));
        if cfg!(windows) {
            assert!(!is_verbose());
        } else {
            assert!(is_verbose());
        }
        reset_logging_state();
    }

    #[test]
    fn set_verbose_restores_requested_state_outside_alt_screen() {
        let _guard = lock_logging_state_test();
        reset_logging_state();
        set_verbose(true);
        assert!(is_verbose());
        set_verbose(false);
        assert!(!is_verbose());
        reset_logging_state();
    }
}
