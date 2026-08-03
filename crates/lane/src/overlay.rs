//! Durable overlay store under `$CODEWHALE_HOME/overlay/`.
//!
//! Each overlay entry is persisted as a single JSON file:
//!
//! ```text
//! $CODEWHALE_HOME/overlay/<entry-id>.json
//! ```
//!
//! The store never mutates Git main — it is a side-channel that warms future
//! runs by providing promoted notes, workflow patches, tests, branch heuristics,
//! model/cache policies, and prompt patches.
//!
//! ## Acceptance criteria
//!
//! - Entries are **listable** (`OverlayStore::list`).
//! - Entries are **attributable** to their promoting run
//!   (`OverlayEntry::promoting_run_id`; `OverlayStore::list_by_run`).
//! - Entries are **removable** (`OverlayStore::remove`).
//! - Runs are **diffable in telemetry** via `OverlayRunDiff` in
//!   `codewhale-workflow`.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use codewhale_config::codewhale_home;
use codewhale_workflow::{OverlayEntry, OverlayEntryKind, validate_overlay_entry};
use uuid::Uuid;

const OVERLAY_SUBDIR: &str = "overlay";

/// Resolve `$CODEWHALE_HOME/overlay`.
pub fn overlay_root() -> Result<PathBuf> {
    Ok(codewhale_home()?.join(OVERLAY_SUBDIR))
}

/// Durable store for cached-main overlay entries.
///
/// Each entry is stored as `<root>/<id>.json`.  The store never re-reads a
/// partially-written file: writes go through a `.tmp` side-file that is
/// renamed into place atomically.
#[derive(Debug, Clone)]
pub struct OverlayStore {
    root: PathBuf,
}

impl OverlayStore {
    /// Open an overlay store at `root`.  The directory is created lazily on the
    /// first write so that read-only operations don't require write permission.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Open the default overlay store at `$CODEWHALE_HOME/overlay`.
    pub fn open_default() -> Result<Self> {
        Ok(Self::new(overlay_root()?))
    }

