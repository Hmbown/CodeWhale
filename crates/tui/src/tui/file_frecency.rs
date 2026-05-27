//! @-mention frecency tracking (#441).
//!
//! Records every file the user @-mentions with a timestamp and click count,
//! decays the score over time so a file that was hot last week ranks below
//! one mentioned 5 minutes ago, and re-orders mention-popup completions by
//! the resulting score. Persisted as a single JSONL file at
//! `~/.deepseek/file-frecency.jsonl` so frecency survives restarts.
//!
//! Append-only on the wire, compacted in memory: the loader replays every
//! line into a `HashMap<String, FrecencyEntry>` keyed by repo-relative path,
//! folding duplicates into the last record. We cap the in-memory map at
//! 1000 entries and evict the lowest-scored on overflow — same heuristic
//! the OPENCODE source uses.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Hard cap on the number of paths we track (the acceptance criterion for
/// #441). Older / lower-scored entries are evicted when the map exceeds
/// this.
const FRECENCY_CAP: usize = 1000;

/// Half-life of a frecency score, in seconds. After this many seconds the
/// score has decayed to ½ of its peak. 7 days is OPENCODE's default — long
/// enough that a commonly-edited file stays sticky across a workweek but
/// short enough that yesterday's deep-dive doesn't haunt you forever.
const HALF_LIFE_SECS: f64 = 7.0 * 24.0 * 60.0 * 60.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FrecencyRecord {
    /// Workspace-relative path string.
    path: String,
    /// Total mentions over the lifetime of the entry.
    count: u32,
    /// Unix timestamp (seconds) of the last mention.
    last_used: u64,
}

#[derive(Debug, Default)]
struct Store {
    by_path: HashMap<String, FrecencyRecord>,
    persisted_path: Option<PathBuf>,
    loaded: bool,
}

fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Store::default()))
}

fn default_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codewhale").join("file-frecency.jsonl"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Time-decayed frecency score for a record, in arbitrary units. Mentions
/// count linearly; the whole sum is multiplied by an exponential decay
/// factor based on time since `last_used`. Records older than ~5 half-lives
/// score effectively zero.
fn decayed_score(record: &FrecencyRecord, now: u64) -> f64 {
    let age_secs = now.saturating_sub(record.last_used) as f64;
    let lambda = std::f64::consts::LN_2 / HALF_LIFE_SECS;
    (record.count as f64) * (-lambda * age_secs).exp()
}

fn ensure_loaded(store: &mut Store) {
    if store.loaded {
        return;
    }
    store.loaded = true;
    let Some(path) = default_path() else {
        return;
    };
    store.persisted_path = Some(path.clone());
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<FrecencyRecord>(line) else {
            continue;
        };
        store.by_path.insert(record.path.clone(), record);
    }
}

fn evict_to_cap(store: &mut Store, now: u64) {
    if store.by_path.len() <= FRECENCY_CAP {
        return;
    }
    let target = FRECENCY_CAP;
    let mut scored: Vec<(String, f64)> = store
        .by_path
        .iter()
        .map(|(k, v)| (k.clone(), decayed_score(v, now)))
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let drop_count = store.by_path.len().saturating_sub(target);
    for (key, _) in scored.iter().take(drop_count) {
        store.by_path.remove(key);
    }
}

fn append_record_line(path: &PathBuf, record: &FrecencyRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Record one mention of `path` (a workspace-relative path string). Updates
/// the in-memory store, persists a single JSONL line, and evicts the lowest-
/// scored entry if we just exceeded the cap. Best-effort: I/O failures are
/// logged and swallowed — losing a frecency datapoint is never worth
/// failing the user's `@` autocomplete.
pub fn record_mention(path: &str) {
    if path.is_empty() {
        return;
    }
    let store = store();
    let Ok(mut store) = store.lock() else {
        return;
    };
    ensure_loaded(&mut store);
    let now = now_secs();
    let entry = store
        .by_path
        .entry(path.to_string())
        .or_insert_with(|| FrecencyRecord {
            path: path.to_string(),
            count: 0,
            last_used: now,
        });
    entry.count = entry.count.saturating_add(1);
    entry.last_used = now;
    let snapshot = entry.clone();
    if let Some(persisted_path) = store.persisted_path.clone()
        && let Err(err) = append_record_line(&persisted_path, &snapshot)
    {
        tracing::debug!(target: "frecency", "persist failed: {err}");
    }
    evict_to_cap(&mut store, now);
}

/// Re-sort a candidate list by frecency score (highest first), preserving
/// the original order for ties so the underlying ranker's choices aren't
/// upended. Candidates the store has never seen score zero — they end up
/// at the bottom of the sort, which means a one-time mention will start
/// floating to the top after first use.
#[must_use]
pub fn rerank_by_frecency(candidates: Vec<String>) -> Vec<String> {
    if candidates.len() <= 1 {
        return candidates;
    }
    let store = store();
    let Ok(mut store) = store.lock() else {
        return candidates;
    };
    ensure_loaded(&mut store);
    let now = now_secs();
    let mut scored: Vec<(usize, String, f64)> = candidates
        .into_iter()
        .enumerate()
        .map(|(idx, path)| {
            let score = store
                .by_path
                .get(&path)
                .map(|r| decayed_score(r, now))
                .unwrap_or(0.0);
            (idx, path, score)
        })
        .collect();
    // Stable sort on (-score, original-index): ties keep the underlying
    // ranker's order.
    scored.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.into_iter().map(|(_, path, _)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recently mentioned paths win against never-mentioned ones; never-mentioned
    /// preserve their original ranker order.
    #[test]
    fn rerank_floats_recent_paths_to_the_top() {
        // Use the global store; reset its state so we don't leak across tests.
        let store = super::store();
        let mut s = store.lock().unwrap();
        s.by_path.clear();
        s.loaded = true; // skip on-disk replay
        s.persisted_path = None; // skip persistence
        let now = super::now_secs();
        s.by_path.insert(
            "src/popular.rs".into(),
            FrecencyRecord {
                path: "src/popular.rs".into(),
                count: 8,
                last_used: now,
            },
        );
        drop(s);

        let order = super::rerank_by_frecency(vec![
            "README.md".to_string(),
            "src/popular.rs".to_string(),
            "Cargo.toml".to_string(),
        ]);
        assert_eq!(order[0], "src/popular.rs");
        // README.md was first in original order; Cargo.toml second. Both score 0
        // so the original relative order survives.
        assert_eq!(order[1], "README.md");
        assert_eq!(order[2], "Cargo.toml");
    }

    /// Decayed score drops below a freshly-used entry after enough half-lives
    /// that count alone can't carry the older one. With a 7-day half-life,
    /// 8 weeks gives 8 half-lives → ~256× decay; an entry mentioned twice
    /// today comfortably beats one mentioned 50× two months ago.
    #[test]
    fn old_entries_decay_below_recent_ones() {
        let now: u64 = 7 * 24 * 60 * 60 * 8; // 8 weeks (8 half-lives)
        let stale = FrecencyRecord {
            path: "x".into(),
            count: 50,
            last_used: 0,
        };
        let fresh = FrecencyRecord {
            path: "y".into(),
            count: 2,
            last_used: now,
        };
        assert!(
            super::decayed_score(&fresh, now) > super::decayed_score(&stale, now),
            "fresh={}, stale={}",
            super::decayed_score(&fresh, now),
            super::decayed_score(&stale, now)
        );
    }
}
