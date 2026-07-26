//! Durable attribution and selective-review ledger for agent-authored hunks.
//!
//! The ledger is separate from the user's repository and side-git commit graph.
//! A selective reject matches the recorded post-image before reversing it, so
//! later user or external edits stop with a conflict instead of being lost.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LEDGER_SCHEMA_VERSION: u32 = 1;
const LEDGER_FILE: &str = "hunk-ledger.v1.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HunkAttribution {
    Agent {
        agent_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_name: Option<String>,
    },
    RootTurn,
    External,
    Unattributed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HunkState {
    Pending,
    Accepted,
    Rejected,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HunkRecord {
    pub id: String,
    pub turn_id: String,
    pub tool_call_id: String,
    pub path: PathBuf,
    pub base_snapshot: String,
    pub base_digest: String,
    pub result_digest: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub attribution: HunkAttribution,
    pub state: HunkState,
    #[serde(default)]
    pub snapshot_pruned: bool,
}

impl HunkRecord {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        turn_id: impl Into<String>,
        tool_call_id: impl Into<String>,
        path: impl Into<PathBuf>,
        base_snapshot: impl Into<String>,
        before: Vec<u8>,
        after: Vec<u8>,
        attribution: HunkAttribution,
    ) -> Self {
        let turn_id = turn_id.into();
        let tool_call_id = tool_call_id.into();
        let path = path.into();
        let base_snapshot = base_snapshot.into();
        let base_digest = digest(&before);
        let result_digest = digest(&after);
        let mut stable = Sha256::new();
        for part in [
            turn_id.as_bytes(),
            tool_call_id.as_bytes(),
            path.to_string_lossy().as_bytes(),
            base_snapshot.as_bytes(),
            base_digest.as_bytes(),
            result_digest.as_bytes(),
        ] {
            stable.update((part.len() as u64).to_le_bytes());
            stable.update(part);
        }
        let id = format!("hunk-{}", hex_digest(stable.finalize().as_ref()));
        Self {
            id,
            turn_id,
            tool_call_id,
            path,
            base_snapshot,
            base_digest,
            result_digest,
            before,
            after,
            attribution,
            state: HunkState::Pending,
            snapshot_pruned: false,
        }
    }

    #[must_use]
    pub fn lineage_available(&self) -> bool {
        !self.snapshot_pruned
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HunkLedger {
    schema_version: u32,
    #[serde(default)]
    records: Vec<HunkRecord>,
}

impl Default for HunkLedger {
    fn default() -> Self {
        Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectiveReviewOutcome {
    Accepted,
    Rejected,
    Conflict { expected: String, actual: String },
}

impl HunkLedger {
    #[must_use]
    pub fn records(&self) -> &[HunkRecord] {
        &self.records
    }

    pub fn record(&mut self, record: HunkRecord) -> bool {
        if self.records.iter().any(|item| item.id == record.id) {
            return false;
        }
        self.records.push(record);
        true
    }

    #[must_use]
    pub fn for_turn(&self, turn_id: &str) -> Vec<&HunkRecord> {
        self.records
            .iter()
            .filter(|record| record.turn_id == turn_id)
            .collect()
    }

    pub fn mark_snapshot_pruned(&mut self, snapshot: &str) -> usize {
        let mut changed = 0;
        for record in &mut self.records {
            if record.base_snapshot == snapshot && !record.snapshot_pruned {
                record.snapshot_pruned = true;
                changed += 1;
            }
        }
        changed
    }

    pub fn accept(&mut self, id: &str) -> io::Result<SelectiveReviewOutcome> {
        let record = self.record_mut(id)?;
        record.state = HunkState::Accepted;
        Ok(SelectiveReviewOutcome::Accepted)
    }

    /// Reject one mutation only when the workspace still contains its exact
    /// post-image. Later or external edits are never guessed through.
    pub fn reject(&mut self, workspace: &Path, id: &str) -> io::Result<SelectiveReviewOutcome> {
        let record = self.record_mut(id)?;
        let target = workspace.join(&record.path);
        let current = match fs::read(&target) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        let actual = digest(&current);
        if actual != record.result_digest {
            record.state = HunkState::Conflict;
            return Ok(SelectiveReviewOutcome::Conflict {
                expected: record.result_digest.clone(),
                actual,
            });
        }
        if record.before.is_empty() {
            match fs::remove_file(&target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            crate::utils::write_atomic_workspace(&target, &record.before)?;
        }
        record.state = HunkState::Rejected;
        Ok(SelectiveReviewOutcome::Rejected)
    }

    pub fn load(side_repo_dir: &Path) -> io::Result<Self> {
        let path = side_repo_dir.join(LEDGER_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let ledger: Self = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if ledger.schema_version != LEDGER_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported hunk ledger schema {}", ledger.schema_version),
            ));
        }
        Ok(ledger)
    }

    pub fn save(&self, side_repo_dir: &Path) -> io::Result<()> {
        fs::create_dir_all(side_repo_dir)?;
        let path = side_repo_dir.join(LEDGER_FILE);
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        crate::utils::write_atomic_workspace(&path, &bytes)
    }

    fn record_mut(&mut self, id: &str) -> io::Result<&mut HunkRecord> {
        self.records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("unknown hunk {id}")))
    }
}

#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex_digest(Sha256::digest(bytes).as_ref()))
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn record(path: &str, before: &[u8], after: &[u8]) -> HunkRecord {
        HunkRecord::new(
            "turn-7",
            "tool-3",
            path,
            "pre-turn:7",
            before.to_vec(),
            after.to_vec(),
            HunkAttribution::Agent {
                agent_id: "agent_123".into(),
                agent_name: Some("Builder".into()),
            },
        )
    }

    #[test]
    fn ids_and_attribution_are_stable_and_deduplicated() {
        let a = record("src/鲸.rs", "alpha\r\n".as_bytes(), "βeta\r\n".as_bytes());
        let b = record("src/鲸.rs", "alpha\r\n".as_bytes(), "βeta\r\n".as_bytes());
        assert_eq!(a.id, b.id);
        assert!(matches!(a.attribution, HunkAttribution::Agent { .. }));
        let mut ledger = HunkLedger::default();
        assert!(ledger.record(a));
        assert!(!ledger.record(b));
        assert_eq!(ledger.for_turn("turn-7").len(), 1);
    }

    #[test]
    fn persistence_keeps_binary_crlf_unicode_and_lineage() {
        let tmp = tempdir().unwrap();
        let mut ledger = HunkLedger::default();
        ledger.record(record("bin/鲸.dat", &[0, 255, b'\r', b'\n'], &[1, 0, 2]));
        ledger.save(tmp.path()).unwrap();
        let mut loaded = HunkLedger::load(tmp.path()).unwrap();
        assert_eq!(loaded, ledger);
        assert_eq!(loaded.mark_snapshot_pruned("pre-turn:7"), 1);
        assert!(!loaded.records()[0].lineage_available());
    }

    #[test]
    fn selective_reject_restores_rename_and_removes_untracked() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/new.rs"), b"new\n").unwrap();
        let mut ledger = HunkLedger::default();
        let renamed = record("src/new.rs", b"old\n", b"new\n");
        let id = renamed.id.clone();
        ledger.record(renamed);
        assert_eq!(
            ledger.reject(tmp.path(), &id).unwrap(),
            SelectiveReviewOutcome::Rejected
        );
        assert_eq!(fs::read(tmp.path().join("src/new.rs")).unwrap(), b"old\n");

        fs::write(tmp.path().join("scratch.bin"), [1, 2, 3]).unwrap();
        let created = record("scratch.bin", b"", &[1, 2, 3]);
        let id = created.id.clone();
        ledger.record(created);
        assert_eq!(
            ledger.reject(tmp.path(), &id).unwrap(),
            SelectiveReviewOutcome::Rejected
        );
        assert!(!tmp.path().join("scratch.bin").exists());
    }

    #[test]
    fn overlapping_external_edit_stops_without_partial_apply() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("file.txt"), b"agent plus user\n").unwrap();
        let mut ledger = HunkLedger::default();
        let edit = record("file.txt", b"base\n", b"agent\n");
        let id = edit.id.clone();
        ledger.record(edit);
        let outcome = ledger.reject(tmp.path(), &id).unwrap();
        assert!(matches!(outcome, SelectiveReviewOutcome::Conflict { .. }));
        assert_eq!(
            fs::read(tmp.path().join("file.txt")).unwrap(),
            b"agent plus user\n"
        );
        assert_eq!(ledger.records()[0].state, HunkState::Conflict);
    }

    #[test]
    fn accepted_hunk_never_mutates_workspace() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("x"), b"after").unwrap();
        let mut ledger = HunkLedger::default();
        let item = record("x", b"before", b"after");
        let id = item.id.clone();
        ledger.record(item);
        assert_eq!(
            ledger.accept(&id).unwrap(),
            SelectiveReviewOutcome::Accepted
        );
        assert_eq!(fs::read(tmp.path().join("x")).unwrap(), b"after");
    }
}