    /// Directory where overlay entries live.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return all overlay entries, sorted by `promoted_at` ascending.
    ///
    /// Files that cannot be parsed are skipped with a tracing warning rather
    /// than causing the whole list to fail.
    pub fn list(&self) -> Result<Vec<OverlayEntry>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }

        let mut entries: Vec<OverlayEntry> = fs::read_dir(&self.root)
            .with_context(|| format!("reading overlay directory {}", self.root.display()))?
            .filter_map(|dir_entry| {
                let dir_entry = dir_entry.ok()?;
                let path = dir_entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    return None;
                }
                match fs::read_to_string(&path) {
                    Ok(raw) => match serde_json::from_str::<OverlayEntry>(&raw) {
                        Ok(entry) => Some(entry),
                        Err(err) => {
                            tracing::warn!(
                                path = %path.display(),
                                %err,
                                "skipping unparseable overlay entry"
                            );
                            None
                        }
                    },
                    Err(err) => {
                        tracing::warn!(
                            path = %path.display(),
                            %err,
                            "skipping unreadable overlay entry"
                        );
                        None
                    }
                }
            })
            .collect();

        entries.sort_by(|a, b| a.promoted_at.cmp(&b.promoted_at));
        Ok(entries)
    }

    /// Return all overlay entries promoted by `run_id`.
    pub fn list_by_run(&self, run_id: &str) -> Result<Vec<OverlayEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|entry| entry.promoting_run_id == run_id)
            .collect())
    }

    /// Return all overlay entries of a given kind.
    pub fn list_by_kind(&self, kind: OverlayEntryKind) -> Result<Vec<OverlayEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|entry| entry.kind == kind)
            .collect())
    }

    /// Retrieve a single overlay entry by id.  Returns `None` if not found.
    pub fn get(&self, id: &str) -> Result<Option<OverlayEntry>> {
        let path = self.entry_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("reading overlay entry {}", path.display()))?;
        let entry = serde_json::from_str::<OverlayEntry>(&raw)
            .with_context(|| format!("parsing overlay entry {}", path.display()))?;
        Ok(Some(entry))
    }

    /// Promote an entry into the overlay.
    ///
    /// If `entry.id` is empty, a new UUID is generated and assigned.
    /// If `entry.promoted_at` is empty, the current UTC time is used.
    ///
    /// Fails if an entry with the same id already exists — callers must
    /// `remove` before re-adding if they want to replace an entry.
    pub fn add(&self, mut entry: OverlayEntry) -> Result<OverlayEntry> {
        if entry.id.is_empty() {
            entry.id = Uuid::new_v4().to_string();
        }
        if entry.promoted_at.is_empty() {
            entry.promoted_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        }

        validate_overlay_entry(&entry).map_err(|e| anyhow::anyhow!("{e}"))?;

        fs::create_dir_all(&self.root)
            .with_context(|| format!("creating overlay directory {}", self.root.display()))?;

        let path = self.entry_path(&entry.id);
        if path.exists() {
            bail!(
                "overlay entry `{}` already exists; remove it before re-adding",
                entry.id
            );
        }

        let tmp_path = path.with_extension("json.tmp");
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .with_context(|| format!("creating temp file {}", tmp_path.display()))?;
            let json = serde_json::to_string_pretty(&entry).context("serialising overlay entry")?;
            file.write_all(json.as_bytes())
                .with_context(|| format!("writing overlay entry to {}", tmp_path.display()))?;
        }

        fs::rename(&tmp_path, &path)
            .with_context(|| format!("renaming {} → {}", tmp_path.display(), path.display()))?;

        Ok(entry)
    }

    /// Remove an overlay entry by id.
    ///
    /// Returns `true` if the entry was present and removed, `false` if not
    /// found.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let path = self.entry_path(id);
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path)
            .with_context(|| format!("removing overlay entry {}", path.display()))?;
        Ok(true)
    }

    /// Remove all overlay entries attributed to `run_id`.
    ///
    /// Returns the ids of entries that were removed.
    pub fn remove_by_run(&self, run_id: &str) -> Result<Vec<String>> {
        let to_remove: Vec<String> = self
            .list_by_run(run_id)?
            .into_iter()
            .map(|e| e.id)
            .collect();
        for id in &to_remove {
            self.remove(id)?;
        }
        Ok(to_remove)
    }

    fn entry_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_workflow::OverlayEntryKind;

    fn store_in_tempdir() -> (tempfile::TempDir, OverlayStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = OverlayStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    fn entry(id: &str, run_id: &str, kind: OverlayEntryKind) -> OverlayEntry {
        OverlayEntry::new(id, kind, "content", run_id, "2026-08-03T00:00:00Z")
    }

    #[test]
    fn list_empty_on_missing_dir() {
        let (_dir, store) = store_in_tempdir();
        // Root doesn't exist yet — should return empty, not an error.
        let root = store.root().to_path_buf();
        let store_no_dir = OverlayStore::new(root.join("does-not-exist"));
        let list = store_no_dir.list().expect("list should succeed");
        assert!(list.is_empty());
    }

    #[test]
    fn add_and_list() {
        let (_dir, store) = store_in_tempdir();
        let e = entry("e1", "run-1", OverlayEntryKind::Note);
        store.add(e.clone()).expect("add");

        let list = store.list().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "e1");
        assert_eq!(list[0].promoting_run_id, "run-1");
    }

    #[test]
    fn add_generates_id_when_empty() {
        let (_dir, store) = store_in_tempdir();
        let mut e = entry("", "run-2", OverlayEntryKind::Workflow);
        e.id = String::new();
        let added = store.add(e).expect("add");
        assert!(!added.id.is_empty(), "id should be auto-generated");
        assert_eq!(store.list().expect("list").len(), 1);
    }

    #[test]
    fn add_duplicate_id_is_rejected() {
        let (_dir, store) = store_in_tempdir();
        let e = entry("dup", "run-3", OverlayEntryKind::Test);
        store.add(e.clone()).expect("first add");
        let result = store.add(e);
        assert!(result.is_err(), "second add with same id should fail");
    }

    #[test]
    fn get_returns_none_for_missing_entry() {
        let (_dir, store) = store_in_tempdir();
        assert!(store.get("nonexistent").expect("get").is_none());
    }

    #[test]
    fn get_returns_entry() {
        let (_dir, store) = store_in_tempdir();
        let e = entry("g1", "run-4", OverlayEntryKind::BranchHeuristic);
        store.add(e).expect("add");
        let fetched = store.get("g1").expect("get").expect("should be present");
        assert_eq!(fetched.id, "g1");
        assert_eq!(fetched.kind, OverlayEntryKind::BranchHeuristic);
    }

    #[test]
    fn remove_returns_false_for_missing() {
        let (_dir, store) = store_in_tempdir();
        assert!(!store.remove("absent").expect("remove"));
    }

    #[test]
    fn remove_returns_true_and_entry_gone() {
        let (_dir, store) = store_in_tempdir();
        let e = entry("r1", "run-5", OverlayEntryKind::PromptPatch);
        store.add(e).expect("add");
        assert!(store.remove("r1").expect("remove"));
        assert!(store.list().expect("list").is_empty());
    }

    #[test]
    fn list_by_run_filters_correctly() {
        let (_dir, store) = store_in_tempdir();
        store
            .add(entry("a1", "run-A", OverlayEntryKind::Note))
            .expect("add a1");
        store
            .add(entry("b1", "run-B", OverlayEntryKind::Note))
            .expect("add b1");
        store
            .add(entry("a2", "run-A", OverlayEntryKind::Test))
            .expect("add a2");

        let run_a = store.list_by_run("run-A").expect("list by run");
        assert_eq!(run_a.len(), 2);
        assert!(run_a.iter().all(|e| e.promoting_run_id == "run-A"));
    }

    #[test]
    fn remove_by_run_removes_all_attributed() {
        let (_dir, store) = store_in_tempdir();
        store
            .add(entry("x1", "run-X", OverlayEntryKind::Note))
            .expect("add x1");
        store
            .add(entry("x2", "run-X", OverlayEntryKind::Workflow))
            .expect("add x2");
        store
            .add(entry("y1", "run-Y", OverlayEntryKind::Note))
            .expect("add y1");

        let removed = store.remove_by_run("run-X").expect("remove by run");
        assert_eq!(removed.len(), 2);

        let remaining = store.list().expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "y1");
    }

    #[test]
    fn list_by_kind_filters_correctly() {
        let (_dir, store) = store_in_tempdir();
        store
            .add(entry("k1", "run-K", OverlayEntryKind::Note))
            .expect("add k1");
        store
            .add(entry("k2", "run-K", OverlayEntryKind::Test))
            .expect("add k2");
        store
            .add(entry("k3", "run-K", OverlayEntryKind::Note))
            .expect("add k3");

        let notes = store
            .list_by_kind(OverlayEntryKind::Note)
            .expect("list by kind");
        assert_eq!(notes.len(), 2);
        assert!(notes.iter().all(|e| e.kind == OverlayEntryKind::Note));
    }

    #[test]
    fn list_sorted_by_promoted_at() {
        let (_dir, store) = store_in_tempdir();
        let mut e1 = entry("s1", "run-S", OverlayEntryKind::Note);
        e1.promoted_at = "2026-08-01T00:00:00Z".to_string();
        let mut e2 = entry("s2", "run-S", OverlayEntryKind::Note);
        e2.promoted_at = "2026-08-03T00:00:00Z".to_string();
        let mut e3 = entry("s3", "run-S", OverlayEntryKind::Note);
        e3.promoted_at = "2026-08-02T00:00:00Z".to_string();

        // Add in random order
        store.add(e2).expect("add e2");
        store.add(e1).expect("add e1");
        store.add(e3).expect("add e3");

        let list = store.list().expect("list");
        assert_eq!(list[0].id, "s1");
        assert_eq!(list[1].id, "s3");
        assert_eq!(list[2].id, "s2");
    }
}
