//! IDE bridge integration — discovers and connects to an MCP-over-WS
//! bridge published by an IDE host (VS Code via Claude Code extension,
//! Cursor, Zed, etc.).
//!
//! Behavioural shape mirrors opencode's editor-context client:
//! * exponential backoff retry that never gives up;
//! * `reconnect(Option<PathBuf>)` so callers can re-resolve the lockfile
//!   after `cd` into a different project;
//! * cwd-scoped lockfile selection (via `deepseek_ide_bridge::discover_for`)
//!   so unrelated IDE windows don't leak their selection into prompts;
//! * a SolidJS-style "selection changed" signal exposed as
//!   [`IdeBridgeHandle::take_dirty`] — the main UI loop reads it once per
//!   tick and flips `needs_redraw` to true on change, so editor selection
//!   updates surface in the footer / prompt block immediately rather than
//!   waiting for the next unrelated event.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use deepseek_ide_bridge::{
    BridgeTarget, ConnectOptions, IdeBridgeClient, SelectionChange, WorkspaceFolders,
};
use tokio::sync::{Notify, OnceCell};

static GLOBAL: OnceCell<IdeBridgeHandle> = OnceCell::const_new();

/// Initial reconnect delay; doubles each failure up to `BACKOFF_MAX`.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// After this many consecutive failed connect attempts the worker stops
/// burning CPU/IO on a host that clearly has no IDE bridge running and
/// parks until a `reconnect()` (e.g. user `cd`-ed into a project that
/// does have one) wakes it back up. Roughly aligns with one minute of
/// retries at the 10 s ceiling.
const MAX_FAILURES_BEFORE_PARK: u32 = 8;

#[derive(Default)]
struct State {
    client: Option<IdeBridgeClient>,
    directory: Option<PathBuf>,
    /// Aborts the per-connection selection-watch task when the client is
    /// torn down (reconnect, shutdown). Keyed off `OnceCell` semantics so
    /// only one task can ever be live at a time.
    watcher: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    state: RwLock<State>,
    /// Monotonic counter that advances every time the IDE pushes a new
    /// selection. The UI loop calls [`IdeBridgeHandle::take_dirty`] once
    /// per tick and triggers a redraw on change. Atomic + relaxed because
    /// missing an update by a single tick (~24 ms) is harmless and we
    /// never need a happens-before with other state.
    selection_seq: AtomicU64,
    /// Last value the UI consumed via `take_dirty`. Stored alongside
    /// `selection_seq` so all IDE-bridge state lives in one struct.
    selection_seq_seen: AtomicU64,
    /// Pulsed by `reconnect()` to wake the single connect worker. The
    /// worker parks on this `Notify` whenever (a) it has hit the failure
    /// ceiling and is in passive mode, or (b) it just tore down a live
    /// connection because the directory changed. Coalesces N concurrent
    /// `reconnect()` calls into a single re-resolve.
    wake: Notify,
}

#[derive(Clone, Default)]
pub struct IdeBridgeHandle {
    inner: Arc<Inner>,
}

