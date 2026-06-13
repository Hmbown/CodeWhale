//! Durable fleet inbox and run ledger.
//!
//! Stores fleet state as append-only JSONL so the manager can survive
//! restarts and reconstruct queue/worker state by replaying records.
//! Artifacts are referenced by bounded metadata; large payloads live on disk
//! and are never embedded in the ledger.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use codewhale_protocol::fleet::*;
use serde::{Deserialize, Serialize};

const FLEET_DIR: &str = ".codewhale";
const FLEET_LEDGER_FILE: &str = "fleet.jsonl";
const PARTIAL_SUFFIX: &str = ".tmp";

/// A single append-only record in the fleet ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum FleetLedgerRecord {
    RunCreated {
        run: FleetRun,
    },
    RunStatusChanged {
        run_id: FleetRunId,
        status: FleetRunStatus,
        timestamp: String,
    },
    TaskEnqueued {
        entry: FleetInboxEntry,
    },
    TaskLeased {
        run_id: FleetRunId,
        task_id: String,
        worker_id: String,
        leased_at: String,
        lease_expires_at: Option<String>,
    },
    TaskCompletedOrFailed {
        run_id: FleetRunId,
        task_id: String,
        worker_id: String,
        timestamp: String,
        #[serde(default = "default_terminal_task_status")]
        status: FleetTaskLedgerStatus,
    },
    EventAppended {
        event: FleetWorkerEvent,
    },
    Heartbeat {
        worker_id: String,
        timestamp: String,
        #[serde(default)]
        cpu_percent: Option<f32>,
        #[serde(default)]
        memory_mb: Option<u64>,
    },
    ReceiptRecorded {
        receipt: FleetReceipt,
    },
    AlertSent {
        run_id: FleetRunId,
        task_id: String,
        channel: String,
        timestamp: String,
    },
}

/// Reconstructed fleet state after replaying the ledger.
#[derive(Debug, Clone, Default)]
pub struct FleetLedgerState {
    pub runs: BTreeMap<String, FleetRun>,
    pub run_status_overrides: BTreeMap<String, FleetRunStatus>,
    /// Tasks keyed by run_id:task_id.
    pub tasks: BTreeMap<String, FleetTaskState>,
    /// Worker status by worker_id.
    pub workers: BTreeMap<String, FleetWorkerStatus>,
    /// Latest heartbeat by worker_id.
    pub heartbeats: BTreeMap<String, FleetHeartbeatState>,
    /// Latest event seq per worker_id:task_id.
    pub latest_seq: BTreeMap<String, u64>,
    /// Latest event envelope per worker_id:run_id:task_id.
    pub latest_events: BTreeMap<String, FleetWorkerEvent>,
    /// Completed receipts by run_id:task_id.
    pub receipts: BTreeMap<String, FleetReceipt>,
}

#[derive(Debug, Clone)]
pub struct FleetTaskState {
    pub entry: FleetInboxEntry,
    pub status: FleetTaskLedgerStatus,
    pub leased_to: Option<String>,
    pub leased_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetTaskLedgerStatus {
    Enqueued,
    Leased,
    Completed,
    Failed,
    Cancelled,
}

fn default_terminal_task_status() -> FleetTaskLedgerStatus {
    FleetTaskLedgerStatus::Completed
}

#[derive(Debug, Clone)]
pub struct FleetHeartbeatState {
    pub timestamp: String,
    pub cpu_percent: Option<f32>,
    pub memory_mb: Option<u64>,
}

/// Append-only JSONL ledger for fleet runs.
pub struct FleetLedger {
    ledger_path: PathBuf,
}

impl FleetLedger {
    /// Open (or create) the ledger under `workspace/.codewhale/fleet.jsonl`.
    pub fn open(workspace: &Path) -> Result<Self> {
        let dir = workspace.join(FLEET_DIR);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating fleet ledger dir {}", dir.display()))?;
        let ledger_path = dir.join(FLEET_LEDGER_FILE);
        if !ledger_path.exists() {
            std::fs::write(&ledger_path, "")
                .with_context(|| format!("creating fleet ledger {}", ledger_path.display()))?;
        }
        Ok(Self { ledger_path })
    }

    pub fn path(&self) -> &Path {
        &self.ledger_path
    }

