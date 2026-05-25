//! Cross-session composer input history (#366).
//!
//! Persists user-typed prompts to `~/.deepseek/composer_history.txt` so
//! pressing Up-arrow at the composer recalls submissions from previous
//! sessions, not just the current one. One entry per line, oldest first,
//! capped at [`MAX_HISTORY_ENTRIES`] entries (older entries are pruned
//! at append time).
//!
//! Entries that begin with `/` (slash commands) are NOT stored — they
//! pollute the recall stream and the fuzzy slash-menu already covers
//! them. Empty / whitespace-only inputs are also skipped.
//!
//! ## Off-thread writes (#1927)
//!
//! [`append_history`] used to block the caller for a read-then-atomic-
//! rewrite of the full file. That ran on the UI thread inside
//! `submit_input`, contributing a perceptible stall after Enter. The
//! public entry point now hands work to a dedicated writer thread via
//! [`writer_sender`] and returns immediately. Submissions stay serialised
//! in arrival order, so the on-disk file keeps its "oldest first"
//! invariant.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

/// Hard cap on persisted history. Keeps the file small (typical entries
/// are < 200 chars, so 1000 entries ≈ 200 KB) and bounds startup load
/// time.
pub const MAX_HISTORY_ENTRIES: usize = 1000;

const HISTORY_FILE_NAME: &str = "composer_history.txt";
// Prevent a steady stream of test/UI writes from delaying persistence
// indefinitely while waiting for a 2ms idle window.
const WRITER_BATCH_LIMIT: usize = 128;

fn default_history_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".deepseek").join(HISTORY_FILE_NAME))
}

/// Read the persisted history into memory. Returns an empty vec if the
/// file doesn't exist or can't be parsed — this is best-effort.
#[must_use]
pub fn load_history() -> Vec<String> {
    let Some(path) = default_history_path() else {
        return Vec::new();
    };
    load_history_from(&path)
}

fn load_history_from(path: &Path) -> Vec<String> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .collect()
}

/// Append an entry to the persisted history, pruning old entries to
/// stay within [`MAX_HISTORY_ENTRIES`]. Slash-commands and empty input
/// are skipped — those don't help recall.
///
/// Best-effort and non-blocking — work is forwarded to a dedicated writer
/// thread so the caller (typically the UI submit handler) returns
/// immediately. See module docs for the rationale (#1927). Failures on
/// the writer thread are logged via `tracing` but not propagated.
pub fn append_history(entry: &str) {
    let Some(path) = default_history_path() else {
        return;
    };
    append_history_dispatched(&path, entry);
}

/// Path-injectable variant of [`append_history`] used by tests. Forwards
/// the work to the dedicated writer thread (or falls back to a synchronous
/// write if the channel send fails) so callers never block on disk I/O.
fn append_history_dispatched(path: &Path, entry: &str) {
    let entry = entry.to_string();
    if let Err(err) = writer_sender().send(HistoryWrite::Append(path.to_path_buf(), entry)) {
        match err.0 {
            HistoryWrite::Append(path, entry) => append_history_to(&path, &entry),
            #[cfg(test)]
            HistoryWrite::Flush(_) => unreachable!("flush messages are only sent by tests"),
        }
    }
}

enum HistoryWrite {
    Append(PathBuf, String),
    #[cfg(test)]
    Flush(Sender<()>),
}

/// Lazy singleton sender for the dedicated composer-history writer
/// thread. Initialised on first use; the thread runs for the lifetime
/// of the process and drains queued writes in arrival order.
fn writer_sender() -> &'static Sender<HistoryWrite> {
    static SENDER: OnceLock<Sender<HistoryWrite>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = channel::<HistoryWrite>();
        let spawn_result = std::thread::Builder::new()
            .name("composer-history-writer".to_string())
            .spawn(move || {
                // recv() returns Err when all senders have dropped, which
                // only happens at process shutdown because the singleton
                // sender lives in a static for the lifetime of the process.
                while let Ok(message) = rx.recv() {
                    match message {
                        HistoryWrite::Append(path, entry) => {
                            append_history_batch(&rx, (path, entry));
                        }
                        #[cfg(test)]
                        HistoryWrite::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            });
        if let Err(err) = spawn_result {
            tracing::warn!("Failed to spawn composer-history-writer: {err}");
        }
        tx
    })
}

