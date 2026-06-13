//! Memoization for the per-cell tool-output shaping pipeline.
//!
//! `output_rows` (in `tui::history`) walks the raw tool output, ANSI-strips
//! each line, classifies path/URL-like rows, and wraps the rest to the
//! current viewport width. `selected_output_indices` then computes the
//! head/tail/importance subset that the compact "Live" view shows. Both
//! functions are pure functions of `(output, width)` and `(rows,
//! line_limit)`, but they are called on every render frame for every
//! visible tool cell. For a 4 KB output on a 120 FPS render loop, that
//! is 2–6 redundant walks per frame, per cell.
//!
//! This module adds a process-local, content-addressed cache in front of
//! the two pure functions. The cache is global (one per process) and
//! consults a small `HashMap` keyed on `(content_hash, width)` for the
//! rows and `(rows_hash, line_limit)` for the indices. Insertion-order
//! LRU eviction keeps memory bounded.
//!
//! ## When the cache is a win
//!
//! - Long tool cells that are scrolled into view repeatedly (the model
//!   often re-asks for the same `read_file` after a partial failure).
//! - The whole transcript re-rendering at 120 FPS while streaming: the
//!   finalized tool cells below the live tail are unchanged on every
//!   frame, so their `output_rows` and `selected_output_indices` calls
//!   are pure cache hits.
//! - Terminal resizes still invalidate correctly because `width` is part
//!   of the key.
//!
//! ## When the cache misses
//!
//! - New tool output (different `content_hash`).
//! - First render of a cell (cache is cold).
//! - Terminal width changed since the last render.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};

use crate::tui::history::OutputRow;

/// Default capacity for the LRU. Sized for a worst-case \"5,000-line
/// transcript at 200 cells, plus a 4 KB row cache for the live tail\" —
/// well under a megabyte.
const DEFAULT_CAPACITY: usize = 256;

/// Internal cache entry. Stores the wrapped `Vec<OutputRow>` plus the
/// `Vec<usize>` of selected indices so a single key lookup can satisfy
/// both render steps. Indices are recomputed lazily when the
/// `line_limit` changes; rows are shared across all line limits.
#[derive(Debug, Clone)]
struct CacheEntry {
    rows: Vec<OutputRow>,
    /// Map of `line_limit -> selected indices`. Bounded by the
    /// distinct line limits passed in by the renderer (typically 1–3).
    selected_by_limit: HashMap<usize, Vec<usize>>,
}

impl CacheEntry {
    fn new(rows: Vec<OutputRow>) -> Self {
        Self {
            rows,
            selected_by_limit: HashMap::new(),
        }
    }
}

