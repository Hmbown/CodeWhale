use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sandbox::SandboxPolicy;

const APPROVAL_LOG_FILE: &str = "approval_receipts.jsonl";
const APPROVAL_LOCK_FILE: &str = "approval_receipts.lock";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum ApprovalOutcome {
    ApprovedOnce,
    Denied,
    Cancelled,
    Unavailable,
    RetryWithPolicy { policy: SandboxPolicy },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum ApprovalReceipt {
    Asked {
        approval_id: String,
        tool_call_id: String,
        tool_name: String,
        created_at: DateTime<Utc>,
    },
    Decided {
        approval_id: String,
        tool_call_id: String,
        outcome: ApprovalOutcome,
        created_at: DateTime<Utc>,
    },
}

impl ApprovalReceipt {
    pub(crate) fn asked(tool_call_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        let tool_call_id = tool_call_id.into();
        Self::Asked {
            approval_id: tool_call_id.clone(),
            tool_call_id,
            tool_name: tool_name.into(),
            created_at: Utc::now(),
        }
    }

    pub(crate) fn decided(tool_call_id: impl Into<String>, outcome: ApprovalOutcome) -> Self {
        let tool_call_id = tool_call_id.into();
        Self::Decided {
            approval_id: tool_call_id.clone(),
            tool_call_id,
            outcome,
            created_at: Utc::now(),
        }
    }

    fn approval_id(&self) -> &str {
        match self {
            Self::Asked { approval_id, .. } | Self::Decided { approval_id, .. } => approval_id,
        }
    }

    fn tool_call_id(&self) -> &str {
        match self {
            Self::Asked { tool_call_id, .. } | Self::Decided { tool_call_id, .. } => tool_call_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletedApproval {
    pub(crate) ask: ApprovalReceipt,
    pub(crate) outcome: ApprovalOutcome,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ApprovalReplay {
    pub(crate) completed: Vec<CompletedApproval>,
    pub(crate) unmatched_asks: Vec<ApprovalReceipt>,
}

impl ApprovalReplay {
    pub(crate) fn from_receipts(receipts: &[ApprovalReceipt]) -> Result<Self, String> {
        let mut open = BTreeMap::<String, ApprovalReceipt>::new();
        let mut closed = BTreeSet::<String>::new();
        let mut completed = Vec::new();

        for receipt in receipts {
            let approval_id = receipt.approval_id();
            if approval_id.trim().is_empty() || receipt.tool_call_id().trim().is_empty() {
                return Err("approval receipt has an empty correlation id".to_string());
            }
            match receipt {
                ApprovalReceipt::Asked {
                    approval_id,
                    tool_call_id,
                    tool_name,
                    ..
                } => {
                    if tool_name.trim().is_empty() {
                        return Err(format!(
                            "approval ask '{approval_id}' has an empty tool name"
                        ));
                    }
                    if approval_id != tool_call_id {
                        return Err(format!(
                            "approval ask '{approval_id}' does not match tool call '{tool_call_id}'"
                        ));
                    }
                    // Provider tool_call_id values are reused across turns.
                    // An exact duplicate Asked is ignored; a later distinct
                    // Asked last-write-wins the open slot (or starts a new
                    // cycle after the previous one closed).
                    if open.get(approval_id) == Some(receipt) {
                        continue;
                    }
                    closed.remove(approval_id);
                    open.insert(approval_id.clone(), receipt.clone());
                }
                ApprovalReceipt::Decided {
                    approval_id,
                    tool_call_id,
                    outcome,
                    ..
                } => {
                    if approval_id != tool_call_id {
                        return Err(format!(
                            "approval decision '{approval_id}' does not match tool call '{tool_call_id}'"
                        ));
                    }
                    let Some(ask) = open.remove(approval_id) else {
                        return Err(format!(
                            "approval decision '{approval_id}' has no unmatched ask"
                        ));
                    };
                    closed.insert(approval_id.clone());
                    completed.push(CompletedApproval {
                        ask,
                        outcome: outcome.clone(),
                    });
                }
            }
        }

        Ok(Self {
            completed,
            unmatched_asks: open.into_values().collect(),
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ApprovalReceiptStore {
    sessions_dir: PathBuf,
}

impl ApprovalReceiptStore {
    pub(crate) fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn default_location() -> io::Result<Self> {
        crate::session_manager::default_sessions_dir().map(Self::new)
    }

    fn validated_session_id(session_id: &str) -> io::Result<&str> {
        let trimmed = session_id.trim();
        if trimmed.is_empty()
            || !trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid session id '{session_id}'"),
            ));
        }
        Ok(trimmed)
    }

    fn log_path(&self, session_id: &str) -> io::Result<PathBuf> {
        let session_id = Self::validated_session_id(session_id)?;
        Ok(self.sessions_dir.join(session_id).join(APPROVAL_LOG_FILE))
    }

    fn lock_path(&self, session_id: &str) -> io::Result<PathBuf> {
        let session_id = Self::validated_session_id(session_id)?;
        Ok(self.sessions_dir.join(session_id).join(APPROVAL_LOCK_FILE))
    }

    fn open_lock_file(&self, session_id: &str) -> io::Result<File> {
        let path = self.lock_path(session_id)?;
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "approval lock has no parent")
        })?;
        fs::create_dir_all(parent)?;
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
    }

    fn open_existing_lock_file(&self, session_id: &str) -> io::Result<Option<File>> {
        let path = self.lock_path(session_id)?;
        match OpenOptions::new().read(true).open(path) {
            Ok(file) => Ok(Some(file)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn load_unlocked(&self, session_id: &str) -> io::Result<Vec<ApprovalReceipt>> {
        let path = self.log_path(session_id)?;
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let mut receipts = Vec::new();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let line = line?;
            let receipt = serde_json::from_str(&line).map_err(|err| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("invalid approval receipt at line {}: {err}", index + 1),
                )
            })?;
            receipts.push(receipt);
        }
        ApprovalReplay::from_receipts(&receipts)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        Ok(receipts)
    }

    pub(crate) fn load(&self, session_id: &str) -> io::Result<Vec<ApprovalReceipt>> {
        if !self.log_path(session_id)?.exists() {
            return Ok(Vec::new());
        }
        let Some(lock_file) = self.open_existing_lock_file(session_id)? else {
            // Imported or legacy snapshots can contain a receipt log without
            // its ephemeral lock file. Preserve read-only session loading;
            // live writers always publish the lock before creating the log.
            return self.load_unlocked(session_id);
        };
        let lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock.read()?;
        self.load_unlocked(session_id)
    }

    pub(crate) fn replay(&self, session_id: &str) -> io::Result<ApprovalReplay> {
        let receipts = self.load(session_id)?;
        ApprovalReplay::from_receipts(&receipts)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    pub(crate) fn append(&self, session_id: &str, receipt: &ApprovalReceipt) -> io::Result<()> {
        let lock_file = self.open_lock_file(session_id)?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock.write()?;
        let path = self.log_path(session_id)?;
        let mut candidate = self.load_unlocked(session_id)?;
        candidate.push(receipt.clone());
        ApprovalReplay::from_receipts(&candidate)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "approval log has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let mut line = serde_json::to_vec(receipt)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        line.push(b'\n');
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(&line)?;
        file.sync_all()?;
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn sessions_dir(&self) -> &std::path::Path {
        &self.sessions_dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed_receipts(outcome: ApprovalOutcome) -> Vec<ApprovalReceipt> {
        vec![
            ApprovalReceipt::asked("tool-1", "exec_shell"),
            ApprovalReceipt::decided("tool-1", outcome),
        ]
    }

    #[test]
    fn replay_reconstructs_every_closed_outcome() {
        let outcomes = [
            ApprovalOutcome::ApprovedOnce,
            ApprovalOutcome::Denied,
            ApprovalOutcome::Cancelled,
            ApprovalOutcome::Unavailable,
            ApprovalOutcome::RetryWithPolicy {
                policy: crate::sandbox::SandboxPolicy::DangerFullAccess,
            },
        ];

        for outcome in outcomes {
            let replay = ApprovalReplay::from_receipts(&completed_receipts(outcome.clone()))
                .expect("closed approval log replays");
            assert_eq!(replay.completed.len(), 1);
            assert_eq!(replay.completed[0].outcome, outcome);
            assert!(replay.unmatched_asks.is_empty());
        }
    }

    #[test]
    fn replay_detects_an_unmatched_ask_after_interruption() {
        let ask = ApprovalReceipt::asked("tool-interrupted", "write_file");
        let replay = ApprovalReplay::from_receipts(std::slice::from_ref(&ask))
            .expect("an unmatched ask is valid crash evidence");

        assert!(replay.completed.is_empty());
        assert_eq!(replay.unmatched_asks, vec![ask]);
    }

    #[test]
    fn replay_rejects_decisions_without_one_open_ask() {
        let orphan = ApprovalReceipt::decided("tool-orphan", ApprovalOutcome::ApprovedOnce);
        assert!(ApprovalReplay::from_receipts(&[orphan]).is_err());

        let duplicate = vec![
            ApprovalReceipt::asked("tool-duplicate", "exec_shell"),
            ApprovalReceipt::decided("tool-duplicate", ApprovalOutcome::Denied),
            ApprovalReceipt::decided("tool-duplicate", ApprovalOutcome::ApprovedOnce),
        ];
        assert!(ApprovalReplay::from_receipts(&duplicate).is_err());
    }

    #[test]
    fn replay_ignores_an_exact_duplicate_ask() {
        let ask = ApprovalReceipt::asked("call-reused", "exec_shell");
        let replay = ApprovalReplay::from_receipts(&[ask.clone(), ask.clone()])
            .expect("exact duplicate Asked is not a hard error");

        assert!(replay.completed.is_empty());
        assert_eq!(replay.unmatched_asks, vec![ask]);
    }

    #[test]
    fn replay_last_write_wins_an_open_ask_for_the_same_id() {
        let first = ApprovalReceipt::asked("call-reused", "exec_shell");
        let second = ApprovalReceipt::asked("call-reused", "write_file");
        let replay = ApprovalReplay::from_receipts(&[first, second.clone()])
            .expect("a later distinct Asked replaces the open one");

        assert!(replay.completed.is_empty());
        assert_eq!(replay.unmatched_asks, vec![second]);
    }

    #[test]
    fn replay_accepts_provider_tool_call_id_reuse_across_turns() {
        let first = ApprovalReceipt::asked("call-reused", "exec_shell");
        let decided = ApprovalReceipt::decided("call-reused", ApprovalOutcome::ApprovedOnce);
        let second = ApprovalReceipt::asked("call-reused", "write_file");
        let replay = ApprovalReplay::from_receipts(&[first.clone(), decided, second.clone()])
            .expect("a reused provider tool_call_id starts a new cycle");

        assert_eq!(replay.completed.len(), 1);
        assert_eq!(replay.completed[0].ask, first);
        assert_eq!(replay.completed[0].outcome, ApprovalOutcome::ApprovedOnce);
        assert_eq!(replay.unmatched_asks, vec![second]);
    }

    #[test]
    fn replay_keeps_each_closed_cycle_when_the_call_id_is_reused() {
        let first = ApprovalReceipt::asked("call-reused", "exec_shell");
        let first_decision = ApprovalReceipt::decided("call-reused", ApprovalOutcome::ApprovedOnce);
        let second = ApprovalReceipt::asked("call-reused", "write_file");
        let second_decision = ApprovalReceipt::decided("call-reused", ApprovalOutcome::Denied);
        let replay = ApprovalReplay::from_receipts(&[
            first.clone(),
            first_decision,
            second.clone(),
            second_decision,
        ])
        .expect("each closed cycle is retained");

        assert_eq!(replay.completed.len(), 2);
        assert_eq!(replay.completed[0].ask, first);
        assert_eq!(replay.completed[0].outcome, ApprovalOutcome::ApprovedOnce);
        assert_eq!(replay.completed[1].ask, second);
        assert_eq!(replay.completed[1].outcome, ApprovalOutcome::Denied);
        assert!(replay.unmatched_asks.is_empty());
    }

    #[test]
    fn store_appends_a_reused_tool_call_id_as_a_new_cycle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = ApprovalReceiptStore::new(tmp.path().join("sessions"));
        let first = ApprovalReceipt::asked("call-reused", "exec_shell");
        let decided = ApprovalReceipt::decided("call-reused", ApprovalOutcome::ApprovedOnce);
        let second = ApprovalReceipt::asked("call-reused", "write_file");

        store
            .append("session-1", &first)
            .expect("persist first ask");
        store
            .append("session-1", &decided)
            .expect("persist first decision");
        store
            .append("session-1", &second)
            .expect("reused tool_call_id must append");

        let receipts = store.load("session-1").expect("load reused-id log");
        assert_eq!(receipts, vec![first, decided, second]);
        let replay = ApprovalReplay::from_receipts(&receipts).expect("replay reused-id log");
        assert_eq!(replay.completed.len(), 1);
        assert_eq!(replay.unmatched_asks.len(), 1);
    }

    #[test]
    fn store_appends_and_replays_a_session_owned_log() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = ApprovalReceiptStore::new(tmp.path().join("sessions"));
        let ask = ApprovalReceipt::asked("tool-persisted", "edit_file");
        let decision = ApprovalReceipt::decided("tool-persisted", ApprovalOutcome::ApprovedOnce);

        store
            .append("session-1", &ask)
            .expect("persist approval ask");
        store
            .append("session-1", &decision)
            .expect("persist approval decision");

        let receipts = store.load("session-1").expect("load approval log");
        assert_eq!(receipts, vec![ask, decision]);
        let replay = ApprovalReplay::from_receipts(&receipts).expect("replay approval log");
        assert_eq!(replay.completed.len(), 1);
        assert!(replay.unmatched_asks.is_empty());
    }

    #[test]
    fn loading_missing_or_imported_logs_is_read_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let store = ApprovalReceiptStore::new(sessions_dir.clone());

        assert!(store.load("session-empty").expect("missing log").is_empty());
        assert!(!sessions_dir.join("session-empty").exists());

        let imported_dir = sessions_dir.join("session-imported");
        fs::create_dir_all(&imported_dir).expect("imported session dir");
        let ask = ApprovalReceipt::asked("tool-imported", "exec_shell");
        let mut line = serde_json::to_vec(&ask).expect("serialize imported ask");
        line.push(b'\n');
        fs::write(imported_dir.join(APPROVAL_LOG_FILE), line).expect("imported log");

        assert_eq!(
            store.load("session-imported").expect("load imported log"),
            vec![ask]
        );
        assert!(!imported_dir.join(APPROVAL_LOCK_FILE).exists());
    }

    #[test]
    fn store_serializes_competing_writers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = ApprovalReceiptStore::new(tmp.path().join("sessions"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let mut writers = Vec::new();

        for _ in 0..8 {
            let store = store.clone();
            let barrier = barrier.clone();
            writers.push(std::thread::spawn(move || {
                barrier.wait();
                store.append(
                    "session-race",
                    &ApprovalReceipt::asked("tool-race", "exec_shell"),
                )
            }));
        }

        let results = writers
            .into_iter()
            .map(|writer| writer.join().expect("writer thread"))
            .collect::<Vec<_>>();
        assert!(
            results.iter().all(Result::is_ok),
            "a duplicate Asked for the same id must not fail the writer: {results:?}"
        );
        let receipts = store.load("session-race").expect("load receipts");
        assert_eq!(receipts.len(), 8);
        let replay = ApprovalReplay::from_receipts(&receipts).expect("replay raced asks");
        assert!(replay.completed.is_empty());
        assert_eq!(replay.unmatched_asks.len(), 1);
    }
}
