//! Async-signal-safe terminal restore for abort-class process deaths.
//!
//! #5424: a v0.9.7 user's TUI exited by itself mid-turn with none of the
//! ordinary cleanup running — no panic hook (so no crash dump), no
//! `TerminalCleanupGuard`, no tokio signal task — and the shell was left with
//! mouse capture still enabled, leaking SGR mouse-motion bytes into zsh.
//! Whatever the fatal cause (a stack overflow aborts after Rust prints its
//! message; an allocation failure aborts; `catch_unwind` cannot see either),
//! the *terminal poisoning* is fixable from a classic signal handler: one
//! fixed byte string written with `write(2)`.
//!
//! Scope is deliberately narrow:
//!
//! - **SIGABRT, SIGBUS, SIGILL, SIGFPE** are intercepted. **SIGSEGV is not**:
//!   the Rust runtime's stack-overflow diagnostic runs on SIGSEGV, and this
//!   handler must not bury it. Rust's overflow path itself funnels into
//!   `abort()`, i.e. SIGABRT, so stack overflows still get the restore.
//! - After the restore bytes are written (best-effort, both stdout and the
//!   crash marker file), the disposition is reset to `SIG_DFL` and the signal
//!   is re-raised, so the process keeps dying with the same signal — wait
//!   status, core-dump behavior, and `$?` are unchanged.
//! - Installed only when stdout is a TTY, so piped/embedded surfaces never
//!   get escape bytes injected into their output.
//!
//! The marker file (`.codewhale/crashes/last-fatal-signal.log`, appended,
//! never truncated) records which signal fired. The kernel's mtime on the
//! file timestamps the crash without any time formatting in the handler. On
//! the next real-world #5424-class report, that one line distinguishes an
//! abort (stack overflow / alloc failure / double panic) from an OOM-kill
//! (SIGKILL — uninterceptable, no marker) before any logs arrive.

/// The restore byte string, written in a single `write(2)`.
///
/// Mirrors `emergency_restore_terminal`'s mode teardown, minus raw mode
/// (termios state lives behind a lock that may be held by the dying thread)
/// and minus every query (a dead process cannot read replies). Modes left
/// over that a shell does not self-heal are the ones that poison input:
/// mouse capture and the kitty keyboard stack get the full reset.
#[cfg(unix)]
const FATAL_RESTORE_BYTES: &[u8] = concat!(
    "\x1b[?2026l", // close any open DEC 2026 synchronized-update batch
    "\x1b[<1u",    // pop one kitty keyboard-enhancement stack level
    "\x1b[?1007l", // alternate scroll off
    "\x1b[?1004l", // focus reporting off
    "\x1b[?2004l", // bracketed paste off
    "\x1b[?1006l", // SGR mouse encoding off
    "\x1b[?1002l", // button-event mouse tracking off
    "\x1b[?1003l", // any-motion mouse tracking off
    "\x1b[?1000l", // normal mouse tracking off
    "\x1b[?1049l", // leave the alternate screen
    "\x1b[?25h",   // show the cursor
)
.as_bytes();

/// Absolute path of the append-only fatal-signal marker, fixed at install
/// time (before any worker thread exists, after which it is never written
/// again — a plain load from the signal handler).
#[cfg(unix)]
static MARKER_PATH: std::sync::OnceLock<Box<[u8; 4096]>> = std::sync::OnceLock::new();
#[cfg(unix)]
static MARKER_LEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Install the fatal-signal restore guard. POSIX only; no-op elsewhere.
///
/// Call once, early, on the main thread before worker threads are spawned
/// (the install writes the `OnceLock`s; after that they are read-only).
pub(crate) fn install_fatal_signal_guard() {
    #[cfg(unix)]
    {
        // Piped/embedded surfaces must never receive escape bytes.
        if unsafe { libc::isatty(libc::STDOUT_FILENO) } == 0 {
            tracing::debug!("Fatal-signal terminal guard skipped: stdout is not a TTY");
            return;
        }
        if let Some(home) = crate::config::effective_home_dir() {
            let dir = home.join(".codewhale").join("crashes");
            // Pre-create so the handler's open(2) cannot fail on ENOENT and
            // so a first crash needs no directory creation mid-signal.
            if std::fs::create_dir_all(&dir).is_ok() {
                let path = dir.join("last-fatal-signal.log");
                if let Ok(bytes) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) {
                    let len = bytes.as_bytes().len();
                    if len < 4096 {
                        let mut buf = [0u8; 4096];
                        buf[..len].copy_from_slice(bytes.as_bytes());
                        let _ = MARKER_PATH.set(Box::new(buf));
                        MARKER_LEN.store(len, std::sync::atomic::Ordering::Release);
                    }
                }
            }
        }
        for signal in [libc::SIGABRT, libc::SIGBUS, libc::SIGILL, libc::SIGFPE] {
            unsafe { install_handler(signal) };
        }
        tracing::debug!("Fatal-signal terminal guard installed (ABRT/BUS/ILL/FPE)");
    }
    #[cfg(not(unix))]
    {
        // Windows console modes are restored by the panic hook and the
        // cleanup guard; there is no signal-class death to intercept there.
    }
}

