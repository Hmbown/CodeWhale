//! TUI runtime logging. Initializes a `tracing-subscriber` that writes to a
//! daily-rolling file under `~/.deepseek/logs/`, and (on Unix) redirects the
//! process's `stderr` fd to that same file for the lifetime of the alt-screen
//! TUI.
//!
//! Why this exists:
//!
//! The TUI runs inside an alt-screen buffer drawn by `ratatui` using an
//! incremental diff renderer. The renderer assumes nothing else is writing
//! to the terminal — its internal "current cells" model is the only source
//! of truth for what's on screen. If anything emits raw bytes to stdout or
//! stderr while the alt-screen is active (an `eprintln!` from a sub-agent,
//! a `tracing` warning that defaulted to `stderr`, a panic message, a
//! third-party crate's verbose output, …) those bytes land in the alt-screen
//! buffer at the current cursor position, scroll the buffer up, and leave
//! the renderer's model out of sync with reality. The visible symptom is
//! "scroll demon": the TUI content drifts down, leaving a band of blank
//! rows above the header. This was the regression in issue #1085 (fixed in
//! v0.8.18 by adding a viewport-reset path) and re-surfaced in v0.8.27
//! when the flicker fix dropped the `\x1b[2J\x1b[3J` deep-clear that had
//! been masking the underlying leak.
//!
//! Defence-in-depth:
//!   1. A `tracing-subscriber` writes formatted logs to
//!      `~/.deepseek/logs/tui-YYYY-MM-DD.log` so `tracing::warn!` /
//!      `tracing::error!` calls go somewhere observable instead of
//!      disappearing into the void (the TUI previously had no global
//!      subscriber, so contributors reached for `eprintln!`).
//!   2. On Unix the process's stderr fd is redirected (via `dup2`) to the
//!      same log file for the lifetime of `TuiLogGuard`. Any raw stderr
//!      write — ours, a dependency's, a panic message — lands in the log
//!      file instead of the alt-screen. The guard restores the original
//!      stderr fd on drop so post-TUI shutdown messages still reach the
//!      user's terminal.
//!   3. Crate-level `#![deny(clippy::print_stderr, clippy::print_stdout)]`
//!      on the TUI runtime modules forbids new `eprintln!` / `println!`
//!      calls at compile time. CLI-output paths (`main.rs` eval, init,
//!      `runtime_api::print_*`, `logging::info`/`warn`) keep their existing
//!      prints via `#[allow(clippy::print_stderr)]` because they run before
//!      the alt-screen is entered.

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Owns the active tracing subscriber and (on Unix) a saved copy of the
/// original `stderr` fd so it can be restored on drop. Dropped when the TUI
/// exits the alt-screen.
pub struct TuiLogGuard {
    #[cfg(unix)]
    saved_stderr_fd: Option<libc::c_int>,
    _file: File,
    // Exposed via `log_path()` for diagnostics (e.g. `/doctor`,
    // `--print-log-path`). Currently no caller — keep the accessor
    // wired up so adding one later doesn't require revisiting the
    // guard struct.
    #[allow(dead_code)]
    log_path: PathBuf,
}

impl TuiLogGuard {
    /// Path the subscriber is writing to.
    #[allow(dead_code)]
    #[must_use]
    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }
}

#[cfg(unix)]
impl Drop for TuiLogGuard {
    fn drop(&mut self) {
        if let Some(saved) = self.saved_stderr_fd.take() {
            // SAFETY: `saved` came from `libc::dup` of the original stderr
            // fd in `init`; calling `dup2` to restore it is the standard
            // pairing. If `dup2` fails we just leak the saved fd — the
            // process is exiting anyway.
            unsafe {
                let _ = libc::dup2(saved, libc::STDERR_FILENO);
                let _ = libc::close(saved);
            }
        }
    }
}

#[cfg(not(unix))]
impl Drop for TuiLogGuard {
    fn drop(&mut self) {}
}

/// Initialize the TUI logging subsystem. Idempotent across re-entry by way
/// of `set_default` — if a global subscriber is already set we still install
/// the stderr redirect.
///
/// Returns a guard that must outlive the alt-screen session. Drop it after
/// `LeaveAlternateScreen` so any shutdown messages reach the user.
pub fn init() -> Result<TuiLogGuard> {
    let log_dir = log_directory().context("could not resolve TUI log directory")?;
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create {}", log_dir.display()))?;

    let date = chrono::Local::now().format("%Y-%m-%d");
    let log_path = log_dir.join(format!("tui-{date}.log"));

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;

    // The tracing-subscriber consumes a clone of the file handle for its
    // writer. We keep our own handle for the dup2 redirect below — we need
    // the same on-disk file but a separate fd so the subscriber's writes
    // and the raw-stderr writes don't fight over the same kernel offset.
    let subscriber_file = file
        .try_clone()
        .context("failed to clone log file handle for subscriber")?;

    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let subscriber = tracing_subscriber::registry().with(env_filter).with(
        fmt::layer()
            .with_writer(move || {
                subscriber_file
                    .try_clone()
                    .expect("clone log file handle for tracing writer")
            })
            .with_ansi(false)
            .with_target(true)
            .with_thread_ids(false),
    );

    // Best-effort: if a subscriber is already set (e.g., re-entry, or a
    // host process installed one), we skip ours rather than panic. The
    // stderr redirect below still happens.
    let _ = tracing::subscriber::set_global_default(subscriber);

    #[cfg(unix)]
    let saved_stderr_fd = redirect_stderr_to(&file).ok();

    Ok(TuiLogGuard {
        #[cfg(unix)]
        saved_stderr_fd,
        _file: file,
        log_path,
    })
}

fn log_directory() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && !home.as_os_str().is_empty()
    {
        return Some(home.join(".deepseek").join("logs"));
    }
    if let Some(userprofile) = std::env::var_os("USERPROFILE").map(PathBuf::from)
        && !userprofile.as_os_str().is_empty()
    {
        return Some(userprofile.join(".deepseek").join("logs"));
    }
    dirs::home_dir().map(|h| h.join(".deepseek").join("logs"))
}

#[cfg(unix)]
fn redirect_stderr_to(file: &File) -> Result<libc::c_int> {
    use std::os::fd::AsRawFd;
    let target = file.as_raw_fd();
    // SAFETY: `libc::dup` and `libc::dup2` are the documented fd-management
    // primitives. We save the current stderr fd before reassigning so the
    // guard can restore it on drop.
    unsafe {
        let saved = libc::dup(libc::STDERR_FILENO);
        if saved < 0 {
            return Err(
                anyhow::Error::from(std::io::Error::last_os_error()).context("dup(STDERR_FILENO)")
            );
        }
        if libc::dup2(target, libc::STDERR_FILENO) < 0 {
            let err = std::io::Error::last_os_error();
            let _ = libc::close(saved);
            return Err(anyhow::Error::from(err).context("dup2(log_file, STDERR_FILENO)"));
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_directory_prefers_home() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        // SAFETY: serialised by lock_test_env.
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::set_var("USERPROFILE", "");
        }

        let resolved = log_directory().expect("log_directory should resolve");
        assert_eq!(resolved, tmp.path().join(".deepseek").join("logs"));

        // SAFETY: cleanup under the same lock.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_userprofile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }
}
