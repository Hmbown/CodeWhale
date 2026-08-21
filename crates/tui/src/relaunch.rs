//! `/relaunch` process-image handoff.
//!
//! `/relaunch` deliberately reuses the `/exit` teardown path: the command
//! records the current session id in
//! [`App::pending_relaunch`](crate::tui::app::App::pending_relaunch) and
//! returns `AppAction::Quit`, so the event loop shuts the engine down, the
//! persistence actor flushes and saves the session, and the terminal is
//! restored exactly as a plain quit would. At the end of `run_tui` the
//! pending id is handed to [`request`]; after telemetry has closed the
//! session out, `run_async_main_inner` calls [`exec_relaunch`] to replace
//! the process image with the same executable resuming that session.
//!
//! # Why `exec`, and why this late
//!
//! On Unix, `CommandExt::exec` replaces the running image in place: the PID,
//! the terminal, and every open file descriptor are inherited by the resumed
//! process — no orphan for a supervisor to reap, and the terminal stays ours.
//! POSIX defines `exec` from a multithreaded process: the other threads are
//! discarded together with the old image and the syscall itself touches no
//! locks, so the still-running Tokio workers at this point are not a hazard.
//!
//! The replacement must not happen earlier than it does. Inside the event
//! loop the alternate screen and raw mode are still active; before
//! `run_tui` returns they are restored, the persistence actor has flushed the
//! session to disk, and only after `finish_telemetry` has the old session's
//! `session_end` been recorded and persisted. Running the exec after
//! `run_async_main_dispatch` returns is what makes the new process an
//! ordinary, clean `resume` launch.
//!
//! Windows has no `exec`; the handoff is consumed as a no-op there and the
//! resume hint printed at quit is the relaunch instruction.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The session id `/relaunch` wants the next process image to resume.
/// Written at the end of `run_tui`, consumed by `run_async_main_inner` after
/// telemetry close-out. Process-wide state is the pragmatic channel here:
/// the marker must cross `run_async_main_dispatch`, whose return type is
/// shared with every other surface, so it cannot ride the ordinary `Result`.
static PENDING_RELAUNCH: Mutex<Option<String>> = Mutex::new(None);

/// Hand the current session id to the relaunch handoff.
pub(crate) fn request(session_id: &str) {
    let mut pending = PENDING_RELAUNCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *pending = Some(session_id.to_string());
}

