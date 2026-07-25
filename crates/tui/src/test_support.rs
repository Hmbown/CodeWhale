//! Shared test-only helpers.

use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard, OnceLock, TryLockError};
use std::thread::ThreadId;

/// Build a syntactically valid, non-secret JWT fixture without embedding a
/// high-entropy token-shaped literal in Git history.
pub(crate) fn future_test_jwt(label: &str) -> String {
    use base64::Engine as _;

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":9999999999}"#);
    format!("test.{payload}.{label}")
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn env_lock_owner() -> &'static Mutex<Option<ThreadId>> {
    static OWNER: OnceLock<Mutex<Option<ThreadId>>> = OnceLock::new();
    OWNER.get_or_init(|| Mutex::new(None))
}

fn record_env_lock_owner() {
    let mut owner = match env_lock_owner().lock() {
        Ok(owner) => owner,
        Err(poisoned) => poisoned.into_inner(),
    };
    *owner = Some(std::thread::current().id());
}

fn current_thread_owns_contended_env_lock() -> bool {
    let owner = match env_lock_owner().lock() {
        Ok(owner) => owner,
        Err(poisoned) => poisoned.into_inner(),
    };
    owner.as_ref() == Some(&std::thread::current().id())
}

/// Owned process-wide test-environment lock.
///
/// Clearing the owner before the underlying mutex unlocks keeps re-entrant
/// reader detection exact; a stale thread id could otherwise let the previous
/// owner bypass a newly acquired lock during its tiny owner-registration
/// window.
pub(crate) struct TestEnvLock {
    _guard: MutexGuard<'static, ()>,
}

impl Drop for TestEnvLock {
    fn drop(&mut self) {
        let mut owner = match env_lock_owner().lock() {
            Ok(owner) => owner,
            Err(poisoned) => poisoned.into_inner(),
        };
        if owner.as_ref() == Some(&std::thread::current().id()) {
            *owner = None;
        }
    }
}

/// Acquire the process-wide env-var mutex.
///
/// If a prior test panicked while holding the lock, recover the guard instead
/// of cascading failures across unrelated tests.
pub(crate) fn lock_test_env() -> TestEnvLock {
    let guard = match env_lock().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    record_env_lock_owner();
    TestEnvLock { _guard: guard }
}

/// Read process-global test environment while respecting [`lock_test_env`].
///
/// Config-path writers hold the mutex for their whole test. Production path
/// resolution normally only reads the environment, but those reads still have
/// to wait or they can resolve another test's temporary config and later write
/// into it. The owner check makes the barrier re-entrant for a test that reads
/// its own guarded override.
pub(crate) fn with_test_env_lock<T>(read: impl FnOnce() -> T) -> T {
    match env_lock().try_lock() {
        Ok(_guard) => read(),
        Err(TryLockError::Poisoned(poisoned)) => {
            let _guard = poisoned.into_inner();
            read()
        }
        Err(TryLockError::WouldBlock) if current_thread_owns_contended_env_lock() => read(),
        Err(TryLockError::WouldBlock) => {
            let _guard = match env_lock().lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            read()
        }
    }
}

fn current_thread_holds_test_env_lock() -> bool {
    match env_lock().try_lock() {
        Ok(guard) => {
            drop(guard);
            false
        }
        Err(TryLockError::Poisoned(poisoned)) => {
            drop(poisoned.into_inner());
            false
        }
        Err(TryLockError::WouldBlock) => current_thread_owns_contended_env_lock(),
    }
}

/// Restore one environment variable when dropped.
///
/// Callers that mutate process-global environment variables must hold
/// [`lock_test_env`] until after this guard is dropped.
pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

impl EnvVarGuard {
    pub(crate) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        debug_assert!(
            current_thread_holds_test_env_lock(),
            "EnvVarGuard::set({key}) requires lock_test_env()"
        );
        let previous = std::env::var_os(key);
        // SAFETY: callers hold the process-wide test env mutex.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }

    pub(crate) fn remove(key: &'static str) -> Self {
        debug_assert!(
            current_thread_holds_test_env_lock(),
            "EnvVarGuard::remove({key}) requires lock_test_env()"
        );
        let previous = std::env::var_os(key);
        // SAFETY: callers hold the process-wide test env mutex.
        unsafe { std::env::remove_var(key) };
        Self { key, previous }
    }

