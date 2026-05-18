//! Lightweight verbose logging helpers for the CLI.

use std::sync::atomic::{AtomicBool, Ordering};

use colored::Colorize;

use crate::palette;
static VERBOSE: AtomicBool = AtomicBool::new(false);

/// Enable or disable verbose logging output.
pub fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::SeqCst);
}

/// Return true when `DEEPSEEK_LOG_LEVEL` requests verbose output.
///
/// Two separate logging systems exist and are intentionally kept independent:
///
/// 1. **CLI verbose output** (this module): controlled by `DEEPSEEK_LOG_LEVEL`.
///    When enabled, `info()` and `warn()` emit messages to stderr via
///    `eprintln!`. These are internal CLI diagnostics, not `tracing` events.
///
/// 2. **`tracing` subscriber** (file logging): controlled by `RUST_LOG` in
///    `runtime_log.rs`. This filters structured tracing spans and events
///    emitted by the `tracing` crate, writing them to the log file.
///
/// `RUST_LOG` is intentionally NOT checked here. If it were, setting
/// `RUST_LOG=trace` for file diagnostics would also enable CLI verbose
/// output, causing `eprintln!` messages to spill onto the TUI alt-screen.
/// The problem is especially visible on Windows where stderr is not
/// redirected to the log file.
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

    #[test]
    fn log_value_parser_accepts_common_rust_log_directives() {
        assert!(log_value_enables_verbose("debug"));
        assert!(log_value_enables_verbose("deepseek_cli=debug"));
        assert!(log_value_enables_verbose("warn,deepseek_tui::client=trace"));
        assert!(!log_value_enables_verbose("warn"));
        assert!(!log_value_enables_verbose("deepseek_tui=off"));
    }
}