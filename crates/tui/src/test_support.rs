//! Shared test-only helpers.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) use crate::shell_dispatcher::test_env_lock::{
    EnvScopeMembership, EnvScopeTicket, TestEnvLock, current_env_scope_generation,
    current_thread_holds_test_env_lock, env_scope_ticket, join_env_scope, lock_test_env,
    with_test_env_lock,
};

/// Process-wide state root for unit tests that do not intentionally provide an
/// explicit config/settings path.
///
/// The production fallback is the user's real home. That is useful at runtime
/// and unsafe in a parallel test binary: an unguarded save can otherwise read
/// or overwrite the developer's config. Tests that exercise path precedence
/// still hold [`lock_test_env`] and provide explicit temporary environment
/// values; every other test is confined here — enforced by
/// [`guarded_environment_provides_state_paths`], not assumed.
pub(crate) fn isolated_test_state_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "codewhale-tui-test-state-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap_or_else(|error| {
            panic!(
                "failed to create isolated unit-test state root {}: {error}",
                root.display()
            )
        });
        root
    })
}

/// Where the calling test's state should live when it has not sealed the
/// environment itself.
///
/// Two different callers land here. A test that never took [`lock_test_env`]
/// gets the shared root, exactly as before — those tests already coexist there
/// under [`with_test_state_io_lock`]. A test that *holds* the lock but sealed
/// nothing gets a private directory instead: before #5359 it resolved the
/// developer's real home, so it has never shared the process root, and several
/// such tests run full settings transactions. Adding that traffic to the shared
/// root pushed the transaction lock past its deadline and hung unrelated
/// `config_command_*` tests. Keep them isolated from the developer *and* from
/// each other.
pub(crate) fn unsealed_test_state_root() -> PathBuf {
    let shared = isolated_test_state_root();
    if !current_thread_holds_test_env_lock() {
        return shared.to_path_buf();
    }
    // libtest runs each test in a fresh thread. Keep one root for that thread:
    // a settings save resolves its path more than once, while different tests
    // must not inherit each other's files.
    HOLDER_ROOT.with(|cached| {
        cached
            .get_or_init(|| {
                let root = shared.join(format!("env-holder-{:?}", std::thread::current().id()));
                std::fs::create_dir_all(&root).unwrap_or_else(|error| {
                    panic!(
                        "failed to create per-holder test state root {}: {error}",
                        root.display()
                    )
                });
                root
            })
            .clone()
    })
}

thread_local! {
    static HOLDER_ROOT: OnceLock<PathBuf> = const { OnceLock::new() };
}

/// Build a syntactically valid, non-secret JWT fixture without embedding a
/// high-entropy token-shaped literal in Git history.
pub(crate) fn future_test_jwt(label: &str) -> String {
    use base64::Engine as _;

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":9999999999}"#);
    format!("test.{payload}.{label}")
}

fn state_io_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Serialize read/merge/write operations against the process-wide isolated
/// test state root.
///
/// Path isolation protects the developer's files, but parallel tests still
/// share the same temporary files. Settings persistence is a multi-step
/// operation, so it needs this second barrier around the complete I/O
/// transaction rather than only around path resolution.
pub(crate) fn with_test_state_io_lock<T>(operation: impl FnOnce() -> T) -> T {
    let _guard = match state_io_lock().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    operation()
}

/// Restore one environment variable when dropped.
///
/// Callers that mutate process-global environment variables must hold
/// [`lock_test_env`] until after this guard is dropped.
///
/// Every live guard is also recorded in [`guarded_env_keys`], so path
/// resolution can distinguish a test that deliberately redirected `HOME`
/// from one that merely holds the lock to serialize unrelated env access —
/// see [`guarded_environment_provides_state_paths`].
pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<OsString>,
}

