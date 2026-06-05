//! Process hardening for Linux sandbox defense-in-depth (#2183).
//!
//! This module applies kernel-level restrictions to the codewhale-tui process
//! itself. Unlike Landlock/seccomp which restrict child processes spawned for
//! shell commands, these hardening measures protect the *parent* TUI process
//! from information leaks and privilege-escalation vectors.
//!
//! # Ordering constraints
//!
//! `apply_process_hardening()` MUST be called **before** the Tokio runtime is
//! booted and **before** any worker threads are spawned. The reasons:
//!
//! 1. `PR_SET_DUMPABLE` — once set to 0, the process cannot be ptraced and
//!    `/proc/self/` becomes root-owned. This must happen before any threads
//!    exist, because the kernel applies dumpable state per-thread-group and
//!    changing it after threads are live can race with `/proc` lookups.
//!
//! 2. `PR_SET_NO_NEW_PRIVS` — prevents the process and all descendants from
//!    ever gaining new privileges via setuid/setgid/fscaps. This is
//!    irreversible and must be applied before executing any helper binaries or
//!    subprocesses that might (incorrectly) rely on privilege boundaries.
//!
//! 3. `RLIMIT_CORE` — disables core dumps so that sensitive in-memory data
//!    (API keys, tokens, prompt content) is never written to disk on a crash.
//!    Setting this before any data is loaded into memory is the safest posture.
//!
//! # Platform support
//!
//! These hardening measures are Linux-only (they use `prctl` and `setrlimit`
//! from the `libc` crate). On non-Linux platforms, `apply_process_hardening()`
//! is a no-op that logs a debug-level message.

/// Apply process-level hardening measures.
///
/// On Linux, this:
/// - Sets `PR_SET_DUMPABLE` to 0 (prevents ptrace, core dumps)
/// - Sets `PR_SET_NO_NEW_PRIVS` to 1 (irreversible no-new-privileges)
/// - Sets `RLIMIT_CORE` to 0 (disables core dumps)
///
/// On non-Linux platforms this is a no-op.
///
/// # Panics
///
/// Does NOT panic. Failures are logged via `tracing::warn` because the
/// hardening is defense-in-depth — the sandbox still protects child processes
/// even if these prctls fail (e.g., in a container where some are restricted).
pub fn apply_process_hardening() {
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    {
        apply_linux_hardening();
    }
    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
    {
        #[cfg(all(target_os = "linux", target_env = "ohos"))]
        tracing::debug!("Process hardening skipped: not supported on HarmonyOS");
        #[cfg(not(target_os = "linux"))]
        tracing::debug!("Process hardening skipped: not on Linux");
    }
}

/// Linux-specific hardening implementation.
#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
fn apply_linux_hardening() {
    // ── PR_SET_DUMPABLE = 0 ────────────────────────────────────────────────
    //
    // When dumpable is 0:
    // - The process cannot be ptraced by non-root
    // - /proc/<pid>/ becomes owned by root:root (mode 0400)
    // - No core dumps are produced
    //
    // Pattern from openai/codex codex-rs/codex-sandbox/src/linux.rs; reimplemented.
    //
    // Safety: prctl with PR_SET_DUMPABLE modifies only the calling process.
    let result = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0i64, 0i64, 0i64, 0i64) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(
            "PR_SET_DUMPABLE failed ({}); continuing without this hardening",
            err
        );
    } else {
        tracing::debug!("PR_SET_DUMPABLE=0 applied");
    }

    // ── PR_SET_NO_NEW_PRIVS = 1 ────────────────────────────────────────────
    //
    // Once set, neither this process nor any descendant can ever gain new
    // privileges via setuid, setgid, file capabilities, or LSMs like SELinux
    // transitions. This is the strongest anti-escalation primitive the kernel
    // offers.
    //
    // Pattern from openai/codex codex-rs/codex-sandbox/src/linux.rs; reimplemented.
    //
    // Safety: prctl with PR_SET_NO_NEW_PRIVS modifies only the calling process
    // and its future descendants.
    let result = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1i64, 0i64, 0i64, 0i64) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(
            "PR_SET_NO_NEW_PRIVS failed ({}); continuing without this hardening",
            err
        );
    } else {
        tracing::debug!("PR_SET_NO_NEW_PRIVS=1 applied");
    }

    // ── RLIMIT_CORE = 0 ────────────────────────────────────────────────────
    //
    // Disables core dumps at the rlimit level. In combination with
    // PR_SET_DUMPABLE=0, this provides a belt-and-suspenders guarantee that
    // no core file will ever be written.
    //
    // Safety: setrlimit modifies resource limits for the calling process only.
    let rlim_core = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let result = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const rlim_core) };
    if result != 0 {
        let err = std::io::Error::last_os_error();
        tracing::warn!(
            "RLIMIT_CORE failed ({}); continuing without this hardening",
            err
        );
    } else {
        tracing::debug!("RLIMIT_CORE=0 applied");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_process_hardening_does_not_panic() {
        // This test exists to ensure the function can be called without
        // panicking, even on platforms where hardening is a no-op.
        apply_process_hardening();
    }
}
