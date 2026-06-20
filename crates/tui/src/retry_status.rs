//! Process-wide retry-state surface (#499).
//!
//! The HTTP retry path in `client::send_with_retry` already times its
//! waits and knows the error category. This module gives the TUI a way
//! to observe that state — `start`, `succeeded`, and `failed` flip a
//! global `RetryState` that the footer / status panel reads each frame.
//!
//! Why a process-wide global: the user-facing TUI runs as one engine
//! per process, and the only retry state we want to surface is the one
//! the user is staring at. Sub-agent retries in background tasks
//! deliberately do **not** light up the foreground banner — they're
//! supposed to be invisible. If a future feature ever needs per-engine
//! retry surfaces, swap this for an `Arc<RwLock<...>>` carried on the
//! `EngineHandle`; the public API stays the same.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One in-flight retry attempt. `deadline` is the wall-clock time the
/// next request will fire — the UI subtracts `Instant::now()` from it
/// to render a live countdown.
#[derive(Debug, Clone)]
pub struct RetryBanner {
    /// 1-indexed retry attempt number (the first retry is attempt 1).
    pub attempt: u32,
    /// Time at which the next request will be sent.
    pub deadline: Instant,
    /// Short human-readable reason ("rate limited", "server error", …).
    pub reason: String,
}

/// Snapshot of the retry surface for the UI to render.
#[derive(Debug, Clone, Default)]
pub enum RetryState {
    /// No retry in flight. Banner hidden.
    #[default]
    Idle,
    /// A request is sleeping before retrying. Show countdown banner.
    Active(RetryBanner),
    /// All retries exhausted; show failure row until the next turn
    /// starts. `since` records when the row was set so a future polish
    /// pass can age it out automatically; today the engine clears it on
    /// `TurnStarted`.
    Failed {
        reason: String,
        #[allow(dead_code)]
        since: Instant,
    },
}

impl RetryState {
    /// Wall-clock seconds remaining on the active banner, or `None` if
    /// not active. Saturates at zero — the renderer should treat any
    /// negative remaining as "firing now".
    #[must_use]
    pub fn seconds_remaining(&self) -> Option<u64> {
        match self {
            Self::Active(banner) => Some(
                banner
                    .deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs(),
            ),
            _ => None,
        }
    }

    /// Whether the failure row should still be shown. Mirrors the
    /// "until next turn" rule in the issue spec; the engine clears it
    /// explicitly via [`clear`] on `TurnStarted`.
    #[cfg(test)]
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }
}

/// Lazy-init the cell on first read so callers don't have to initialize
/// process-wide state at boot.
fn cell() -> &'static Mutex<RetryState> {
    static STATE: OnceLock<Mutex<RetryState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RetryState::Idle))
}

fn rate_limit_cell() -> &'static Mutex<Option<Instant>> {
    static STATE: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// Public read snapshot for renderers.
#[must_use]
pub fn snapshot() -> RetryState {
    cell().lock().map(|s| s.clone()).unwrap_or(RetryState::Idle)
}

/// Extend the provider-wide rate-limit pause window. This is separate from
/// the footer banner so one successful concurrent request cannot clear another
/// request's active `Retry-After` window.
pub fn note_rate_limit(delay: Duration) {
    let deadline = Instant::now() + delay;
    if let Ok(mut current) = rate_limit_cell().lock()
        && current.is_none_or(|existing| existing < deadline)
    {
        *current = Some(deadline);
    }
}

/// Remaining provider-wide rate-limit pause, if any.
#[must_use]
pub fn rate_limit_remaining() -> Option<Duration> {
    let now = Instant::now();
    let mut current = rate_limit_cell().lock().ok()?;
    match *current {
        Some(deadline) if deadline > now => Some(deadline.duration_since(now)),
        Some(_) => {
            *current = None;
            None
        }
        None => None,
    }
}

/// Mark an in-flight retry. `attempt` is the number of the *upcoming*
/// retry (1 for the first); `delay` is how long the client will sleep
/// before firing.
pub fn start(attempt: u32, delay: Duration, reason: impl Into<String>) {
    let banner = RetryBanner {
        attempt,
        deadline: Instant::now() + delay,
        reason: reason.into(),
    };
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Active(banner);
    }
}

/// Mark the retry chain as having succeeded. Hides the banner.
pub fn succeeded() {
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Idle;
    }
}

/// Mark the retry chain as having exhausted retries. The renderer keeps
/// the failure row until [`clear`] (typically called on `TurnStarted`).
pub fn failed(reason: impl Into<String>) {
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Failed {
            reason: reason.into(),
            since: Instant::now(),
        };
    }
}

/// Reset to idle. Called on `TurnStarted` so the previous turn's
/// failure row doesn't bleed into the next turn.
pub fn clear() {
    if let Ok(mut s) = cell().lock() {
        *s = RetryState::Idle;
    }
}

#[cfg(test)]
pub fn clear_rate_limit() {
    if let Ok(mut current) = rate_limit_cell().lock() {
        *current = None;
    }
}

/// Test helper: serialize tests that touch the global state so cargo's
/// parallel runner can't observe a torn read. The guard is exported so
/// tests in *other* modules (e.g. footer rendering tests) can hold the
/// same lock as the ones in `retry_status::tests`.
#[cfg(test)]
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: Mutex<()> = Mutex::new(());
    GUARD.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Acquire the cross-module test guard from [`super::test_guard`] and
    /// reset state to `Idle` before yielding to the test body.
    fn setup() -> std::sync::MutexGuard<'static, ()> {
        let g = test_guard();
        clear();
        clear_rate_limit();
        g
    }

    #[test]
    fn idle_by_default_after_clear() {
        let _g = setup();
        assert!(matches!(snapshot(), RetryState::Idle));
        assert_eq!(snapshot().seconds_remaining(), None);
    }

    #[test]
    fn start_then_succeeded_returns_to_idle() {
        let _g = setup();
        start(1, Duration::from_secs(5), "rate limited");
        let s = snapshot();
        assert!(matches!(s, RetryState::Active(_)));
        let remaining = s.seconds_remaining().unwrap();
        assert!(remaining <= 5, "{remaining}");
        succeeded();
        assert!(matches!(snapshot(), RetryState::Idle));
    }

    #[test]
    fn failed_persists_until_clear() {
        let _g = setup();
        failed("upstream 500");
        let s = snapshot();
        assert!(s.is_failed());
        if let RetryState::Failed { reason, .. } = s {
            assert_eq!(reason, "upstream 500");
        } else {
            panic!("expected Failed");
        }
        clear();
        assert!(matches!(snapshot(), RetryState::Idle));
    }

    #[test]
    fn deadline_in_past_yields_zero_remaining() {
        let _g = setup();
        // Bypass `start` so we can plant a deadline already in the past.
        if let Ok(mut s) = cell().lock() {
            *s = RetryState::Active(RetryBanner {
                attempt: 2,
                deadline: Instant::now() - Duration::from_secs(1),
                reason: "test".into(),
            });
        }
        assert_eq!(snapshot().seconds_remaining(), Some(0));
        clear();
    }

    #[test]
    fn rate_limit_deadline_survives_banner_clear() {
        let _g = setup();
        note_rate_limit(Duration::from_secs(5));
        start(1, Duration::from_secs(5), "rate limited");
        succeeded();
        assert!(
            rate_limit_remaining().is_some(),
            "provider-wide rate limit pause must not be cleared by an unrelated success"
        );
        clear_rate_limit();
    }
}