fn append_history_batch(rx: &Receiver<HistoryWrite>, first: (PathBuf, String)) {
    let mut pending = vec![first];
    #[cfg(test)]
    let mut flush = None;

    while pending.len() < WRITER_BATCH_LIMIT {
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(HistoryWrite::Append(path, entry)) => pending.push((path, entry)),
            #[cfg(test)]
            Ok(HistoryWrite::Flush(done)) => {
                flush = Some(done);
                break;
            }
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    for (path, entries) in group_history_writes_by_path(pending) {
        append_history_entries_to(&path, entries.iter().map(String::as_str));
    }

    #[cfg(test)]
    if let Some(done) = flush {
        let _ = done.send(());
    }
}

fn group_history_writes_by_path(writes: Vec<(PathBuf, String)>) -> Vec<(PathBuf, Vec<String>)> {
    let mut grouped: Vec<(PathBuf, Vec<String>)> = Vec::new();

    for (path, entry) in writes {
        if let Some((_, entries)) = grouped
            .iter_mut()
            .find(|(existing_path, _)| existing_path == &path)
        {
            entries.push(entry);
        } else {
            grouped.push((path, vec![entry]));
        }
    }

    grouped
}

fn append_history_to(path: &Path, entry: &str) {
    append_history_entries_to(path, std::iter::once(entry));
}

fn append_history_entries_to<'a>(
    path: &Path,
    entries_to_append: impl IntoIterator<Item = &'a str>,
) {
    if let Some(parent) = path.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        tracing::warn!(
            "Failed to create composer history dir {}: {err}",
            parent.display()
        );
        return;
    }

    // Read existing entries, append the new ones, prune from the front
    // until under the cap, then atomically rewrite.
    let mut entries = load_history_from(path);
    let mut changed = false;
    for entry in entries_to_append {
        let trimmed = entry.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') {
            continue;
        }
        if entries.last().map(String::as_str) == Some(trimmed) {
            // De-dupe consecutive duplicates — repeated submission of the
            // same prompt shouldn't bloat the file.
            continue;
        }
        entries.push(trimmed.to_string());
        changed = true;
    }

    if !changed {
        return;
    }

    if entries.len() > MAX_HISTORY_ENTRIES {
        let excess = entries.len() - MAX_HISTORY_ENTRIES;
        entries.drain(0..excess);
    }

    let payload = entries.join("\n") + "\n";
    if let Err(err) = write_history_atomic(path, payload.as_bytes()) {
        tracing::warn!(
            "Failed to persist composer history at {}: {err}",
            path.display()
        );
    }
}