    /// Append a single record without rewriting existing ledger contents.
    fn append_record(&self, record: &FleetLedgerRecord) -> Result<()> {
        let mut line = serde_json::to_string(record).context("serializing fleet ledger record")?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.ledger_path)
            .with_context(|| format!("opening fleet ledger {}", self.ledger_path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("appending fleet ledger {}", self.ledger_path.display()))?;
        file.flush()
            .with_context(|| format!("flushing fleet ledger {}", self.ledger_path.display()))?;
        file.sync_data()
            .with_context(|| format!("syncing fleet ledger {}", self.ledger_path.display()))?;
        Ok(())
    }

    pub fn create_run(&self, run: &FleetRun) -> Result<()> {
        self.append_record(&FleetLedgerRecord::RunCreated { run: run.clone() })
    }

    pub fn update_run_status(
        &self,
        run_id: &FleetRunId,
        status: FleetRunStatus,
        timestamp: &str,
    ) -> Result<()> {
        self.append_record(&FleetLedgerRecord::RunStatusChanged {
            run_id: run_id.clone(),
            status,
            timestamp: timestamp.to_string(),
        })
    }

    pub fn enqueue(&self, entry: FleetInboxEntry) -> Result<()> {
        self.append_record(&FleetLedgerRecord::TaskEnqueued { entry })
    }

    /// Mark a task as leased to a worker.
    pub fn lease_task(
        &self,
        run_id: &FleetRunId,
        task_id: &str,
        worker_id: &str,
        leased_at: &str,
        lease_expires_at: Option<&str>,
    ) -> Result<()> {
        self.append_record(&FleetLedgerRecord::TaskLeased {
            run_id: run_id.clone(),
            task_id: task_id.to_string(),
            worker_id: worker_id.to_string(),
            leased_at: leased_at.to_string(),
            lease_expires_at: lease_expires_at.map(String::from),
        })
    }

    /// Mark a task as completed or failed.
    pub fn complete_or_fail_task(
        &self,
        run_id: &FleetRunId,
        task_id: &str,
        worker_id: &str,
        timestamp: &str,
    ) -> Result<()> {
        self.append_record(&FleetLedgerRecord::TaskCompletedOrFailed {
            run_id: run_id.clone(),
            task_id: task_id.to_string(),
            worker_id: worker_id.to_string(),
            timestamp: timestamp.to_string(),
            status: FleetTaskLedgerStatus::Completed,
        })
    }

    pub fn append_event(&self, event: FleetWorkerEvent) -> Result<()> {
        self.append_record(&FleetLedgerRecord::EventAppended { event })
    }

    pub fn heartbeat(
        &self,
        worker_id: &str,
        timestamp: &str,
        cpu_percent: Option<f32>,
        memory_mb: Option<u64>,
    ) -> Result<()> {
        self.append_record(&FleetLedgerRecord::Heartbeat {
            worker_id: worker_id.to_string(),
            timestamp: timestamp.to_string(),
            cpu_percent,
            memory_mb,
        })
    }

    pub fn record_receipt(&self, receipt: FleetReceipt) -> Result<()> {
        self.append_record(&FleetLedgerRecord::ReceiptRecorded { receipt })
    }

    pub fn record_alert(
        &self,
        run_id: &FleetRunId,
        task_id: &str,
        channel: &str,
        timestamp: &str,
    ) -> Result<()> {
        self.append_record(&FleetLedgerRecord::AlertSent {
            run_id: run_id.clone(),
            task_id: task_id.to_string(),
            channel: channel.to_string(),
            timestamp: timestamp.to_string(),
        })
    }

