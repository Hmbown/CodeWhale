//! Consolidated harness for plain `#[test]`/`#[tokio::test]` acceptance suites.
//!
//! See `crates/tui/tests/README.md` for why this exists: 17 small integration
//! binaries each re-linked the full `codewhale-tui` graph — one harness keeps the
//! same test names (`integration::adaptive_evidence_acceptance::...`) so
//! `cargo test -p codewhale-tui adaptive_evidence_acceptance` still filters.

// Production modules that are `#[path]`-included by the test files below and
// that themselves use `crate::`. They must exist at the harness crate root so
// `crate::config`, `crate::shell_dispatcher`, etc. resolve when the same
// files are compiled as `crate::integration::<test>::<module>`.

// Shim for `crate::test_support` used by `palette`'s inline tests when compiled
// as part of the integration harness. Mirrors the shim that used to live inside
// `palette_audit.rs` when it was a standalone binary.
#[allow(dead_code)]
mod test_support {
    use std::ffi::{OsStr, OsString};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    fn lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(Mutex::default)
    }

    pub(super) struct TestEnvLock {
        _guard: MutexGuard<'static, ()>,
    }

    pub(super) fn lock_test_env() -> TestEnvLock {
        let guard = match lock().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        TestEnvLock { _guard: guard }
    }

    pub(super) struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        pub(super) fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let previous = std::env::var_os(key);
            // SAFETY: callers hold the process-wide test env mutex.
            unsafe { std::env::set_var(key, value) };
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: callers hold the process-wide test env mutex until drop.
            unsafe {
                if let Some(value) = self.previous.take() {
                    std::env::set_var(self.key, value);
                } else {
                    std::env::remove_var(self.key);
                }
            }
        }
    }
}

#[path = "../../src/config/home.rs"]
#[allow(dead_code)]
mod config;
#[path = "../../src/eval.rs"]
mod eval;
#[path = "../../src/skills/install.rs"]
#[allow(dead_code)]
mod install;
#[path = "../support/llm_client.rs"]
mod llm_client;
#[path = "../../src/model_catalog.rs"]
mod model_catalog;
#[path = "../../src/models.rs"]
#[allow(dead_code)]
mod models;
#[path = "../../src/network_policy.rs"]
mod network_policy;
#[path = "../../src/skills/package_digest.rs"]
#[allow(dead_code)]
mod package_digest;
#[path = "../../src/palette/mod.rs"]
#[allow(dead_code)]
mod palette;
#[path = "../../src/shell_dispatcher.rs"]
mod shell_dispatcher;
// The legacy text tool-call parser lives in codewhale-core now; keep the
// `crate::tool_parser` path the suites use.
use codewhale_core::tool_parser;

mod adaptive_evidence_acceptance;
mod cache_guard;
mod coordination_acceptance;
mod diagnostic_read_only;
mod dotenv_authority;
mod eval_harness;
mod exec_persistent_service;
mod exec_stream_drop_acceptance;
mod exec_turn_usage;
mod integration_mock_llm;
mod lifecycle_outbox_exec;
mod palette_audit;
mod protocol_recovery;
mod reasoning_content_replayed_after_tool_call;
mod skill_cli;
mod telemetry_contract;
mod verifiers_harness_contract;
mod workflow_tool_stream_acceptance;