fn write_history_atomic(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    const RETRY_DELAYS: &[Duration] = &[
        Duration::from_millis(5),
        Duration::from_millis(10),
        Duration::from_millis(25),
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(200),
        Duration::from_millis(400),
    ];

    for (attempt, delay) in RETRY_DELAYS
        .iter()
        .map(Some)
        .chain(std::iter::once(None))
        .enumerate()
    {
        match crate::utils::write_atomic(path, payload) {
            Ok(()) => return Ok(()),
            Err(err) if delay.is_some() => {
                tracing::debug!(
                    "Retrying composer history write to {} after attempt {} failed: {err}",
                    path.display(),
                    attempt + 1
                );
                std::thread::sleep(*delay.expect("delay checked"));
            }
            Err(err) => return Err(err),
        }
    }

    unreachable!("retry iterator always ends with a final write attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Tests use the path-injecting `*_from` / `*_to` helpers so they
    /// don't have to mutate `HOME` (which is not honored by
    /// `dirs::home_dir()` on Windows — it reads `USERPROFILE` /
    /// `SHGetKnownFolderPath` instead). This makes the suite portable
    /// across all three CI runners without per-platform env juggling.
    fn temp_history_path() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join(HISTORY_FILE_NAME);
        (tmp, path)
    }

    fn flush_history_writer_for_tests(timeout: Duration) {
        let (done_tx, done_rx) = channel();
        writer_sender()
            .send(HistoryWrite::Flush(done_tx))
            .expect("history writer accepts flush");
        done_rx
            .recv_timeout(timeout)
            .expect("history writer flush timed out");
    }

    #[test]
    fn append_and_load_round_trip() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "first");
        append_history_to(&path, "second");
        append_history_to(&path, "third");
        assert_eq!(load_history_from(&path), vec!["first", "second", "third"]);
    }

    #[test]
    fn slash_commands_skipped() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "/help");
        append_history_to(&path, "real prompt");
        append_history_to(&path, "/cost");
        assert_eq!(load_history_from(&path), vec!["real prompt"]);
    }

    #[test]
    fn empty_and_whitespace_skipped() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "");
        append_history_to(&path, "   ");
        append_history_to(&path, "\n\t");
        append_history_to(&path, "real");
        assert_eq!(load_history_from(&path), vec!["real"]);
    }

    #[test]
    fn consecutive_duplicates_deduped() {
        let (_tmp, path) = temp_history_path();
        append_history_to(&path, "same");
        append_history_to(&path, "same");
        append_history_to(&path, "same");
        append_history_to(&path, "different");
        append_history_to(&path, "same");
        assert_eq!(load_history_from(&path), vec!["same", "different", "same"]);
    }

    #[test]
    fn pruned_to_cap_at_append_time() {
        let (_tmp, path) = temp_history_path();
        for i in 0..(MAX_HISTORY_ENTRIES + 50) {
            append_history_to(&path, &format!("entry {i}"));
        }
        let history = load_history_from(&path);
        assert_eq!(history.len(), MAX_HISTORY_ENTRIES);
        // Newest entries survive; oldest 50 were pruned.
        assert_eq!(history.first().map(String::as_str), Some("entry 50"));
        assert_eq!(
            history.last().map(String::as_str),
            Some(format!("entry {}", MAX_HISTORY_ENTRIES + 49)).as_deref()
        );
    }

    #[test]
    fn missing_file_loads_empty() {
        let (_tmp, path) = temp_history_path();
        assert!(load_history_from(&path).is_empty());
    }

    /// Regression for #1927 — the dispatched append path must return
    /// promptly even when a synchronous write of the seeded file would
    /// be slow. We pre-populate the file with ~1000 entries (the cap)
    /// so a sync read-modify-write would take real disk time on any
    /// platform, then call `append_history_dispatched` many times and
    /// assert that the cumulative wall-clock cost stays well below the
    /// stall the user reports.
    #[test]
    fn append_history_dispatched_does_not_block_the_caller() {
        let (_tmp, path) = temp_history_path();
        // Seed close to the cap so a synchronous rewrite is non-trivial.
        let seed = (0..(MAX_HISTORY_ENTRIES - 50))
            .map(|i| format!("seed entry {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&path, seed).expect("seed history");

        let start = Instant::now();
        for i in 0..50 {
            append_history_dispatched(&path, &format!("new entry {i}"));
        }
        let dispatch_elapsed = start.elapsed();

        // 50 sync read-modify-write cycles on a ~200KB file would be
        // measurable (tens of ms even on a fast SSD). The dispatch path
        // hands work to the writer thread and returns; the whole loop
        // should finish in single-digit ms. Pick a generous CI-safe
        // bound that still catches a regression to the old sync path.
        assert!(
            dispatch_elapsed < Duration::from_millis(150),
            "append_history dispatch was too slow: {dispatch_elapsed:?} \
             (likely re-introduced #1927: caller blocked on disk write)"
        );

        flush_history_writer_for_tests(Duration::from_secs(if cfg!(windows) { 10 } else { 5 }));

        let loaded = load_history_from(&path);
        assert!(
            loaded.iter().any(|line| line == "new entry 49"),
            "writer thread did not persist the dispatched entries; \
             loaded {} entries, last = {:?}",
            loaded.len(),
            loaded.last()
        );
        assert!(loaded.iter().any(|line| line == "new entry 0"));
    }
}