/// Classic handler: write the fixed restore bytes, append the marker line,
/// then re-raise with the default disposition so the wait status is honest.
///
/// # Safety
///
/// Only async-signal-safe operations: `write(2)`, `open(2)`, `close(2)`,
/// `signal(2)`, `raise(2)`, and fixed-buffer arithmetic. No allocation, no
/// locks (the `OnceLock`/`AtomicUsize` reads complete before any thread
/// exists and are never written again).
#[cfg(unix)]
unsafe extern "C" fn fatal_signal_handler(signal: libc::c_int) {
    unsafe {
        // 1. Restore the terminal: stdout first, stderr as fallback.
        let mut written: usize = 0;
        while written < FATAL_RESTORE_BYTES.len() {
            let n = libc::write(
                libc::STDOUT_FILENO,
                FATAL_RESTORE_BYTES.as_ptr().add(written) as *const libc::c_void,
                FATAL_RESTORE_BYTES.len() - written,
            );
            if n <= 0 {
                break;
            }
            written += n as usize;
        }
        if written == 0 {
            let _ = libc::write(
                libc::STDERR_FILENO,
                FATAL_RESTORE_BYTES.as_ptr() as *const libc::c_void,
                FATAL_RESTORE_BYTES.len(),
            );
        }

        // 2. Append the one-line marker (mtime timestamps it).
        let len = MARKER_LEN.load(std::sync::atomic::Ordering::Acquire);
        if len > 0
            && let Some(path) = MARKER_PATH.get()
        {
            let fd = libc::open(
                path.as_ptr() as *const libc::c_char,
                libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT,
                0o600,
            );
            if fd >= 0 {
                // "signal=NN\n" in a fixed buffer; no formatting machinery.
                let mut line = [0u8; 16];
                line[0] = b's';
                line[1] = b'i';
                line[2] = b'g';
                line[3] = b'n';
                line[4] = b'a';
                line[5] = b'l';
                line[6] = b'=';
                let mut value = if signal < 0 { 0 } else { signal } as u32;
                let mut digits = [0u8; 10];
                let mut count = 0;
                loop {
                    digits[count] = b'0' + (value % 10) as u8;
                    value /= 10;
                    count += 1;
                    if value == 0 || count == digits.len() {
                        break;
                    }
                }
                for index in 0..count {
                    line[7 + index] = digits[count - 1 - index];
                }
                let total = 7 + count;
                line[total] = b'\n';
                let _ = libc::write(fd, line.as_ptr() as *const libc::c_void, total + 1);
                let _ = libc::close(fd);
            }
        }

        // 3. Die with the honest wait status.
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Install [`fatal_signal_handler`] for one signal via `sigaction`.
///
/// # Safety
///
/// `signal` must be a fatal signal whose default action is to terminate.
#[cfg(unix)]
unsafe fn install_handler(signal: libc::c_int) {
    unsafe {
        // Zero the whole struct then set our two fields: the remaining
        // members (empty signal mask; any hidden per-OS plumbing like the
        // Linux sa_restorer) are exactly what a zeroed default means, and
        // the wrapper `sigaction` fills in what it owns.
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = fatal_signal_handler as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
            tracing::warn!(signal, "fatal-signal guard install failed");
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn restore_bytes_cover_every_poisoning_mode() {
        // Input-poisoning modes first: mouse capture (all four DEC modes)
        // and the kitty keyboard stack.
        let bytes = String::from_utf8_lossy(FATAL_RESTORE_BYTES).to_string();
        for mode in [
            "?1006l", "?1002l", "?1003l", "?1000l", "<1u", "?2004l", "?1004l", "?1007l", "?1049l",
            "?2026l", "?25h",
        ] {
            assert!(
                bytes.contains(mode),
                "fatal restore must reset {mode}; got: {bytes:?}"
            );
        }
    }

    #[test]
    fn restore_bytes_are_one_write_friendly() {
        // No interior NULs, ASCII-only escape program, reasonable size.
        assert!(!FATAL_RESTORE_BYTES.contains(&0));
        assert!(FATAL_RESTORE_BYTES.len() < 128);
        assert!(FATAL_RESTORE_BYTES.starts_with(b"\x1b[?2026l"));
    }
}