/// Take the pending relaunch session id, if one was requested.
pub(crate) fn take_pending() -> Option<String> {
    PENDING_RELAUNCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// The argv `/relaunch` hands to the next process image:
/// `<exe> resume <session-id>` — the CLI's positional resume form, which the
/// standalone TUI binary also accepts as its `resume` subcommand.
pub(crate) fn relaunch_argv(exe: &Path, session_id: &str) -> (PathBuf, Vec<String>) {
    (
        exe.to_path_buf(),
        vec!["resume".to_string(), session_id.to_string()],
    )
}

/// The current executable for the relaunch handoff.
///
/// Resolves the platform image path and keeps it only when it names a live
/// file; otherwise falls back to `argv[0]` (the invocation name, which the
/// Unix `exec` resolves through `PATH`). The fallback covers both a
/// resolution error and a dead resolved path: a missing file, or a Linux
/// binary replaced by rename, which `/proc/self/exe` reports with a literal
/// ` (deleted)` suffix.
pub(crate) fn current_executable() -> Option<PathBuf> {
    resolve_current_executable(
        std::env::current_exe().ok(),
        std::env::args().next().map(PathBuf::from),
    )
}

/// Pick the executable to relaunch from a platform-resolved image path and
/// `argv[0]`.
///
/// `resolved` is kept only when it is non-empty, exists, and is not marked
/// ` (deleted)`; every other case — including a resolution error — falls
/// back to `argv[0]`.
pub(crate) fn resolve_current_executable(
    resolved: Option<PathBuf>,
    argv0: Option<PathBuf>,
) -> Option<PathBuf> {
    let fallback = argv0.filter(|path| !path.as_os_str().is_empty());
    match resolved {
        Some(exe) if !exe.as_os_str().is_empty() && exe.exists() && !marked_deleted(&exe) => {
            Some(exe)
        }
        _ => fallback,
    }
}

/// True when the path's file name ends in the Linux `/proc/self/exe` marker
/// for a binary replaced by rename.
fn marked_deleted(exe: &Path) -> bool {
    exe.file_name()
        .map(|name| name.to_string_lossy().ends_with(" (deleted)"))
        .unwrap_or(false)
}

/// Replace this process with `<exe> resume <session-id>`.
///
/// Atomic on Unix: `exec` never returns on success — the returned error is
/// only produced when the replacement failed. The terminal remains this
/// process's terminal and no orphan process is left for a supervisor to
/// reap. Any stdout still buffered in the old image would be discarded by
/// the replacement, so it is flushed first.
#[cfg(unix)]
pub(crate) fn exec_relaunch(exe: &Path, session_id: &str) -> std::io::Error {
    use std::io::Write;
    use std::os::unix::process::CommandExt;

    let _ = std::io::stdout().flush();
    let (exe, argv) = relaunch_argv(exe, session_id);
    let mut command = std::process::Command::new(exe);
    command.args(argv);
    command.exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the handoff tests: `PENDING_RELAUNCH` is process-wide
    /// state and the harness runs tests in parallel, so each test takes
    /// exclusive ownership of it through this gate.
    static TEST_GATE: Mutex<()> = Mutex::new(());

    fn gate() -> std::sync::MutexGuard<'static, ()> {
        TEST_GATE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn relaunch_argv_builds_the_positional_resume_form() {
        let exe = Path::new("/usr/local/bin/codewhale");
        let (argv_exe, argv) = relaunch_argv(exe, "session-123");
        assert_eq!(argv_exe, exe);
        assert_eq!(argv, ["resume", "session-123"]);
    }

    #[test]
    fn relaunch_argv_keeps_windows_style_exe_paths_verbatim() {
        let exe = Path::new("C:/tools/codewhale.exe");
        let (argv_exe, argv) = relaunch_argv(exe, "abc");
        assert_eq!(argv_exe, exe);
        assert_eq!(argv, ["resume", "abc"]);
    }

    #[test]
    fn request_then_take_hands_the_session_id_over_once() {
        let _gate = gate();
        assert_eq!(take_pending(), None, "handoff must start empty");
        request("session-1");
        assert_eq!(take_pending(), Some("session-1".to_string()));
        assert_eq!(take_pending(), None, "taking consumes the handoff");
    }

    #[test]
    fn a_later_request_replaces_an_unconsumed_one() {
        let _gate = gate();
        request("session-1");
        request("session-2");
        assert_eq!(take_pending(), Some("session-2".to_string()));
    }

    #[test]
    fn a_current_exe_marked_deleted_falls_back_to_argv0() {
        let argv0 = PathBuf::from("codewhale");
        assert_eq!(
            resolve_current_executable(
                Some(PathBuf::from("/x/codewhale (deleted)")),
                Some(argv0.clone()),
            ),
            Some(argv0),
        );
    }

    #[test]
    fn a_missing_current_exe_falls_back_to_argv0() {
        let argv0 = PathBuf::from("codewhale");
        assert_eq!(
            resolve_current_executable(
                Some(PathBuf::from("/x/does/not/exist/codewhale")),
                Some(argv0.clone()),
            ),
            Some(argv0),
        );
    }

    #[test]
    fn a_live_current_exe_is_kept_verbatim() {
        let exe = std::env::current_exe().expect("test binary path");
        assert!(exe.exists(), "the test binary itself is the live input");
        assert_eq!(
            resolve_current_executable(Some(exe.clone()), Some(PathBuf::from("codewhale"))),
            Some(exe),
        );
    }

    /// Pins the name check itself: `exists()` is true here, so only the
    /// ` (deleted)` suffix can trigger the fallback.
    #[test]
    fn an_existing_file_named_deleted_is_rejected_by_name() {
        let dir =
            std::env::temp_dir().join(format!("codewhale-relaunch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marked = dir.join("codewhale (deleted)");
        std::fs::write(&marked, b"").unwrap();
        let argv0 = PathBuf::from("codewhale");
        let result = resolve_current_executable(Some(marked), Some(argv0.clone()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result, Some(argv0));
    }
}