    /// Replay the ledger and reconstruct current state. Malformed or partial
    /// lines are skipped so an interrupted write cannot corrupt earlier state.
    pub fn rebuild_state(&self) -> Result<FleetLedgerState> {
        let mut state = FleetLedgerState::default();
        if !self.ledger_path.exists() {
            return Ok(state);
        }
        let file = std::fs::File::open(&self.ledger_path)
            .with_context(|| format!("opening ledger {}", self.ledger_path.display()))?;
        let reader = std::io::BufReader::new(file);
        for (line_no, line) in reader.lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(err) => {
                    tracing::warn!("fleet ledger line {} unreadable: {}", line_no + 1, err);
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let record: FleetLedgerRecord = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(
                        "fleet ledger line {} parse error (skipping): {}",
                        line_no + 1,
                        err
                    );
                    continue;
                }
            };
            apply_record(&mut state, record);
        }
        Ok(state)
    }

    /// Claim the next available inbox task for `worker_id`. Returns the
    /// enqueued entry and appends a lease record.
    pub fn claim_next(
        &self,
        worker_id: &str,
        _worker_capabilities: &[String],
        timestamp: &str,
    ) -> Result<Option<FleetInboxEntry>> {
        let state = self.rebuild_state()?;
        // Find oldest enqueued task whose task spec (if known) matches worker
        // capabilities. For now, tasks without specs match everything.
        let candidate = state
            .tasks
            .values()
            .filter(|t| matches!(t.status, FleetTaskLedgerStatus::Enqueued))
            .map(|t| &t.entry)
            .min_by_key(|e| (e.priority, e.enqueued_at.clone()))
            .cloned();
        let Some(entry) = candidate else {
            return Ok(None);
        };
        self.lease_task(&entry.run_id, &entry.task_id, worker_id, timestamp, None)?;
        Ok(Some(entry))
    }

    /// Compact the ledger by rewriting only the records needed to reconstruct
    /// current state. This truncates history but preserves run/task/event
    /// metadata and receipts.
    pub fn compact(&self) -> Result<()> {
        let state = self.rebuild_state()?;
        let tmp_path = self.ledger_path.with_extension(PARTIAL_SUFFIX);
        let mut lines = Vec::new();
        for run in state.runs.values() {
            lines.push(serde_json::to_string(&FleetLedgerRecord::RunCreated {
                run: run.clone(),
            })?);
            if let Some(status) = state.run_status_overrides.get(&run.id.0) {
                lines.push(serde_json::to_string(
                    &FleetLedgerRecord::RunStatusChanged {
                        run_id: run.id.clone(),
                        status: status.clone(),
                        timestamp: run.updated_at.clone().unwrap_or_default(),
                    },
                )?);
            }
        }
        for task in state.tasks.values() {
            lines.push(serde_json::to_string(&FleetLedgerRecord::TaskEnqueued {
                entry: task.entry.clone(),
            })?);
            if let Some(worker) = &task.leased_to {
                lines.push(serde_json::to_string(&FleetLedgerRecord::TaskLeased {
                    run_id: task.entry.run_id.clone(),
                    task_id: task.entry.task_id.clone(),
                    worker_id: worker.clone(),
                    leased_at: task.leased_at.clone().unwrap_or_default(),
                    lease_expires_at: None,
                })?);
            }
            if matches!(
                task.status,
                FleetTaskLedgerStatus::Completed
                    | FleetTaskLedgerStatus::Failed
                    | FleetTaskLedgerStatus::Cancelled
            ) {
                lines.push(serde_json::to_string(
                    &FleetLedgerRecord::TaskCompletedOrFailed {
                        run_id: task.entry.run_id.clone(),
                        task_id: task.entry.task_id.clone(),
                        worker_id: task.leased_to.clone().unwrap_or_default(),
                        timestamp: task.completed_at.clone().unwrap_or_default(),
                        status: task.status,
                    },
                )?);
            }
        }
        for event in state.latest_events.values() {
            lines.push(serde_json::to_string(&FleetLedgerRecord::EventAppended {
                event: event.clone(),
            })?);
        }
        for (worker_id, heartbeat) in &state.heartbeats {
            lines.push(serde_json::to_string(&FleetLedgerRecord::Heartbeat {
                worker_id: worker_id.clone(),
                timestamp: heartbeat.timestamp.clone(),
                cpu_percent: heartbeat.cpu_percent,
                memory_mb: heartbeat.memory_mb,
            })?);
        }
        for receipt in state.receipts.values() {
            lines.push(serde_json::to_string(
                &FleetLedgerRecord::ReceiptRecorded {
                    receipt: receipt.clone(),
                },
            )?);
        }
        let mut contents = lines.join("\n");
        if !contents.is_empty() {
            contents.push('\n');
        }
        std::fs::write(&tmp_path, contents)?;
        std::fs::rename(&tmp_path, &self.ledger_path)?;
        Ok(())
    }
}

fn task_key(run_id: &str, task_id: &str) -> String {
    format!("{}:{}", run_id, task_id)
}

fn event_key(worker_id: &str, run_id: &str, task_id: &str) -> String {
    format!("{}:{}:{}", worker_id, run_id, task_id)
}

fn mark_task_terminal(
    state: &mut FleetLedgerState,
    run_id: &FleetRunId,
    task_id: &str,
    worker_id: &str,
    timestamp: &str,
    status: FleetTaskLedgerStatus,
) {
    let key = task_key(&run_id.0, task_id);
    if let Some(task) = state.tasks.get_mut(&key) {
        task.status = status;
        task.leased_to = Some(worker_id.to_string());
        task.completed_at = Some(timestamp.to_string());
    }
}