#[allow(dead_code)]
impl IdeBridgeHandle {
    pub fn instance() -> Option<&'static Self> {
        GLOBAL.get()
    }

    pub async fn init() -> &'static Self {
        GLOBAL
            .get_or_init(|| async {
                let handle = Self::default();
                handle.set_directory(std::env::current_dir().ok());
                handle.spawn_worker();
                handle
            })
            .await
    }

    pub fn is_connected(&self) -> bool {
        self.inner
            .state
            .read()
            .ok()
            .is_some_and(|s| s.client.is_some())
    }

    pub fn ide_name(&self) -> Option<String> {
        let guard = self.inner.state.read().ok()?;
        guard.client.as_ref()?.ide_name().map(str::to_string)
    }

    pub fn latest_selection(&self) -> Option<SelectionChange> {
        let guard = self.inner.state.read().ok()?;
        guard.client.as_ref()?.latest_selection()
    }

    /// Borrow the cached selection without cloning. Returns `None` when
    /// no client is connected or the cache is empty. Closure runs while
    /// a read lock is held — keep it fast and synchronous.
    pub fn with_latest_selection<R>(&self, f: impl FnOnce(&SelectionChange) -> R) -> Option<R> {
        let guard = self.inner.state.read().ok()?;
        guard.client.as_ref()?.with_latest_selection(f)
    }

    pub async fn current_selection(&self) -> Option<SelectionChange> {
        let client = self.inner.state.read().ok()?.client.as_ref()?.clone();
        client.get_current_selection().await.ok()
    }

    pub async fn workspace_folders(&self) -> Option<WorkspaceFolders> {
        let client = self.inner.state.read().ok()?.client.as_ref()?.clone();
        client.get_workspace_folders().await.ok()
    }

    pub async fn shutdown(&self) {
        let (client, watcher) = if let Ok(mut s) = self.inner.state.write() {
            (s.client.take(), s.watcher.take())
        } else {
            (None, None)
        };
        if let Some(w) = watcher {
            w.abort();
        }
        if let Some(c) = client {
            c.shutdown().await;
        }
    }

    /// Returns true once after every selection update from the IDE. The
    /// UI loop calls this once per tick: if it returns true, flip
    /// `needs_redraw` so the footer chip / `<editor_context>` block
    /// reflect the new selection on the very next frame.
    pub fn take_dirty(&self) -> bool {
        let current = self.inner.selection_seq.load(Ordering::Relaxed);
        let previous = self
            .inner
            .selection_seq_seen
            .swap(current, Ordering::Relaxed);
        current != previous
    }

    /// Re-resolve the IDE bridge against a new working directory.
    /// Pass `None` to use `process::current_dir()`.
    ///
    /// Cheap and idempotent: just records the new directory and pulses
    /// the long-lived worker. Multiple rapid `reconnect()` calls collapse
    /// to a single re-resolve, and a parked worker (one that has given
    /// up retrying because no IDE was around) wakes up.
    pub fn reconnect(&self, directory: Option<PathBuf>) {
        let new_dir = directory.or_else(|| std::env::current_dir().ok());
        let same = self
            .inner
            .state
            .read()
            .ok()
            .and_then(|s| s.directory.clone())
            .as_deref()
            == new_dir.as_deref();
        if same {
            return;
        }
        self.set_directory(new_dir);
        // If we're already connected, we need to tear down the live
        // client first so the worker re-resolves against the new
        // directory. `tear_down_for_reconnect` pulses `wake` for us.
        // Otherwise the worker is either retrying or parked — pulsing
        // `wake` directly is enough.
        if self.is_connected() {
            let handle = self.clone();
            tokio::spawn(async move { handle.tear_down_for_reconnect().await });
        } else {
            self.inner.wake.notify_one();
        }
    }

    fn set_directory(&self, directory: Option<PathBuf>) {
        if let Ok(mut s) = self.inner.state.write() {
            s.directory = directory;
        }
    }

    fn current_directory(&self) -> Option<PathBuf> {
        self.inner.state.read().ok()?.directory.clone()
    }

    fn bump_selection_seq(&self) {
        self.inner.selection_seq.fetch_add(1, Ordering::Relaxed);
    }

    /// Spawn the **single** long-running connect/retry worker. Replaces
    /// the previous "spawn-on-every-reconnect" model that could leak
    /// tasks blocked on a serialising mutex when no IDE was available.
    fn spawn_worker(&self) {
        let handle = self.clone();
        tokio::spawn(async move {
            handle.run_worker().await;
        });
    }

    /// Single-task event loop:
    ///   - if disconnected: try to connect; on success, park until torn
    ///     down (directory change → `reconnect()` calls `tear_down`);
    ///   - on failure: exponential backoff, capped by `BACKOFF_MAX` and
    ///     `MAX_FAILURES_BEFORE_PARK`. After the cap, sleep on `wake`
    ///     instead of polling — `reconnect()` resumes the worker.
    async fn run_worker(&self) {
        let mut backoff = BACKOFF_MIN;
        let mut failures: u32 = 0;

        loop {
            // Already connected → park until something tears it down.
            // `tear_down_for_reconnect()` clears `client` and pulses
            // `wake`, which is exactly what we wait on here.
            if self.is_connected() {
                self.inner.wake.notified().await;
                continue;
            }

            let directory = self.current_directory();
            let target = match resolve_target(directory.as_deref()) {
                Some(t) => t,
                None => {
                    failures = failures.saturating_add(1);
                    if failures >= MAX_FAILURES_BEFORE_PARK {
                        tracing::debug!(
                            failures,
                            "no IDE bridge discovered; parking until reconnect()"
                        );
                        self.inner.wake.notified().await;
                        failures = 0;
                        backoff = BACKOFF_MIN;
                        continue;
                    }
                    self.sleep_or_wake(backoff).await;
                    backoff = next_backoff(backoff);
                    continue;
                }
            };

            tracing::info!(
                attempt = failures + 1,
                port = target.port,
                ide = ?target.ide_name,
                source = ?target.source,
                "connecting to IDE bridge"
            );

            let opts = ConnectOptions {
                handshake_timeout: Duration::from_secs(3),
                call_timeout: Duration::from_secs(3),
            };

            match IdeBridgeClient::connect(&target, opts).await {
                Ok(client) => {
                    tracing::info!(
                        ide = client.ide_name().unwrap_or("unknown"),
                        "IDE bridge connected"
                    );

                    // Subscribe to the client's selection watch BEFORE we
                    // publish the client so the very first push (including
                    // the seed `getCurrentSelection` below) bumps the
                    // dirty flag and reaches the UI on the next tick.
                    let watcher = self.spawn_watcher(client.subscribe_selection());

                    if let Ok(mut s) = self.inner.state.write() {
                        s.client = Some(client.clone());
                        s.watcher = Some(watcher);
                    }

                    // Seed the selection cache so the next prompt can include
                    // the current editor context even before the IDE pushes
                    // its first `selection_changed`.
                    if let Ok(sel) = client.get_current_selection().await {
                        tracing::debug!(file = ?sel.file_path, "initial editor selection");
                    }

                    failures = 0;
                    backoff = BACKOFF_MIN;
                    // Loop top: now `is_connected()` is true → park.
                }
                Err(e) => {
                    failures = failures.saturating_add(1);
                    tracing::warn!(failures, error = %e, "IDE bridge connect failed");
                    if failures >= MAX_FAILURES_BEFORE_PARK {
                        tracing::debug!(failures, "IDE bridge connect repeatedly failing; parking");
                        self.inner.wake.notified().await;
                        failures = 0;
                        backoff = BACKOFF_MIN;
                        continue;
                    }
                    self.sleep_or_wake(backoff).await;
                    backoff = next_backoff(backoff);
                }
            }
        }
    }

    /// Sleep for `delay`, returning early when `reconnect()` pulses the
    /// worker. Lets a `cd` shortcut the backoff timer immediately.
    async fn sleep_or_wake(&self, delay: Duration) {
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = self.inner.wake.notified() => {}
        }
    }

    /// Tear down the live client (called by `reconnect()` after the
    /// directory changes) and pulse the worker so it loops back to
    /// reconnect against the new directory.
    async fn tear_down_for_reconnect(&self) {
        let (client, watcher) = if let Ok(mut s) = self.inner.state.write() {
            (s.client.take(), s.watcher.take())
        } else {
            (None, None)
        };
        if let Some(w) = watcher {
            w.abort();
        }
        if let Some(c) = client {
            c.shutdown().await;
        }
        self.inner.wake.notify_one();
    }

    fn spawn_watcher(
        &self,
        mut rx: tokio::sync::watch::Receiver<u64>,
    ) -> tokio::task::JoinHandle<()> {
        let handle = self.clone();
        tokio::spawn(async move {
            // Discard the seed value so we only react to genuine updates.
            rx.borrow_and_update();
            while rx.changed().await.is_ok() {
                handle.bump_selection_seq();
            }
        })
    }
}