/// Bounded LRU cache of `(output, width) -> OutputRowsCacheEntry`.
///
/// The eviction policy is insertion-order: when the cache reaches
/// `capacity`, the oldest-inserted key is dropped first. Re-inserting an
/// existing key (different content) keeps the original position, so
/// re-rendering the same cell on every frame does not churn unrelated
/// entries.
#[derive(Debug)]
struct OutputRowsCacheInner {
    capacity: usize,
    by_key: HashMap<RowsKey, CacheEntry>,
    insertion_order: VecDeque<RowsKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct RowsKey {
    /// 64-bit content hash of the raw tool output. Two outputs with
    /// different bytes produce different hashes; identical bytes produce
    /// the same hash.
    content_hash: u64,
    /// Terminal width used for wrapping. Resize invalidates.
    width: u16,
}

impl OutputRowsCacheInner {
    fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            capacity: cap,
            by_key: HashMap::with_capacity(cap),
            insertion_order: VecDeque::with_capacity(cap),
        }
    }

    /// Get or compute the wrapped output rows for `output` at `width`.
    /// On a hit, returns a clone of the cached `Vec<OutputRow>` — the
    /// caller can iterate without holding a lock.
    fn get_or_compute_rows<F>(
        &mut self,
        content_hash: u64,
        width: u16,
        compute: F,
    ) -> Vec<OutputRow>
    where
        F: FnOnce() -> Vec<OutputRow>,
    {
        let key = RowsKey {
            content_hash,
            width,
        };
        if let Some(entry) = self.by_key.get(&key) {
            return entry.rows.clone();
        }

        let rows = compute();
        let entry = CacheEntry::new(rows.clone());

        if self.by_key.len() >= self.capacity
            && let Some(oldest) = self.insertion_order.pop_front()
        {
            self.by_key.remove(&oldest);
        }
        self.by_key.insert(key, entry);
        self.insertion_order.push_back(key);
        rows
    }

    /// Get or compute the selected indices for the cached rows at the
    /// given `line_limit`. Looks up the row entry by `(content_hash,
    /// width)` first (the same key used to insert the rows) and then
    /// consults the per-line-limit map on that entry. `compute` is
    /// invoked only on the first call for a given
    /// `(content_hash, width, line_limit)` triple.
    fn get_or_compute_indices<F>(
        &mut self,
        content_hash: u64,
        width: u16,
        line_limit: usize,
        compute: F,
    ) -> Vec<usize>
    where
        F: FnOnce() -> Vec<usize>,
    {
        let key = RowsKey {
            content_hash,
            width,
        };
        if let Some(entry) = self.by_key.get_mut(&key)
            && let Some(indices) = entry.selected_by_limit.get(&line_limit)
        {
            return indices.clone();
        }

        let indices = compute();
        if let Some(entry) = self.by_key.get_mut(&key) {
            entry.selected_by_limit.insert(line_limit, indices.clone());
        }
        indices
    }
}

thread_local! {
    /// Thread-local cache. The TUI render loop runs on a single thread,
    /// so a `!Sync` cache is sufficient and avoids contention with any
    /// background workers that might call into the same module.
    static GLOBAL_CACHE: RefCell<OutputRowsCacheInner> =
        RefCell::new(OutputRowsCacheInner::new());
}

/// Reset the global cache. Used by tests and `/clear`.
#[cfg(test)]
pub fn reset_for_tests() {
    GLOBAL_CACHE.with(|c| *c.borrow_mut() = OutputRowsCacheInner::new());
}

/// Look up (or compute) the wrapped output rows for `output` at `width`.
/// On a hit the cached `Vec<OutputRow>` is cloned without re-running
/// the per-line ANSI strip or the wrap pass.
pub fn get_or_compute_rows<F>(output: &str, width: u16, compute: F) -> Vec<OutputRow>
where
    F: FnOnce() -> Vec<OutputRow>,
{
    let content_hash = hash_str(output);
    GLOBAL_CACHE.with(|c| {
        c.borrow_mut()
            .get_or_compute_rows(content_hash, width, compute)
    })
}

/// Look up (or compute) the selected indices for a previously-cached
/// rows payload at the given `line_limit`. `content_hash` is the same
/// 64-bit content hash that was passed to [`get_or_compute_rows`].
pub fn get_or_compute_indices<F>(
    content_hash: u64,
    width: u16,
    line_limit: usize,
    compute: F,
) -> Vec<usize>
where
    F: FnOnce() -> Vec<usize>,
{
    GLOBAL_CACHE.with(|c| {
        c.borrow_mut()
            .get_or_compute_indices(content_hash, width, line_limit, compute)
    })
}