    pub(crate) fn previous(&self) -> Option<OsString> {
        self.previous.clone()
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: callers hold the process-wide test env mutex until after this
        // guard is dropped.
        unsafe {
            if let Some(value) = self.previous.take() {
                std::env::set_var(self.key, value);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

/// Find the byte position of the first divergence between two strings,
/// returning a windowed view (`±32 bytes` around the divergence) so failures
/// in cache-prefix-stability tests show *which* bytes drifted, not just that
/// they did. Returns `None` when the strings are byte-identical.
pub(crate) fn first_divergence(a: &str, b: &str) -> Option<(usize, String, String)> {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let max = a_bytes.len().min(b_bytes.len());
    for i in 0..max {
        if a_bytes[i] != b_bytes[i] {
            let lo = i.saturating_sub(32);
            let a_hi = (i + 32).min(a_bytes.len());
            let b_hi = (i + 32).min(b_bytes.len());
            let a_ctx = String::from_utf8_lossy(&a_bytes[lo..a_hi]).into_owned();
            let b_ctx = String::from_utf8_lossy(&b_bytes[lo..b_hi]).into_owned();
            return Some((i, a_ctx, b_ctx));
        }
    }
    if a_bytes.len() != b_bytes.len() {
        return Some((
            max,
            format!("(len={})", a_bytes.len()),
            format!("(len={})", b_bytes.len()),
        ));
    }
    None
}

/// Assert two strings are byte-identical, panicking with a windowed diff
/// around the first divergence when they aren't. Used by the prefix-cache
/// stability harness (#263, #280) to pin construction surfaces that land in
/// DeepSeek's KV cache prefix.
#[track_caller]
pub(crate) fn assert_byte_identical(label: &str, a: &str, b: &str) {
    if let Some((pos, a_ctx, b_ctx)) = first_divergence(a, b) {
        panic!(
            "{label}: prompt construction is non-deterministic — first diff at byte {pos}\n\
             ── side A (±32B) ──\n{a_ctx:?}\n── side B (±32B) ──\n{b_ctx:?}",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn config_path_read_waits_for_foreign_env_redirect_to_restore() {
        let (tx, rx) = mpsc::channel();
        let redirected = std::env::temp_dir().join(format!(
            "codewhale-config-path-read-barrier-{}",
            std::process::id()
        ));

        let reader = {
            let lock = lock_test_env();
            let redirect = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &redirected);
            let reader = std::thread::spawn(move || {
                tx.send(crate::config_persistence::config_toml_path(None))
                    .expect("send resolved config path");
            });

            assert!(
                rx.recv_timeout(Duration::from_millis(50)).is_err(),
                "a foreign reader observed the temporary config redirect"
            );
            drop(redirect);
            drop(lock);
            reader
        };

        let resolved = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("reader resumed after the redirect was restored")
            .expect("resolve config path");
        reader.join().expect("reader thread");
        assert_ne!(resolved, redirected);
    }
}

// ── Shared App/TuiOptions fixtures (#3923) ──────────────────────────────
//
// Before this module owned them, `create_test_app` was copy-pasted across 28
// test modules, each spelling out the full `TuiOptions` literal — 87 literals
// in all. The copies had drifted: different modules pinned different locales,
// currencies, and onboarding flags without anyone having chosen that, which is
// the non-hermeticity behind the intermittent `config_command_allow_shell_*`
// failures. Adding a `TuiOptions` field meant editing up to 87 sites.
//
// Express intentional differences by mutating the returned value at the call
// site, so the difference is visible as a deliberate line of test code rather
// than hidden inside another near-identical literal.

use std::path::{Path, PathBuf};

/// Default `TuiOptions` for tests, pinned to the deepseek-v4-pro fixture route.
pub(crate) fn test_tui_options(workspace: impl AsRef<Path>) -> crate::tui::app::TuiOptions {
    let workspace = workspace.as_ref().to_path_buf();
    crate::tui::app::TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace,
        config_path: None,
        config_profile: None,
        allow_shell: false,
        use_alt_screen: true,
        use_mouse_capture: false,
        use_bracketed_paste: true,
        max_subagents: 1,
        skills_dir: PathBuf::from("."),
        memory_path: PathBuf::from("memory.md"),
        notes_path: PathBuf::from("notes.txt"),
        mcp_config_path: PathBuf::from("mcp.json"),
        use_memory: false,
        // Majority-of-fixtures defaults, measured across the 89 literals this
        // helper replaced. Modules that need the other value say so explicitly.
        start_in_agent_mode: false,
        skip_onboarding: true,
        yolo: false,
        resume_session_id: None,
        initial_input: None,
    }
}

/// Build an `App` whose observable state does not depend on the developer's
/// machine.
///
/// `App::new` consults real persisted settings (provider/model maps,
/// auto-model, route limits, locale, currency), so an un-pinned fixture
/// computes against whatever the developer last configured. Every pin below
/// exists because some test was observed to depend on it.
pub(crate) fn test_app_with_options(options: crate::tui::app::TuiOptions) -> crate::tui::app::App {
    let config = crate::config::Config::default();
    let mut app = crate::tui::app::App::new(options, &config);

    // Deterministic presentation regardless of host locale.
    app.cost_currency = crate::pricing::CostCurrency::Usd;
    app.ui_locale = crate::localization::Locale::En;
    // Transcript tests must not depend on a concurrently swapped settings
    // home. Tests for hidden reasoning opt out explicitly.
    app.show_thinking = true;
    // Pin the route identity: without this, a machine with customized
    // settings computes context-window assertions against a different model
    // than the requested deepseek-v4-pro.
    app.set_provider_identity(crate::config::ApiProvider::Deepseek, "deepseek");
    app.billing_presentation = crate::route_billing::BillingPresentation::Metered;
    app.model = "deepseek-v4-pro".to_string();
    app.auto_model = false;
    app.last_effective_model = None;
    app.active_route_limits = None;
    app.active_context_window_override = None;
    // Fixtures replace `app.workspace` freely. Do not retain `App::new`'s real
    // process cwd as a second discovery root: parallel tests and a large
    // developer checkout can otherwise consume the bounded mention index
    // before the fixture workspace is scanned.
    app.composer.mention_cwd = None;
    app
}