fn guarded_env_keys() -> &'static Mutex<std::collections::HashMap<&'static str, usize>> {
    static KEYS: OnceLock<Mutex<std::collections::HashMap<&'static str, usize>>> = OnceLock::new();
    KEYS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn register_guarded_env_key(key: &'static str) {
    let mut keys = match guarded_env_keys().lock() {
        Ok(keys) => keys,
        Err(poisoned) => poisoned.into_inner(),
    };
    *keys.entry(key).or_insert(0) += 1;
}

fn unregister_guarded_env_key(key: &'static str) {
    let mut keys = match guarded_env_keys().lock() {
        Ok(keys) => keys,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(count) = keys.get_mut(key) {
        *count -= 1;
        if *count == 0 {
            keys.remove(key);
        }
    }
}

/// Whether some live [`EnvVarGuard`] currently covers `key`.
pub(crate) fn env_var_currently_guarded(key: &str) -> bool {
    match guarded_env_keys().lock() {
        Ok(keys) => keys.contains_key(key),
        Err(poisoned) => poisoned.into_inner().contains_key(key),
    }
}

/// Whether the calling test actually provided the state-path environment it
/// is about to resolve.
///
/// Holding [`lock_test_env`] alone is not that: many tests hold the lock only
/// to serialize access to unrelated variables (`TERM_PROGRAM`, API keys) and
/// have provided no temporary paths at all. Trusting the lock routed those
/// tests to the developer's real `~/.codewhale` state, which is exactly the
/// leak the isolated root exists to prevent (#5359). A test earns environment
/// resolution by holding the lock *and* either setting one of the explicit
/// override variables or redirecting `HOME`/`USERPROFILE` through
/// [`EnvVarGuard`].
pub(crate) fn guarded_environment_provides_state_paths() -> bool {
    if !current_thread_holds_test_env_lock() {
        return false;
    }
    let guarded_path_is_present = |var: &str| {
        env_var_currently_guarded(var)
            && std::env::var_os(var)
                .is_some_and(|value| value.to_str().is_none_or(|text| !text.trim().is_empty()))
    };
    [
        "CODEWHALE_HOME",
        "CODEWHALE_CONFIG_PATH",
        "DEEPSEEK_CONFIG_PATH",
        "HOME",
        "USERPROFILE",
    ]
    .iter()
    .any(|var| guarded_path_is_present(var))
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
        register_guarded_env_key(key);
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
        register_guarded_env_key(key);
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
        unregister_guarded_env_key(self.key);
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

/// Default `TuiOptions` for tests, pinned to the deepseek-v4-pro fixture route.
pub(crate) fn test_tui_options(workspace: impl AsRef<Path>) -> crate::tui::app::TuiOptions {
    let workspace = workspace.as_ref().to_path_buf();
    crate::tui::app::TuiOptions {
        model: "deepseek-v4-pro".to_string(),
        workspace,
        config_path: None,
        config_profile: None,
        allow_shell: false,
        screen_mode: crate::tui::app::ScreenMode::Fullscreen,
        use_mouse_capture: false,
        mouse_capture_preference: false,
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
        startup_notice: None,
    }
}

/// Build an `App` whose observable state does not depend on the developer's
/// machine.
///
/// `App::new` consults real persisted settings (provider/model maps,
/// auto-model, route limits, locale, currency), so an un-pinned fixture
/// computes against whatever the developer last configured. Every pin below
/// exists because some test was observed to depend on it. This fixture models
/// a session after the user has chosen a Startup action; direct `App::new`
/// tests remain the clean-launch authority.
pub(crate) fn test_app_with_options(options: crate::tui::app::TuiOptions) -> crate::tui::app::App {
    let config = crate::config::Config::default();
    let mut app = crate::tui::app::App::new(options, &config);

    // Shared behavior tests operate on the live session surface. Do not make
    // the production startup conditional for them: clean launches are covered
    // by direct `App::new` tests that retain the Tideline Startup Hero.
    app.launch.visible = false;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn ambient_codewhale_home_is_not_a_test_seal() {
        let _lock = lock_test_env();
        let _ambient = EnvVarGuard::set("CODEWHALE_HOME", "/tmp/ambient-codewhale-home");
        unregister_guarded_env_key("CODEWHALE_HOME");

        let sealed = guarded_environment_provides_state_paths();

        register_guarded_env_key("CODEWHALE_HOME");
        assert!(!sealed, "ambient developer state must remain confined");
    }

    #[test]
    fn removing_overrides_does_not_seal_the_ambient_home() {
        let _lock = lock_test_env();
        let _codewhale_home = EnvVarGuard::remove("CODEWHALE_HOME");
        let _codewhale_config = EnvVarGuard::remove("CODEWHALE_CONFIG_PATH");
        let _deepseek_config = EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH");

        assert!(
            !guarded_environment_provides_state_paths(),
            "removing an override must not expose the developer's HOME"
        );
    }

    #[test]
    fn removing_home_variables_does_not_seal_a_missing_path() {
        let _lock = lock_test_env();
        let _home = EnvVarGuard::remove("HOME");
        let _userprofile = EnvVarGuard::remove("USERPROFILE");

        assert!(
            !guarded_environment_provides_state_paths(),
            "removing HOME variables must keep state in the isolated test root"
        );
        assert_eq!(
            crate::config_persistence::config_toml_path(None)
                .expect("resolve isolated config path"),
            unsealed_test_state_root().join(codewhale_config::CONFIG_FILE_NAME)
        );
    }

    #[test]
    fn lock_without_sealed_paths_does_not_use_developer_config() {
        let _lock = lock_test_env();
        let path = crate::config_persistence::config_toml_path(None)
            .expect("resolve isolated config path");
        let root = isolated_test_state_root();
        assert!(
            path.starts_with(root),
            "holding lock_test_env without an EnvVarGuard must not read ~/.codewhale ({})",
            path.display()
        );
        assert_eq!(
            path,
            unsealed_test_state_root().join(codewhale_config::CONFIG_FILE_NAME)
        );
    }

    #[test]
    fn unguarded_state_writes_use_isolated_test_root() {
        const PROBE_ENV: &str = "CODEWHALE_TEST_STATE_ISOLATION_PROBE";
        const RECEIPT_ENV: &str = "CODEWHALE_TEST_STATE_ISOLATION_RECEIPT";

        if std::env::var_os(PROBE_ENV).is_some() {
            let config_path =
                crate::config_persistence::persist_root_bool_key(None, "allow_shell", true)
                    .expect("write isolated config");
            let direct_config_path =
                crate::config::save_workspace_trust(Path::new("/tmp/codewhale-test-workspace"))
                    .expect("write through direct default config path");
            crate::settings::Settings::default()
                .save()
                .expect("write isolated settings");
            let settings_path =
                crate::settings::Settings::path().expect("resolve isolated settings");
            let root = isolated_test_state_root();
            assert!(config_path.starts_with(root), "{}", config_path.display());
            assert!(
                settings_path.starts_with(root),
                "{}",
                settings_path.display()
            );
            assert!(
                direct_config_path.starts_with(root),
                "{}",
                direct_config_path.display()
            );
            let receipt = std::env::var_os(RECEIPT_ENV).expect("receipt path");
            std::fs::write(
                receipt,
                format!(
                    "{}\n{}\n{}\n{}\n",
                    root.display(),
                    config_path.display(),
                    settings_path.display(),
                    direct_config_path.display()
                ),
            )
            .expect("write isolation receipt");
            return;
        }

        let sentinel = tempfile::tempdir().expect("sentinel home");
        let user_state = sentinel.path().join(".codewhale");
        std::fs::create_dir_all(&user_state).expect("create sentinel state");
        let config_path = user_state.join("config.toml");
        let settings_path = user_state.join("settings.toml");
        let config_sentinel = b"# developer config sentinel\n";
        let settings_sentinel = b"# developer settings sentinel\n";
        std::fs::write(&config_path, config_sentinel).expect("seed config");
        std::fs::write(&settings_path, settings_sentinel).expect("seed settings");
        let receipt_path = sentinel.path().join("receipt.txt");

        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .arg("--exact")
            .arg("test_support::tests::unguarded_state_writes_use_isolated_test_root")
            .arg("--test-threads=1")
            .env(PROBE_ENV, "1")
            .env(RECEIPT_ENV, &receipt_path)
            .env("HOME", sentinel.path())
            .env("USERPROFILE", sentinel.path())
            .env_remove("CODEWHALE_HOME")
            .env_remove("CODEWHALE_CONFIG_PATH")
            .env_remove("DEEPSEEK_CONFIG_PATH")
            .output()
            .expect("run isolated-state probe");
        assert!(
            output.status.success(),
            "probe failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        assert_eq!(
            std::fs::read(&config_path).expect("read config sentinel"),
            config_sentinel
        );
        assert_eq!(
            std::fs::read(&settings_path).expect("read settings sentinel"),
            settings_sentinel
        );

        let receipt = std::fs::read_to_string(&receipt_path).expect("read isolation receipt");
        let mut paths = receipt.lines().map(PathBuf::from);
        let isolated_root = paths.next().expect("root receipt");
        let written_config = paths.next().expect("config receipt");
        let written_settings = paths.next().expect("settings receipt");
        let direct_config = paths.next().expect("direct config receipt");
        assert!(!isolated_root.starts_with(sentinel.path()));
        assert!(written_config.starts_with(&isolated_root));
        assert!(written_settings.starts_with(&isolated_root));
        assert!(direct_config.starts_with(&isolated_root));
        assert!(written_config.exists());
        assert!(written_settings.exists());
    }

    #[test]
    fn config_path_read_waits_for_foreign_env_redirect_to_restore() {
        let (started_tx, started_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let redirected = std::env::temp_dir().join(format!(
            "codewhale-config-path-read-barrier-{}",
            std::process::id()
        ));

        let reader = {
            let lock = lock_test_env();
            let redirect = EnvVarGuard::set("DEEPSEEK_CONFIG_PATH", &redirected);
            let reader = std::thread::spawn(move || {
                started_tx.send(()).expect("signal config path read start");
                tx.send(crate::config_persistence::config_toml_path(None))
                    .expect("send resolved config path");
            });

            started_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("reader reached config path resolution");
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

    #[test]
    fn settings_save_waits_for_foreign_state_io_transaction() {
        let (holder_ready_tx, holder_ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            with_test_state_io_lock(|| {
                holder_ready_tx.send(()).expect("signal state lock held");
                release_rx.recv().expect("release state lock");
            });
        });
        holder_ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("holder acquired state I/O lock");

        let (started_tx, started_rx) = mpsc::channel();
        let (saved_tx, saved_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            started_tx.send(()).expect("signal settings save start");
            saved_tx
                .send(crate::settings::Settings::default().save())
                .expect("send settings save result");
        });
        started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("writer reached settings save");
        assert!(
            saved_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "settings save did not wait for an in-flight state transaction"
        );

        release_tx.send(()).expect("release holder");
        holder.join().expect("holder thread");
        saved_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("settings save resumed")
            .expect("settings save succeeded");
        writer.join().expect("writer thread");
    }
}