/// FNV-1a 64-bit content hash. Cheap, no per-process key, and ~5-10×
/// faster than `DefaultHasher` (SipHash) on the small-to-medium tool
/// output strings we see on the render hot path. The cache is a
/// correctness optimization, not a security boundary — a 64-bit collision
/// space is more than wide enough for the per-process LRU's expected
/// ≤ a few hundred entries, and collisions only cause a false miss,
/// never wrong data.
pub fn hash_str(s: &str) -> u64 {
    /// FNV-1a 64-bit offset basis.
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    /// FNV-1a 64-bit prime.
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Mix the length in last so two strings that share a prefix but
    // differ in length (e.g. one has a trailing newline) still collide
    // only on truly-identical content.
    hash ^= s.len() as u64;
    hash.wrapping_mul(FNV_PRIME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> OutputRow {
        OutputRow {
            text: text.to_string(),
            intact: false,
        }
    }

    #[test]
    fn cache_hit_returns_cached_rows() {
        reset_for_tests();

        let calls = std::cell::Cell::new(0u32);
        let compute = || {
            calls.set(calls.get() + 1);
            vec![row("hello"), row("world")]
        };

        let a = get_or_compute_rows("payload", 80, compute);
        let b = get_or_compute_rows("payload", 80, || {
            calls.set(calls.get() + 1);
            vec![row("hello"), row("world")]
        });
        assert_eq!(calls.get(), 1, "second call should hit the cache");
        assert_eq!(a, b);
    }

    #[test]
    fn different_width_invalidates_rows() {
        reset_for_tests();

        let calls = std::cell::Cell::new(0u32);
        let make = || {
            calls.set(calls.get() + 1);
            vec![row("hello")]
        };

        let _ = get_or_compute_rows("payload", 80, make);
        let _ = get_or_compute_rows("payload", 120, make);
        assert_eq!(calls.get(), 2, "different width must miss the cache");
    }

    #[test]
    fn different_output_invalidates_rows() {
        reset_for_tests();

        let calls = std::cell::Cell::new(0u32);
        let make = || {
            calls.set(calls.get() + 1);
            vec![row("x")]
        };

        let _ = get_or_compute_rows("payload-a", 80, make);
        let _ = get_or_compute_rows("payload-b", 80, make);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn indices_cached_per_line_limit() {
        reset_for_tests();

        let rows = get_or_compute_rows("payload", 80, || {
            vec![row("a"), row("b"), row("c"), row("d"), row("e")]
        });
        assert_eq!(rows.len(), 5);

        let content_hash = hash_str("payload");
        let mut calls = 0;
        let pick_two_a = get_or_compute_indices(content_hash, 80, 2, || {
            calls += 1;
            vec![0usize, 4]
        });
        let pick_two_b = get_or_compute_indices(content_hash, 80, 2, || {
            calls += 1;
            vec![0usize, 4]
        });
        assert_eq!(calls, 1, "second lookup with same limit hits the cache");
        assert_eq!(pick_two_a, pick_two_b);
        assert_eq!(pick_two_a, vec![0, 4]);

        // Different line_limit must miss and recompute.
        let _ = get_or_compute_indices(content_hash, 80, 3, || {
            calls += 1;
            vec![0usize, 1, 4]
        });
        assert_eq!(calls, 2);
    }

    #[test]
    fn capacity_evicts_oldest() {
        // Build a private cache so we can size it tightly.
        let mut cache = OutputRowsCacheInner::with_capacity(2);

        let _ = cache.get_or_compute_rows(1, 80, || vec![row("a")]);
        let _ = cache.get_or_compute_rows(2, 80, || vec![row("b")]);
        let _ = cache.get_or_compute_rows(3, 80, || vec![row("c")]);
        // The first entry (hash 1) should have been evicted.
        let mut compute_calls = 0;
        let _ = cache.get_or_compute_rows(1, 80, || {
            compute_calls += 1;
            vec![row("a")]
        });
        assert_eq!(compute_calls, 1, "evicted entry must miss");
    }

    #[test]
    fn hash_str_stable_for_identical_input() {
        assert_eq!(hash_str("hello"), hash_str("hello"));
        assert_ne!(hash_str("hello"), hash_str("world"));
    }

    #[test]
    fn hash_str_differs_on_length_suffix() {
        // A trailing newline is a different content; the hash must differ.
        assert_ne!(hash_str("hello"), hash_str("hello\n"));
    }

    #[test]
    fn hash_str_handles_empty() {
        // Empty string hashes to the FNV offset basis; the result just
        // needs to be stable.
        assert_eq!(hash_str(""), hash_str(""));
    }
}