fn apply_record(state: &mut FleetLedgerState, record: FleetLedgerRecord) {
    match record {
        FleetLedgerRecord::RunCreated { run } => {
            state.runs.insert(run.id.0.clone(), run);
        }
        FleetLedgerRecord::RunStatusChanged {
            run_id,
            status,
            timestamp: _,
        } => {
            state.run_status_overrides.insert(run_id.0, status);
        }
        FleetLedgerRecord::TaskEnqueued { entry } => {
            let key = task_key(&entry.run_id.0, &entry.task_id);
            state.tasks.entry(key).or_insert_with(|| FleetTaskState {
                entry,
                status: FleetTaskLedgerStatus::Enqueued,
                leased_to: None,
                leased_at: None,
                completed_at: None,
            });
        }
        FleetLedgerRecord::TaskLeased {
            run_id,
            task_id,
            worker_id,
            leased_at,
            lease_expires_at: _,
        } => {
            let key = task_key(&run_id.0, &task_id);
            if let Some(task) = state.tasks.get_mut(&key) {
                task.status = FleetTaskLedgerStatus::Leased;
                task.leased_to = Some(worker_id);
                task.leased_at = Some(leased_at);
            }
        }
        FleetLedgerRecord::TaskCompletedOrFailed {
            run_id,
            task_id,
            worker_id,
            timestamp,
            status,
        } => {
            mark_task_terminal(state, &run_id, &task_id, &worker_id, &timestamp, status);
        }
        FleetLedgerRecord::EventAppended { event } => {
            let event_key = event_key(&event.worker_id, &event.run_id.0, &event.task_id);
            if state
                .latest_seq
                .get(&event_key)
                .copied()
                .is_none_or(|seq| event.seq > seq)
            {
                state.latest_seq.insert(event_key.clone(), event.seq);
                state.latest_events.insert(event_key, event.clone());
            }
            // Derive worker status from lifecycle events.
            match &event.payload {
                FleetWorkerEventPayload::Starting | FleetWorkerEventPayload::Running => {
                    state
                        .workers
                        .insert(event.worker_id.clone(), FleetWorkerStatus::Busy);
                }
                FleetWorkerEventPayload::Completed { .. } => {
                    mark_task_terminal(
                        state,
                        &event.run_id,
                        &event.task_id,
                        &event.worker_id,
                        &event.timestamp,
                        FleetTaskLedgerStatus::Completed,
                    );
                    state
                        .workers
                        .insert(event.worker_id.clone(), FleetWorkerStatus::Online);
                }
                FleetWorkerEventPayload::Failed { .. } => {
                    mark_task_terminal(
                        state,
                        &event.run_id,
                        &event.task_id,
                        &event.worker_id,
                        &event.timestamp,
                        FleetTaskLedgerStatus::Failed,
                    );
                    state
                        .workers
                        .insert(event.worker_id.clone(), FleetWorkerStatus::Online);
                }
                FleetWorkerEventPayload::Cancelled { .. } => {
                    mark_task_terminal(
                        state,
                        &event.run_id,
                        &event.task_id,
                        &event.worker_id,
                        &event.timestamp,
                        FleetTaskLedgerStatus::Cancelled,
                    );
                    state
                        .workers
                        .insert(event.worker_id.clone(), FleetWorkerStatus::Online);
                }
                _ => {}
            }
        }
        FleetLedgerRecord::Heartbeat {
            worker_id,
            timestamp,
            cpu_percent,
            memory_mb,
        } => {
            state.heartbeats.insert(
                worker_id.clone(),
                FleetHeartbeatState {
                    timestamp,
                    cpu_percent,
                    memory_mb,
                },
            );
            if state
                .workers
                .get(&worker_id)
                .cloned()
                .unwrap_or(FleetWorkerStatus::Unknown)
                != FleetWorkerStatus::Busy
            {
                state.workers.insert(worker_id, FleetWorkerStatus::Online);
            }
        }
        FleetLedgerRecord::ReceiptRecorded { receipt } => {
            let key = task_key(&receipt.run_id.0, &receipt.task_id);
            state.receipts.insert(key, receipt);
        }
        FleetLedgerRecord::AlertSent { .. } => {
            // Alerts are audit-only for state reconstruction.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_run(id: &str) -> FleetRun {
        FleetRun {
            id: FleetRunId::from(id),
            name: "smoke".to_string(),
            status: FleetRunStatus::Running,
            task_specs: vec![],
            worker_specs: vec![],
            labels: BTreeMap::new(),
            created_at: "2026-06-12T17:00:00Z".to_string(),
            updated_at: None,
            completed_at: None,
        }
    }

    fn sample_entry(run_id: &str, task_id: &str) -> FleetInboxEntry {
        FleetInboxEntry {
            run_id: FleetRunId::from(run_id),
            task_id: task_id.to_string(),
            priority: 0,
            enqueued_at: "2026-06-12T17:00:00Z".to_string(),
            lease_deadline: None,
            attempts: 0,
        }
    }

    #[test]
    fn fleet_ledger_create_and_rebuild_run() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let run = sample_run("run-1");
        ledger.create_run(&run).unwrap();
        ledger
            .update_run_status(&run.id, FleetRunStatus::Completed, "2026-06-12T18:00:00Z")
            .unwrap();

        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.runs.len(), 1);
        assert_eq!(
            state.run_status_overrides["run-1"],
            FleetRunStatus::Completed
        );
    }

    #[test]
    fn fleet_ledger_enqueue_and_claim() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        ledger.create_run(&sample_run("run-1")).unwrap();
        ledger.enqueue(sample_entry("run-1", "task-a")).unwrap();
        ledger.enqueue(sample_entry("run-1", "task-b")).unwrap();

        let claimed = ledger
            .claim_next("worker-1", &[], "2026-06-12T17:01:00Z")
            .unwrap();
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert_eq!(claimed.task_id, "task-a");

        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.tasks.len(), 2);
        assert_eq!(
            state.tasks["run-1:task-a"].status,
            FleetTaskLedgerStatus::Leased
        );
        assert_eq!(
            state.tasks["run-1:task-a"].leased_to.as_deref(),
            Some("worker-1")
        );
        assert_eq!(
            state.tasks["run-1:task-b"].status,
            FleetTaskLedgerStatus::Enqueued
        );
    }

    #[test]
    fn fleet_ledger_survives_restart() {
        let tmp = TempDir::new().unwrap();
        {
            let ledger = FleetLedger::open(tmp.path()).unwrap();
            ledger.create_run(&sample_run("run-1")).unwrap();
            ledger.enqueue(sample_entry("run-1", "task-a")).unwrap();
            ledger
                .lease_task(
                    &FleetRunId::from("run-1"),
                    "task-a",
                    "worker-1",
                    "2026-06-12T17:01:00Z",
                    None,
                )
                .unwrap();
        }
        // Re-open simulates process restart.
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.runs.len(), 1);
        assert_eq!(
            state.tasks["run-1:task-a"].status,
            FleetTaskLedgerStatus::Leased
        );
    }

    #[test]
    fn fleet_ledger_skips_partial_line() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        ledger.create_run(&sample_run("run-1")).unwrap();
        // Append a truncated/invalid JSON line directly; replay should keep
        // the earlier valid record and skip only the partial line.
        let mut file = OpenOptions::new().append(true).open(ledger.path()).unwrap();
        writeln!(file, "{{\"record\":\"run_created\",\"run\":").unwrap();

        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.runs.len(), 1);
        assert!(state.runs.contains_key("run-1"));
    }

    #[test]
    fn fleet_ledger_event_and_heartbeat_reconstruct_worker_status() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        ledger.create_run(&sample_run("run-1")).unwrap();
        ledger.enqueue(sample_entry("run-1", "task-a")).unwrap();
        ledger
            .append_event(FleetWorkerEvent {
                seq: 1,
                run_id: FleetRunId::from("run-1"),
                worker_id: "worker-1".to_string(),
                task_id: "task-a".to_string(),
                timestamp: "2026-06-12T17:01:00Z".to_string(),
                payload: FleetWorkerEventPayload::Running,
                extra: BTreeMap::new(),
            })
            .unwrap();
        ledger
            .heartbeat("worker-1", "2026-06-12T17:02:00Z", Some(12.5), Some(1024))
            .unwrap();

        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.workers["worker-1"], FleetWorkerStatus::Busy);
        assert_eq!(state.heartbeats["worker-1"].cpu_percent, Some(12.5));
    }

    #[test]
    fn fleet_ledger_terminal_events_preserve_failed_and_cancelled_status() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        ledger.create_run(&sample_run("run-1")).unwrap();
        ledger
            .enqueue(sample_entry("run-1", "task-failed"))
            .unwrap();
        ledger
            .enqueue(sample_entry("run-1", "task-cancelled"))
            .unwrap();

        ledger
            .append_event(FleetWorkerEvent {
                seq: 1,
                run_id: FleetRunId::from("run-1"),
                worker_id: "worker-1".to_string(),
                task_id: "task-failed".to_string(),
                timestamp: "2026-06-12T17:03:00Z".to_string(),
                payload: FleetWorkerEventPayload::Failed {
                    reason: "test failed".to_string(),
                    recoverable: false,
                },
                extra: BTreeMap::new(),
            })
            .unwrap();
        ledger
            .append_event(FleetWorkerEvent {
                seq: 2,
                run_id: FleetRunId::from("run-1"),
                worker_id: "worker-2".to_string(),
                task_id: "task-cancelled".to_string(),
                timestamp: "2026-06-12T17:04:00Z".to_string(),
                payload: FleetWorkerEventPayload::Cancelled {
                    cancelled_by: Some("operator".to_string()),
                },
                extra: BTreeMap::new(),
            })
            .unwrap();

        let state = ledger.rebuild_state().unwrap();
        assert_eq!(
            state.tasks["run-1:task-failed"].status,
            FleetTaskLedgerStatus::Failed
        );
        assert_eq!(
            state.tasks["run-1:task-cancelled"].status,
            FleetTaskLedgerStatus::Cancelled
        );

        ledger.compact().unwrap();
        let state = ledger.rebuild_state().unwrap();
        assert_eq!(
            state.tasks["run-1:task-failed"].status,
            FleetTaskLedgerStatus::Failed
        );
        assert_eq!(
            state.tasks["run-1:task-cancelled"].status,
            FleetTaskLedgerStatus::Cancelled
        );
    }

    #[test]
    fn fleet_ledger_compact_preserves_current_state() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        ledger.create_run(&sample_run("run-1")).unwrap();
        ledger.enqueue(sample_entry("run-1", "task-a")).unwrap();
        ledger
            .lease_task(
                &FleetRunId::from("run-1"),
                "task-a",
                "worker-1",
                "2026-06-12T17:01:00Z",
                None,
            )
            .unwrap();
        ledger
            .append_event(FleetWorkerEvent {
                seq: 7,
                run_id: FleetRunId::from("run-1"),
                worker_id: "worker-1".to_string(),
                task_id: "task-a".to_string(),
                timestamp: "2026-06-12T17:01:30Z".to_string(),
                payload: FleetWorkerEventPayload::Running,
                extra: BTreeMap::new(),
            })
            .unwrap();
        ledger
            .heartbeat("worker-1", "2026-06-12T17:02:00Z", Some(12.5), Some(1024))
            .unwrap();
        ledger
            .record_receipt(FleetReceipt {
                run_id: FleetRunId::from("run-1"),
                task_id: "task-a".to_string(),
                worker_id: "worker-1".to_string(),
                completed_at: "2026-06-12T17:03:00Z".to_string(),
                result: FleetTaskResult::Pass,
                artifacts: vec![],
                score: None,
            })
            .unwrap();

        ledger.compact().unwrap();
        let contents = std::fs::read_to_string(ledger.path()).unwrap();
        assert!(contents.lines().count() >= 5, "{contents}");

        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.runs.len(), 1);
        assert_eq!(
            state.tasks["run-1:task-a"].status,
            FleetTaskLedgerStatus::Leased
        );
        assert_eq!(state.workers["worker-1"], FleetWorkerStatus::Busy);
        assert_eq!(state.heartbeats["worker-1"].memory_mb, Some(1024));
        assert!(state.latest_seq.values().any(|seq| *seq == 7));
        assert_eq!(state.receipts["run-1:task-a"].result, FleetTaskResult::Pass);
    }

    #[test]
    fn fleet_ledger_receipt_round_trip() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let receipt = FleetReceipt {
            run_id: FleetRunId::from("run-1"),
            task_id: "task-a".to_string(),
            worker_id: "worker-1".to_string(),
            completed_at: "2026-06-12T17:03:00Z".to_string(),
            result: FleetTaskResult::Pass,
            artifacts: vec![],
            score: None,
        };
        ledger.record_receipt(receipt.clone()).unwrap();
        let state = ledger.rebuild_state().unwrap();
        assert_eq!(state.receipts["run-1:task-a"].result, FleetTaskResult::Pass);
    }
}
