//! Process hardening for Linux sandbox defense-in-depth (#2183).
//!
//! This module applies kernel-level restrictions to the codewhale-tui process
//! itself. These hardening measures protect the *parent* TUI process and its
//! descendants from information leaks and privilege-escalation vectors; they
//! are not a filesystem or network sandbox for child commands. The seccomp
//! source module is not wired into child execution yet.
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
//!    Because this also blocks intentional privilege gains — `sudo`, `su`,
//!    setuid helpers — a user who runs Codewhale as a wheel/wheel-equivalent
//!    administrator and wants the model to be able to escalate can opt out of
//!    exactly this one measure with `CODEWHALE_NO_NEW_PRIVS=0` (#5413). The
//!    other two measures stay on.
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

/// Environment variable that opts the process out of `PR_SET_NO_NEW_PRIVS`.
///
/// `PR_SET_NO_NEW_PRIVS` is an irreversible, inherited-by-children kernel
/// flag, so applying it by default breaks workflows where the user *wants*
/// Codewhale to be able to escalate: `sudo`, `su`, setuid/fscaps helpers run
/// by a wheel-group administrator (#5413). Setting this variable to any
/// falsey value (`0`, `false`, `no`, `off`, `disabled`, or empty) skips
/// exactly this one measure. Any other value, or leaving it unset, keeps the
/// default hardening. `PR_SET_DUMPABLE` and `RLIMIT_CORE` are never skipped.
pub(crate) const NO_NEW_PRIVS_ENV: &str = "CODEWHALE_NO_NEW_PRIVS";

/// Whether a `CODEWHALE_NO_NEW_PRIVS` value requests skipping the
/// no-new-privileges flag. Falsey per the workspace env convention — the same
/// value set (`""`, `0`, `false`, `no`, `off`, `disabled`) that
/// `docs/CONFIGURATION.md` treats as "not set".
fn is_no_new_privs_opt_out(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off" | "disabled"
    )
}

fn no_new_privs_opted_out() -> bool {
    std::env::var_os(NO_NEW_PRIVS_ENV)
        .is_some_and(|value| is_no_new_privs_opt_out(&value.to_string_lossy()))
}

/// Apply process-level hardening measures.
///
/// On Linux, this:
/// - Sets `PR_SET_DUMPABLE` to 0 (prevents ptrace, core dumps)
/// - Sets `PR_SET_NO_NEW_PRIVS` to 1 (irreversible no-new-privileges),
///   unless `CODEWHALE_NO_NEW_PRIVS` holds a falsey value (#5413)
/// - Sets `RLIMIT_CORE` to 0 (disables core dumps)
///
/// On non-Linux platforms this is a no-op.
///
/// # Panics
///
/// Does NOT panic. Failures are logged via `tracing::warn` because the
/// hardening is defense-in-depth. A failure does not abort startup or change
/// whether a separately configured Seatbelt/bubblewrap command wrapper is
/// available.
pub fn apply_process_hardening() {
    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    {
        apply_linux_hardening();
    }
    #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
    {
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
    // That strength is also the flag's one legitimate break: a wheel-group
    // administrator running Codewhale over ssh loses `sudo`/`su`/setuid
    // helpers for the whole process tree (#5413). `CODEWHALE_NO_NEW_PRIVS`
    // with a falsey value skips only this measure, before any thread exists —
    // the same point in startup where the flag itself is applied.
    //
    // Pattern from openai/codex codex-rs/codex-sandbox/src/linux.rs; reimplemented.
    //
    // Safety: prctl with PR_SET_NO_NEW_PRIVS modifies only the calling process
    // and its future descendants.
    if no_new_privs_opted_out() {
        tracing::info!(
            target: "sandbox",
            "PR_SET_NO_NEW_PRIVS skipped via {NO_NEW_PRIVS_ENV}: setuid/sudo escalation is \
             allowed for this process tree"
        );
    } else {
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

    #[test]
    fn no_new_privs_opt_out_accepts_exactly_the_falsey_values() {
        // The workspace env convention: "", 0, false, no, off, disabled.
        for value in ["", "0", "false", "no", "off", "disabled"] {
            assert!(
                is_no_new_privs_opt_out(value),
                "{value:?} should opt out of PR_SET_NO_NEW_PRIVS"
            );
            // Case and surrounding whitespace must not change the answer.
            assert!(is_no_new_privs_opt_out(&format!(
                " {} ",
                value.to_uppercase()
            )));
        }
    }

    #[test]
    fn no_new_privs_opt_out_rejects_truthy_and_garbage_values() {
        // Anything else — including a typo — keeps the hardening on, so the
        // opt-out can never be entered by accident.
        for value in [
            "1",
            "true",
            "yes",
            "on",
            "enabled",
            "maybe",
            "0x0",
            "false-ish",
            "off!",
        ] {
            assert!(
                !is_no_new_privs_opt_out(value),
                "{value:?} should NOT opt out of PR_SET_NO_NEW_PRIVS"
            );
        }
    }

    #[test]
    fn no_new_privs_env_wiring_reads_the_documented_variable() {
        // The env is process-global and tests run in parallel threads, so the
        // wiring is asserted structurally: the constant is the documented
        // name, and the reader is a pure projection of var_os over that
        // constant. Setting/removing the variable here would race every other
        // test in the process.
        assert_eq!(NO_NEW_PRIVS_ENV, "CODEWHALE_NO_NEW_PRIVS");
        let projected = std::env::var_os(NO_NEW_PRIVS_ENV)
            .is_some_and(|value| is_no_new_privs_opt_out(&value.to_string_lossy()));
        assert_eq!(projected, no_new_privs_opted_out());
    }
}