fn resolve_target(directory: Option<&std::path::Path>) -> Option<BridgeTarget> {
    let result = match directory {
        Some(dir) => deepseek_ide_bridge::discovery::discover_for(dir),
        None => deepseek_ide_bridge::discover(),
    };
    match result {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "IDE bridge discovery failed");
            None
        }
    }
}

fn next_backoff(current: Duration) -> Duration {
    let doubled = current.saturating_mul(2);
    if doubled > BACKOFF_MAX {
        BACKOFF_MAX
    } else {
        doubled
    }
}

/// Maximum bytes of `selected_text` to include in the prompt block.
/// Larger selections are truncated with a marker so the system prompt
/// stays within sane token budgets even when the user has the entire
/// file selected in the IDE.
const MAX_SELECTED_TEXT_BYTES: usize = 4 * 1024;

/// Render the active editor selection as a system-prompt block.
/// Returns `None` when no IDE bridge is connected or no selection is available.
pub fn editor_context_block() -> Option<String> {
    let handle = IdeBridgeHandle::instance()?;
    handle
        .with_latest_selection(|selection| {
            // No file path → the selection is unusable as editor context.
            let path = selection.file_path.as_deref()?;
            let mut out =
                String::with_capacity(128 + selection.text.len().min(MAX_SELECTED_TEXT_BYTES));
            out.push_str("<editor_context>\n");
            out.push_str("file: ");
            out.push_str(path);
            out.push('\n');
            if let Some(range) = &selection.selection {
                // LSP positions are 0-based; render as 1-based for both
                // human readers and the model — matches the footer chip
                // (`IDE: foo.rs:21`) and the line/column convention used
                // in compiler errors, `grep -n`, stack traces, etc.
                out.push_str(&format!(
                    "selection: line {}:{} -> {}:{}\n",
                    range.start.line + 1,
                    range.start.character + 1,
                    range.end.line + 1,
                    range.end.character + 1,
                ));
            }
            if !selection.text.is_empty() {
                out.push_str("selected_text:\n");
                append_truncated(&mut out, &selection.text, MAX_SELECTED_TEXT_BYTES);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            out.push_str("</editor_context>");
            Some(out)
        })
        .flatten()
}

/// Append `text` to `out`, clamping at `limit` bytes on a UTF-8 char
/// boundary. Avoids the intermediate `String` allocation that the old
/// `truncate_text(...)` helper produced for every prompt assembly.
fn append_truncated(out: &mut String, text: &str, limit: usize) {
    if text.len() <= limit {
        out.push_str(text);
        return;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated_bytes = text.len() - end;
    out.push_str(&text[..end]);
    out.push_str(&format!("\n... [{truncated_bytes} bytes truncated]"));
}

#[cfg(test)]
mod tests {
    use super::{BACKOFF_MAX, BACKOFF_MIN, IdeBridgeHandle, append_truncated, next_backoff};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    fn render_truncated(text: &str, limit: usize) -> String {
        let mut out = String::new();
        append_truncated(&mut out, text, limit);
        out
    }

    #[test]
    fn truncate_text_preserves_char_boundaries() {
        let text = "abc深度def";
        let truncated = render_truncated(text, 5);
        assert!(truncated.starts_with("abc"));
        assert!(truncated.contains("bytes truncated"));
    }

    #[test]
    fn truncate_text_leaves_short_text_unchanged() {
        assert_eq!(render_truncated("short", 16), "short");
    }

    #[test]
    fn backoff_doubles_until_capped() {
        let mut delay = BACKOFF_MIN;
        let mut steps = 0;
        while delay < BACKOFF_MAX && steps < 10 {
            delay = next_backoff(delay);
            steps += 1;
        }
        assert_eq!(delay, BACKOFF_MAX);
        assert_eq!(next_backoff(BACKOFF_MAX), BACKOFF_MAX);
    }

    #[test]
    fn backoff_does_not_overflow_on_huge_input() {
        let huge = Duration::from_secs(u64::MAX / 2);
        assert_eq!(next_backoff(huge), BACKOFF_MAX);
    }

    #[test]
    fn take_dirty_flips_on_change_and_clears_after_consume() {
        let handle = IdeBridgeHandle::default();
        // No update yet → clean.
        assert!(!handle.take_dirty());

        // Simulate a selection push.
        handle.bump_selection_seq();
        assert!(handle.take_dirty(), "first read after bump must be dirty");
        assert!(
            !handle.take_dirty(),
            "second read without further bumps must be clean"
        );

        // Two pushes between reads coalesce into a single dirty flag —
        // exactly what we want for a per-tick UI redraw signal.
        handle.bump_selection_seq();
        handle.bump_selection_seq();
        assert!(handle.take_dirty());
        assert!(!handle.take_dirty());
    }

    #[test]
    fn take_dirty_handles_seq_wraparound() {
        // Pre-seed the counters so the next bump wraps from u64::MAX → 0.
        let handle = IdeBridgeHandle::default();
        handle
            .inner
            .selection_seq
            .store(u64::MAX, Ordering::Relaxed);
        handle
            .inner
            .selection_seq_seen
            .store(u64::MAX, Ordering::Relaxed);

        handle.bump_selection_seq();
        assert!(
            handle.take_dirty(),
            "wraparound must still register as dirty"
        );
    }
}
