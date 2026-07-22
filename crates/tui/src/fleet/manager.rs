//! Local-first fleet manager loop and operator controls.
//!
//! This module is intentionally ledger-first: the first manager can run in the
//! foreground and coordinate logical local workers while later host adapters
//! add real process and SSH execution behind the same records.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, SecondsFormat, Utc};
use codewhale_protocol::fleet::*;
use serde_json::Value;
use uuid::Uuid;

use super::executor::{
    FleetExecutor, FleetExecutorAttempt, FleetWorkerReportedRoute, FleetWorkerTerminalEvent,
    authority_envelope_for_worker, build_worker_exec_command_with_launch_spec,
};
use super::host::FleetHostErrorKind;
use super::ledger::{FleetLedger, FleetLedgerState, FleetTaskLedgerStatus, FleetTaskState};
use super::scheduler::{FleetScheduler, FleetSchedulerPolicy};
use super::task_spec::{
    FleetTaskSpecDocument, FleetTaskVerificationInput, load_task_spec_document,
    prepare_verification_receipt, validate_task_spec_document, verify_task_result,
};
use super::worker_runtime;
use crate::config::Config;
use crate::tools::subagent::{AgentWorkerSpec, SharedSubAgentManager};

const DEFAULT_STALE_AFTER_SECONDS: u64 = 300;

pub struct FleetManager {
    workspace: PathBuf,
    ledger: FleetLedger,
    stale_after: Duration,
    exec_config: codewhale_config::FleetExecConfig,
    /// `[fleet]` table used to build the agent roster for dispatch
    /// (#fleet-roster cutover (v0.8.67)). Defaults keep built-in + workspace
    /// members resolvable even when the caller has no parsed config.
    fleet_config: codewhale_config::FleetConfigToml,
    /// Optional sub-agent manager for headless worker execution.
    /// When set, fleet workers spawn real sub-agents; when None,
    /// the manager falls back to local simulation.
    sub_agent_manager: Option<SharedSubAgentManager>,
    /// The live session route — the operator's model. Workers whose task and
    /// roster profile pin no model inherit this instead of `"auto"`, so the
    /// model the user picked in `/model` is the model that runs the fleet
    /// (matching the `/fleet roster` operator row). `None` keeps the legacy
    /// `"auto"` fallback for headless callers with no session.
    session_model: Option<String>,
    /// Live provider-route authority used to mint truthful Fleet receipts.
    /// Kept out of Debug because it may contain credentials.
    route_config: Option<Config>,
}

impl std::fmt::Debug for FleetManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FleetManager")
            .field("workspace", &self.workspace)
            .field("ledger", &self.ledger)
            .field("stale_after", &self.stale_after)
            .field("exec_config", &self.exec_config)
            .field(
                "sub_agent_manager",
                &self
                    .sub_agent_manager
                    .as_ref()
                    .map(|_| "SharedSubAgentManager"),
            )
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct FleetRunReport {
    pub run_id: FleetRunId,
    pub task_count: usize,
    pub leased: usize,
    pub queued: usize,
    pub worker_ids: Vec<String>,
}

/// Durable restart transition plus the execution context a caller must drive.
///
/// Restarting is intentionally split from execution so a live foreground
/// manager can observe the transition. Standalone callers must pass this
/// context to [`FleetManager::run_to_completion`] before exiting.
#[derive(Debug, Clone)]
pub struct FleetRestartReport {
    pub run_id: FleetRunId,
    pub max_workers: usize,
    pub inspection: FleetWorkerInspection,
}

#[derive(Debug, Clone, Default)]
pub struct FleetTickReport {
    pub leased: usize,
    pub heartbeats: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FleetExecutorTickReport {
    pub started: usize,
    pub events: usize,
    pub terminals: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FleetStatusSnapshot {
    pub runs: usize,
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub partial: usize,
    pub failed: usize,
    pub restarted: usize,
    pub escalated: usize,
    pub transport_failed: usize,
    pub task_failed: usize,
    pub verifier_failed: usize,
    pub cancelled: usize,
    pub stale: usize,
    pub workers: BTreeMap<String, FleetWorkerStatus>,
}

/// Outcome of resuming a fleet run from durable ledger state after a manager
/// restart. The counts reflect the reconciliation pass; `status` is the
/// post-resume inspectable snapshot.
#[derive(Debug, Clone)]
pub struct FleetResumeReport {
    pub run_id: FleetRunId,
    /// Orphaned in-flight leases detected as stale and reclaimed.
    pub reclaimed_stale: usize,
    /// Stale leases retried within their retry budget.
    pub restarted: usize,
    /// Stale leases that exhausted their retry budget and were failed.
    pub failed: usize,
    /// Escalation alerts emitted for exhausted tasks.
    pub escalated: usize,
    /// Inspectable run status after the resume pass.
    pub status: FleetStatusSnapshot,
}

#[derive(Debug, Clone)]
pub struct FleetWorkerInspection {
    pub worker_id: String,
    pub status: FleetWorkerStatus,
    pub current_run_id: Option<FleetRunId>,
    pub current_task_id: Option<String>,
    pub objective: Option<String>,
    pub role: Option<String>,
    pub host: Option<String>,
    pub latest_heartbeat_at: Option<String>,
    pub latest_event: Option<FleetWorkerEvent>,
    pub artifacts: Vec<FleetArtifactRef>,
    pub receipt_summary: Option<String>,
    pub last_error: Option<String>,
    pub alert_state: Option<String>,
    /// Lightweight projection from the sub-agent worker runtime.
    /// Populated when a sub-agent manager is attached.
    pub runtime_state: Option<FleetWorkerRuntimeProjection>,
}

/// Lightweight TUI projection of a headless sub-agent worker's current state.
///
/// Derived from the sub-agent manager's `AgentWorkerRecord`.
#[derive(Debug, Clone)]
pub struct FleetWorkerRuntimeProjection {
    /// Sub-agent lifecycle status (Queued, Starting, Running, Completed, etc.)
    pub agent_status: String,
    /// Steps taken so far (tool calls + model turns)
    pub steps_taken: u32,
    /// Latest human-readable message from the worker
    pub latest_message: Option<String>,
    /// Error message if the worker failed
    pub error: Option<String>,
    /// Result summary if the worker completed
    pub result_summary: Option<String>,
    /// Whether the worker has a sub-agent session running
    pub has_session: bool,
}

#[derive(Debug, Clone)]
struct FleetExecutorTaskContext {
    entry: FleetInboxEntry,
    task_spec: FleetTaskSpec,
    worker_id: String,
}

impl FleetManager {
    pub fn open(workspace: impl AsRef<Path>) -> Result<Self> {
        let workspace = workspace.as_ref().to_path_buf();
        let ledger = FleetLedger::open(&workspace)?;
        Ok(Self {
            workspace,
            ledger,
            stale_after: Duration::from_secs(DEFAULT_STALE_AFTER_SECONDS),
            exec_config: codewhale_config::FleetExecConfig::default(),
            fleet_config: codewhale_config::FleetConfigToml::default(),
            sub_agent_manager: None,
            session_model: None,
            route_config: None,
        })
    }

    /// Adopt the active session route as the run-level model: whatever the
    /// user selected in `/model` becomes the operator, and workers without a
    /// task/profile model pin inherit it. Empty and `"auto"` values are
    /// ignored so the resolver default keeps applying.
    pub fn with_session_model(mut self, model: impl Into<String>) -> Self {
        let model = model.into();
        let trimmed = model.trim();
        if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("auto") {
            self.session_model = Some(trimmed.to_string());
        }
        self
    }

    pub fn with_route_config(mut self, config: Config) -> Self {
        self.route_config = Some(config);
        self
    }

    /// The run-level model handed to worker-spec resolution: the session
    /// model when one was adopted, else the legacy `"auto"` sentinel.
    fn run_model(&self) -> &str {
        self.session_model.as_deref().unwrap_or("auto")
    }

    pub fn with_stale_after(mut self, stale_after: Duration) -> Self {
        self.stale_after = stale_after;
        self
    }

    /// Apply fleet headless-worker execution policy from config.
    pub fn with_exec_config(mut self, exec_config: codewhale_config::FleetExecConfig) -> Self {
        self.exec_config = exec_config;
        self
    }

    /// Apply the parsed `[fleet]` table so `[fleet.profiles]` members join
    /// the dispatch roster (#fleet-roster cutover (v0.8.67)).
    pub fn with_fleet_config(mut self, fleet_config: codewhale_config::FleetConfigToml) -> Self {
        self.fleet_config = fleet_config;
        self
    }

    /// Merged agent roster (built-ins + `[fleet.profiles]` + workspace files)
    /// used everywhere a task references an `agent_profile` id.
    fn agent_roster(&self) -> crate::fleet::roster::FleetRoster {
        crate::fleet::roster::FleetRoster::load(&self.fleet_config, &self.workspace)
    }

    /// Attach a sub-agent manager so fleet workers can spawn real headless agents.
    pub fn with_sub_agent_manager(mut self, mgr: SharedSubAgentManager) -> Self {
        self.sub_agent_manager = Some(mgr);
        self
    }

    pub fn ledger_path(&self) -> &Path {
        self.ledger.path()
    }

    fn manager_lock_path(&self, run_id: &FleetRunId) -> PathBuf {
        self.workspace
            .join(".codewhale")
            .join("fleet")
            .join(format!("manager-{}.lock", safe_path_segment(&run_id.0)))
    }

    pub fn rebuild_state(&self) -> Result<FleetLedgerState> {
        self.ledger.rebuild_state()
    }

    pub fn load_task_spec(path: &Path) -> Result<FleetTaskSpecDocument> {
        load_task_spec_document(path)
    }

    pub fn create_run_from_task_spec_path(
        &self,
        path: &Path,
        max_workers: usize,
    ) -> Result<FleetRunReport> {
        let doc = Self::load_task_spec(path)?;
        self.create_run(doc, max_workers)
    }

    pub fn create_run(
        &self,
        mut doc: FleetTaskSpecDocument,
        max_workers: usize,
    ) -> Result<FleetRunReport> {
        validate_task_spec_document(&doc)?;
        let roster = self.agent_roster();
        worker_runtime::validate_task_agent_profiles(&doc.tasks, roster.members())?;
        let max_workers = max_workers.clamp(1, 128);
        let run_id = FleetRunId::from(format!(
            "fleet-{}",
            &Uuid::new_v4().simple().to_string()[..8]
        ));
        let now = timestamp();
        if doc.workers.is_empty() {
            doc.workers = default_local_workers(&run_id, max_workers);
        }
        let run = FleetRun {
            id: run_id.clone(),
            name: doc.name.unwrap_or_else(|| run_id.0.clone()),
            status: FleetRunStatus::Queued,
            max_workers: Some(max_workers),
            task_specs: doc.tasks.clone(),
            worker_specs: doc.workers.clone(),
            labels: doc.labels,
            security_policy: doc.security_policy.clone(),
            created_at: now.clone(),
            updated_at: Some(now.clone()),
            completed_at: None,
        };
        self.ledger.create_run(&run)?;
        for task in &run.task_specs {
            self.ledger.enqueue(FleetInboxEntry {
                run_id: run.id.clone(),
                task_id: task.id.clone(),
                priority: task_priority(task),
                enqueued_at: now.clone(),
                lease_deadline: None,
                attempts: 0,
            })?;
        }
        let initial_status = if run.task_specs.is_empty() {
            FleetRunStatus::Completed
        } else {
            FleetRunStatus::Running
        };
        self.ledger
            .update_run_status(&run.id, initial_status, &timestamp())?;
        let tick = self.schedule_run(&run.id, max_workers)?;
        self.refresh_run_status(&run.id)?;
        let state = self.ledger.rebuild_state()?;
        let snapshot = self.status_from_state(Some(&run.id), &state);
        Ok(FleetRunReport {
            run_id: run.id,
            task_count: run.task_specs.len(),
            leased: tick.leased,
            queued: snapshot.queued,
            worker_ids: run.worker_specs.iter().map(|w| w.id.clone()).collect(),
        })
    }

    pub fn schedule_run(&self, run_id: &FleetRunId, max_workers: usize) -> Result<FleetTickReport> {
        self.schedule_run_excluding(run_id, max_workers, &BTreeSet::new())
    }

    fn schedule_run_excluding(
        &self,
        run_id: &FleetRunId,
        max_workers: usize,
        unavailable_workers: &BTreeSet<String>,
    ) -> Result<FleetTickReport> {
        self.reconcile_coordination_worker_statuses()?;
        let max_workers = max_workers.clamp(1, 128);
        let mut report = FleetTickReport::default();
        let state = self.ledger.rebuild_state()?;
        let run = state
            .runs
            .get(&run_id.0)
            .cloned()
            .ok_or_else(|| anyhow!("fleet run {} does not exist", run_id.0))?;
        let worker_ids = worker_ids_for_run(&run, max_workers);

        for task in active_tasks_for_run(&state, run_id) {
            if let Some(worker_id) = task.leased_to.as_deref()
                && worker_ids.iter().any(|id| id == worker_id)
            {
                self.ledger.heartbeat(worker_id, &timestamp(), None, None)?;
                report.heartbeats += 1;
            }
        }

        loop {
            let state = self.ledger.rebuild_state()?;
            let active_workers = active_workers_for_run(&state, run_id);
            if active_workers.len() >= max_workers {
                break;
            }
            let Some(worker_id) = worker_ids
                .iter()
                .find(|id| {
                    !active_workers.contains(*id) && !unavailable_workers.contains(id.as_str())
                })
                .cloned()
            else {
                break;
            };
            let Some((entry, task_spec)) = next_enqueued_task_for_run(&state, run_id) else {
                break;
            };
            if self.start_worker_task(&worker_id, &entry, &task_spec, Some(max_workers))? {
                report.leased += 1;
            }
        }

        self.refresh_run_status(run_id)?;
        Ok(report)
    }

    fn reconcile_coordination_worker_statuses(&self) -> Result<()> {
        let Some(manager) = self.sub_agent_manager.as_ref() else {
            return Ok(());
        };
        let Ok(mut guard) = manager.try_write() else {
            // The next scheduler tick retries before leasing more work.
            return Ok(());
        };
        let state = self.ledger.rebuild_state()?;
        for record in guard.list_worker_records() {
            let current = state
                .tasks
                .values()
                .filter(|task| task.entry.run_id.0 == record.spec.run_id)
                .filter(|task| task.leased_to.as_deref() == Some(record.spec.worker_id.as_str()))
                .max_by_key(|task| task.lifecycle_seq);
            let Some(current) = current else {
                continue;
            };
            let (status, status_label) = match current.status {
                FleetTaskLedgerStatus::Enqueued => {
                    (crate::tools::subagent::AgentWorkerStatus::Queued, "queued")
                }
                FleetTaskLedgerStatus::Leased => (
                    crate::tools::subagent::AgentWorkerStatus::Running,
                    "running",
                ),
                FleetTaskLedgerStatus::Completed => (
                    crate::tools::subagent::AgentWorkerStatus::Completed,
                    "completed",
                ),
                FleetTaskLedgerStatus::Failed => {
                    (crate::tools::subagent::AgentWorkerStatus::Failed, "failed")
                }
                FleetTaskLedgerStatus::Cancelled => (
                    crate::tools::subagent::AgentWorkerStatus::Cancelled,
                    "cancelled",
                ),
            };
            guard.project_external_worker_status(
                &record.spec.worker_id,
                status,
                Some(format!(
                    "Fleet task {} is {}",
                    current.entry.task_id, status_label
                )),
            );
        }
        Ok(())
    }

    pub fn status(&self) -> Result<FleetStatusSnapshot> {
        let state = self.ledger.rebuild_state()?;
        Ok(self.status_from_state(None, &state))
    }

    pub fn run_status(&self, run_id: &FleetRunId) -> Result<FleetStatusSnapshot> {
        let state = self.ledger.rebuild_state()?;
        Ok(self.status_from_state(Some(run_id), &state))
    }

    pub fn run_has_open_work(&self, run_id: &FleetRunId) -> Result<bool> {
        let status = self.run_status(run_id)?;
        Ok(status.queued + status.running + status.stale > 0)
    }

    /// Resume a run from durable ledger state after a manager restart.
    ///
    /// A crashed or detached manager can leave in-flight tasks `Leased` to
    /// workers whose processes are gone. Resume rebuilds run state from the
    /// ledger, reconciles those orphaned/stale leases through the shared
    /// scheduler recovery semantics (retry within budget, else fail and
    /// escalate), records every decision durably, and returns an inspectable
    /// status. It launches no new work and does not re-process tasks that
    /// already reached a terminal state, so it is safe to call repeatedly.
    pub fn resume_run(&self, run_id: &FleetRunId) -> Result<FleetResumeReport> {
        self.resume_run_at(run_id, Utc::now())
    }

    /// Resume reconciliation at an explicit instant. This is the deterministic
    /// seam behind `resume_run`'s wall clock: stale detection compares the
    /// last heartbeat against `now`.
    pub(crate) fn resume_run_at(
        &self,
        run_id: &FleetRunId,
        now: DateTime<Utc>,
    ) -> Result<FleetResumeReport> {
        // Reuse the shared scheduler recovery engine over the same ledger so
        // resume and steady-state supervision converge on one store and one
        // retry/escalation policy. The manager's `stale_after` becomes the
        // scheduler's heartbeat timeout so both surfaces agree on staleness.
        let policy = FleetSchedulerPolicy {
            heartbeat_timeout: self.stale_after,
            ..FleetSchedulerPolicy::default()
        };
        let mut scheduler = FleetScheduler::open(&self.workspace, policy)?;
        scheduler.set_now(now);
        let report = scheduler.resume_run(run_id)?;
        let status = self.run_status(run_id)?;
        Ok(FleetResumeReport {
            run_id: run_id.clone(),
            reclaimed_stale: report.marked_stale,
            restarted: report.restarted,
            failed: report.failed,
            escalated: report.alerts,
            status,
        })
    }

    pub async fn run_to_completion(
        &self,
        run_id: &FleetRunId,
        max_workers: usize,
        executor: &mut FleetExecutor,
        codewhale_binary: &str,
        model: Option<&str>,
        tick_interval: Duration,
    ) -> Result<FleetStatusSnapshot> {
        let max_workers = max_workers.clamp(1, 128);
        let manager_lock_path = self.manager_lock_path(run_id);
        if let Some(parent) = manager_lock_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating fleet manager lock dir {}", parent.display()))?;
        }
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&manager_lock_path)
            .with_context(|| {
                format!("opening fleet manager lock {}", manager_lock_path.display())
            })?;
        let mut manager_lock = fd_lock::RwLock::new(lock_file);
        let standby_interval = tick_interval
            .min(Duration::from_millis(100))
            .max(Duration::from_millis(10));
        let mut observed_owner = false;
        let _manager_guard = loop {
            match manager_lock.try_write() {
                Ok(guard) => {
                    if observed_owner {
                        if self.run_has_open_work(run_id)? {
                            bail!(
                                "fleet manager for run {} exited with open work; wait for stale reconciliation before resuming",
                                run_id.0
                            );
                        }
                        return self.run_status(run_id);
                    }
                    break guard;
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    // Another process owns this run. Wait for it to finish,
                    // but never treat lock release as permission to relaunch
                    // its unchanged leased attempts: an orphan child may still
                    // be alive after a crash. Stale reconciliation owns that
                    // recovery/generation transition.
                    observed_owner = true;
                    if !self.run_has_open_work(run_id)? {
                        return self.run_status(run_id);
                    }
                    tokio::time::sleep(standby_interval).await;
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!(
                            "locking fleet manager ownership {}",
                            manager_lock_path.display()
                        )
                    });
                }
            }
        };
        loop {
            // A terminal ledger update can race the foreground host process.
            // Do not lease new work onto a logical worker until its executor
            // handle has been observed and forgotten below.
            let unavailable_workers = executor.worker_ids().into_iter().collect();
            self.schedule_run_excluding(run_id, max_workers, &unavailable_workers)?;
            self.drive_executor_tick(run_id, executor, codewhale_binary, model)?;
            self.refresh_run_status(run_id)?;
            // A separate `fleet interrupt` process can make the ledger
            // terminal while this manager still owns a live host child. Keep
            // driving until the executor has observed that cancellation and
            // stopped every tracked process.
            if !self.run_has_open_work(run_id)? && executor.worker_ids().is_empty() {
                return self.run_status(run_id);
            }
            tokio::time::sleep(tick_interval).await;
        }
    }

    pub fn drive_executor_tick(
        &self,
        run_id: &FleetRunId,
        executor: &mut FleetExecutor,
        codewhale_binary: &str,
        model: Option<&str>,
    ) -> Result<FleetExecutorTickReport> {
        let mut report = FleetExecutorTickReport::default();
        report.started += self.start_leased_workers(run_id, executor, codewhale_binary, model)?;

        for worker_id in executor.worker_ids() {
            let tracked_attempt = executor.tracked_attempt(&worker_id);
            if let Some(attempt) = tracked_attempt.as_ref()
                && self
                    .executor_task_context_for_attempt(&worker_id, attempt)?
                    .is_none()
            {
                // The ledger advanced to another attempt (restart), or made
                // this attempt terminal (cancel/stop), while this process was
                // still alive. The executor owns the host handle, so fence and
                // reap the old process without publishing any event against the
                // replacement generation.
                executor.stop_worker(&worker_id)?;
                executor.forget_worker(&worker_id);
                report.terminals += 1;
                continue;
            }
            if tracked_attempt.is_none()
                && let Some(_task) = self.cancelled_executor_task_context(&worker_id)?
            {
                // Cancellation is ledgered by an out-of-process control
                // command. Only this executor owns the host process handle,
                // so it must enforce the terminal state before returning from
                // the foreground manager loop. Do not ingest output produced
                // after cancellation; publish one final authoritative event
                // after the process is actually stopped instead.
                executor.stop_worker(&worker_id)?;
                executor.forget_worker(&worker_id);
                report.terminals += 1;
                continue;
            }

            for payload in executor.drain_events(&worker_id) {
                // The subprocess exit is the task-completion authority. Stream
                // `done` / `error` lines are useful progress signals, but
                // appending them as terminal ledger events before the process
                // exits would free the logical worker too early.
                if is_terminal_payload(&payload) {
                    continue;
                }
                let task = if let Some(attempt) = tracked_attempt.as_ref() {
                    self.executor_task_context_for_attempt(&worker_id, attempt)?
                } else {
                    self.executor_task_context(&worker_id)?
                };
                let Some(task) = task else {
                    continue;
                };
                if self
                    .ledger
                    .append_event_if_leased(
                        &task.entry.run_id,
                        &worker_id,
                        &task.entry.task_id,
                        task.entry.attempts,
                        &timestamp(),
                        payload,
                    )?
                    .is_none()
                {
                    continue;
                }
                self.ledger
                    .heartbeat(&worker_id, &timestamp(), None, None)?;
                report.events += 1;
            }

            if let Some(terminal) = executor.poll_terminal_with_status(&worker_id) {
                let task = if let Some(attempt) = tracked_attempt.as_ref() {
                    self.executor_task_context_for_attempt(&worker_id, attempt)?
                } else {
                    self.executor_task_context(&worker_id)?
                };
                let Some(task) = task else {
                    executor.forget_worker(&worker_id);
                    continue;
                };
                if self.record_task_outcome(&task, terminal)? {
                    report.terminals += 1;
                }
                executor.forget_worker(&worker_id);
            }
        }

        self.refresh_run_status(run_id)?;
        Ok(report)
    }

    pub fn inspect_worker(&self, worker_id: &str) -> Result<FleetWorkerInspection> {
        let state = self.ledger.rebuild_state()?;
        let latest_event = latest_event_for_worker(&state, worker_id).cloned();
        let current = active_task_for_worker(&state, worker_id)
            .or_else(|| latest_task_for_worker(&state, worker_id));
        let current_run_id = current.as_ref().map(|task| task.entry.run_id.clone());
        let current_task_id = current.as_ref().map(|task| task.entry.task_id.clone());
        let (objective, role) = current
            .as_ref()
            .and_then(|task| task_spec_for_state(&state, task))
            .map(|task_spec| {
                (
                    task_spec.objective.or(task_spec.description),
                    task_spec.worker.and_then(|worker| worker.role),
                )
            })
            .unwrap_or((None, None));
        let host = current_run_id
            .as_ref()
            .and_then(|run_id| worker_host_for_run(&state, run_id, worker_id));
        let artifacts = state
            .artifact_events
            .values()
            .filter(|event| event.worker_id == worker_id)
            .filter_map(|event| match &event.payload {
                FleetWorkerEventPayload::Artifact(artifact) => Some(artifact.clone()),
                _ => None,
            })
            .chain(
                state
                    .receipts
                    .values()
                    .filter(|receipt| receipt.worker_id == worker_id)
                    .flat_map(|receipt| receipt.artifacts.clone()),
            )
            .collect();
        let receipt_summary = latest_receipt_for_worker(&state, worker_id).map(receipt_summary);
        let last_error = latest_error_for_worker(&state, worker_id);
        let status = state
            .workers
            .get(worker_id)
            .cloned()
            .unwrap_or(FleetWorkerStatus::Unknown);
        let latest_heartbeat_at = state
            .heartbeats
            .get(worker_id)
            .map(|heartbeat| heartbeat.timestamp.clone());
        let alert_state = latest_alert_for_worker(&state, worker_id);

        // Enrich only a live lease with its in-memory worker projection. A
        // terminal durable task always wins over a lagging runtime record.
        let runtime_state = current
            .as_ref()
            .filter(|task| task.status == FleetTaskLedgerStatus::Leased)
            .and(self.sub_agent_manager.as_ref())
            .and_then(|mgr| {
                mgr.try_read()
                    .ok()
                    .and_then(|guard| guard.get_worker_record(worker_id))
                    .map(|record| FleetWorkerRuntimeProjection {
                        agent_status: format!("{:?}", record.status).to_lowercase(),
                        steps_taken: record.steps_taken,
                        latest_message: record.latest_message,
                        error: record.error,
                        result_summary: record.result_summary,
                        has_session: !matches!(
                            record.status,
                            crate::tools::subagent::AgentWorkerStatus::Completed
                                | crate::tools::subagent::AgentWorkerStatus::Failed
                                | crate::tools::subagent::AgentWorkerStatus::Cancelled
                        ),
                    })
            });

        Ok(FleetWorkerInspection {
            worker_id: worker_id.to_string(),
            status,
            current_run_id,
            current_task_id,
            objective,
            role,
            host,
            latest_heartbeat_at,
            latest_event,
            artifacts,
            receipt_summary,
            last_error,
            alert_state,
            runtime_state,
        })
    }

    pub fn interrupt_worker(&self, worker_id: &str) -> Result<FleetWorkerInspection> {
        let state = self.ledger.rebuild_state()?;
        let Some(task) = active_task_for_worker(&state, worker_id) else {
            bail!("worker {worker_id} has no running fleet task");
        };
        let cancelled = self.ledger.cancel_task_if_active(
            &task.entry.run_id,
            &task.entry.task_id,
            Some(worker_id),
            &timestamp(),
            Some("operator"),
            Some("operator"),
        )?;
        if !cancelled {
            bail!("worker {worker_id} no longer has that running fleet task");
        }
        self.refresh_run_status(&task.entry.run_id)?;
        self.inspect_worker(worker_id)
    }

    pub fn restart_worker(&self, worker_id: &str) -> Result<FleetRestartReport> {
        let state = self.ledger.rebuild_state()?;
        let Some(task) = active_task_for_worker(&state, worker_id)
            .or_else(|| latest_task_for_worker(&state, worker_id))
        else {
            bail!("worker {worker_id} has no fleet task to restart");
        };
        let run = state
            .runs
            .get(&task.entry.run_id.0)
            .ok_or_else(|| anyhow!("fleet run {} does not exist", task.entry.run_id.0))?;
        let max_workers = run
            .max_workers
            .unwrap_or_else(|| run.worker_specs.len().max(1))
            .clamp(1, 128);
        let now = timestamp();
        let latest_seq = state
            .latest_seq
            .get(&event_key(
                worker_id,
                &task.entry.run_id.0,
                &task.entry.task_id,
            ))
            .copied()
            .unwrap_or(0);
        let heartbeat_at = state
            .heartbeats
            .get(worker_id)
            .map(|heartbeat| heartbeat.timestamp.as_str());
        if !self.ledger.restart_task_if_unchanged(
            &task.entry.run_id,
            &task.entry.task_id,
            worker_id,
            task.status,
            task.entry.attempts,
            latest_seq,
            heartbeat_at,
            &now,
            None,
            task.entry.attempts,
        )? {
            bail!("worker {worker_id} task changed before it could be restarted");
        }
        self.ledger
            .update_run_status(&task.entry.run_id, FleetRunStatus::Running, &timestamp())?;
        Ok(FleetRestartReport {
            run_id: task.entry.run_id.clone(),
            max_workers,
            inspection: self.inspect_worker(worker_id)?,
        })
    }

    pub fn stop_all(&self) -> Result<usize> {
        let state = self.ledger.rebuild_state()?;
        let now = timestamp();
        let mut affected_runs = BTreeSet::new();
        let mut stopped = 0usize;
        for task in state.tasks.values() {
            if !matches!(
                task.status,
                FleetTaskLedgerStatus::Enqueued | FleetTaskLedgerStatus::Leased
            ) {
                continue;
            }
            if !self.ledger.cancel_task_if_active(
                &task.entry.run_id,
                &task.entry.task_id,
                None,
                &now,
                Some("stop_all"),
                Some("operator"),
            )? {
                continue;
            }
            affected_runs.insert(task.entry.run_id.0.clone());
            stopped += 1;
        }
        for run_id in affected_runs {
            self.ledger.update_run_status(
                &FleetRunId::from(run_id),
                FleetRunStatus::Cancelled,
                &timestamp(),
            )?;
        }
        Ok(stopped)
    }

    pub fn stop_run(&self, run_id: &FleetRunId) -> Result<usize> {
        let state = self.ledger.rebuild_state()?;
        if !state.runs.contains_key(&run_id.0) {
            bail!("fleet run {} does not exist", run_id.0);
        }
        let now = timestamp();
        let mut stopped = 0usize;
        for task in state
            .tasks
            .values()
            .filter(|task| task.entry.run_id == *run_id)
        {
            if !matches!(
                task.status,
                FleetTaskLedgerStatus::Enqueued | FleetTaskLedgerStatus::Leased
            ) {
                continue;
            }
            if !self.ledger.cancel_task_if_active(
                &task.entry.run_id,
                &task.entry.task_id,
                None,
                &now,
                Some("stop_run"),
                Some("operator"),
            )? {
                continue;
            }
            stopped += 1;
        }
        self.ledger
            .update_run_status(run_id, FleetRunStatus::Cancelled, &timestamp())?;
        Ok(stopped)
    }

    fn start_worker_task(
        &self,
        worker_id: &str,
        entry: &FleetInboxEntry,
        task_spec: &FleetTaskSpec,
        max_active_for_run: Option<usize>,
    ) -> Result<bool> {
        let run = self
            .ledger
            .rebuild_state()?
            .runs
            .get(&entry.run_id.0)
            .cloned()
            .ok_or_else(|| anyhow!("fleet run {} does not exist", entry.run_id.0))?;
        let worker_spec = run
            .worker_specs
            .iter()
            .find(|worker| worker.id == worker_id)
            .cloned()
            .unwrap_or_else(|| default_local_worker(worker_id));
        let worker_workspace = resolve_task_cwd(&self.workspace, task_spec)?;
        validate_task_cwd_for_host(&self.workspace, &worker_spec.host, &worker_workspace)?;
        let roster = self.agent_roster();
        let sub_agent_worker = bind_fleet_launch_attempt(
            worker_runtime::apply_exec_hardening(
                worker_runtime::fleet_task_to_worker_spec_with_profiles(
                    worker_id,
                    &entry.run_id.0,
                    task_spec,
                    &worker_spec,
                    self.run_model(),
                    &worker_workspace,
                    roster.members(),
                    None,
                )?,
                &self.exec_config,
            ),
            entry.attempts.saturating_add(1),
        );
        authority_envelope_for_worker(&sub_agent_worker, task_spec)?;
        let log_artifact = self.write_log_artifact(&entry.run_id, worker_id, task_spec)?;
        // Hold the coordination manager from pure preflight through the
        // ledger's pre-commit projection callback and any append compensation. This keeps a concurrent
        // agent/Fleet registration from invalidating the overlap decision in
        // between, while a busy manager simply leaves the task queued for the
        // next scheduler tick.
        let mut coordination_guard = match &self.sub_agent_manager {
            Some(manager) => {
                let Ok(mut guard) = manager.try_write() else {
                    return Ok(false);
                };
                guard
                    .preflight_worker_coordination(&sub_agent_worker)
                    .map_err(anyhow::Error::msg)?;
                Some(guard)
            }
            None => None,
        };
        let registration_snapshot = coordination_guard
            .as_ref()
            .map(|guard| guard.coordination_registration_snapshot());
        let mut registration_succeeded = false;
        let now = timestamp();
        let start_result = self.ledger.start_task_if_enqueued(
            &entry.run_id,
            &entry.task_id,
            worker_id,
            &now,
            None,
            max_active_for_run,
            vec![
                FleetWorkerEventPayload::Leased {
                    lease_expires_at: None,
                },
                FleetWorkerEventPayload::Starting,
                FleetWorkerEventPayload::Artifact(log_artifact),
                FleetWorkerEventPayload::Running,
            ],
            || {
                // Registration shares the durable transition lock, so a
                // cancellation cannot win between claim and projection setup.
                if let Some(guard) = coordination_guard.as_mut() {
                    guard
                        .register_worker_with_coordination(sub_agent_worker)
                        .map_err(anyhow::Error::msg)?;
                    registration_succeeded = true;
                }
                Ok(())
            },
        );
        let started = match start_result {
            Ok(started) => started,
            Err(start_error) => {
                if registration_succeeded
                    && let (Some(guard), Some(snapshot)) =
                        (coordination_guard.as_mut(), registration_snapshot)
                    && let Err(rollback_error) =
                        guard.restore_coordination_registration_snapshot(snapshot)
                {
                    return Err(anyhow!("{start_error:#}; additionally {rollback_error}"));
                }
                return Err(start_error);
            }
        };
        if !started {
            return Ok(false);
        }

        Ok(true)
    }

    fn start_leased_workers(
        &self,
        run_id: &FleetRunId,
        executor: &mut FleetExecutor,
        codewhale_binary: &str,
        model: Option<&str>,
    ) -> Result<usize> {
        let state = self.ledger.rebuild_state()?;
        let run = state
            .runs
            .get(&run_id.0)
            .cloned()
            .ok_or_else(|| anyhow!("fleet run {} does not exist", run_id.0))?;
        let roster = self.agent_roster();
        let mut started = 0usize;
        for task in active_tasks_for_run(&state, run_id) {
            let Some(worker_id) = task.leased_to.as_deref() else {
                continue;
            };
            if executor.is_tracking(worker_id) {
                continue;
            }
            let Some(task_spec) = run
                .task_specs
                .iter()
                .find(|spec| spec.id == task.entry.task_id)
                .cloned()
            else {
                continue;
            };
            let worker_spec = run
                .worker_specs
                .iter()
                .find(|worker| worker.id == worker_id)
                .cloned()
                .unwrap_or_else(|| default_local_worker(worker_id));
            let coordination_record = if let Some(manager) = self.sub_agent_manager.as_ref() {
                let Ok(guard) = manager.try_read() else {
                    continue;
                };
                Some(guard.get_worker_record(worker_id))
            } else {
                None
            };
            let preparation = (|| -> Result<_> {
                let cwd = resolve_task_cwd(&self.workspace, &task_spec)?;
                validate_task_cwd_for_host(&self.workspace, &worker_spec.host, &cwd)?;
                let expected_launch_spec = bind_fleet_launch_attempt(
                    worker_runtime::apply_exec_hardening(
                        worker_runtime::fleet_task_to_worker_spec_with_profiles(
                            worker_id,
                            &run_id.0,
                            &task_spec,
                            &worker_spec,
                            self.run_model(),
                            &cwd,
                            roster.members(),
                            None,
                        )?,
                        &self.exec_config,
                    ),
                    task.entry.attempts,
                );
                let launch_spec = match coordination_record {
                    Some(Some(record)) => {
                        validate_registered_launch_spec(&record.spec, &expected_launch_spec)?;
                        record.spec
                    }
                    Some(None) => {
                        bail!("Fleet worker {worker_id} has no coordination-registered launch spec")
                    }
                    None => expected_launch_spec,
                };
                let command = build_worker_exec_command_with_launch_spec(
                    codewhale_binary,
                    &task_spec,
                    &launch_spec,
                    &self.exec_config,
                    model,
                    roster.members(),
                )?;
                Ok((cwd, command))
            })();
            let attempt = FleetExecutorAttempt {
                run_id: task.entry.run_id.clone(),
                task_id: task.entry.task_id.clone(),
                attempt: task.entry.attempts,
            };
            let (cwd, command) = match preparation {
                Ok(prepared) => prepared,
                Err(err) => {
                    let task = FleetExecutorTaskContext {
                        entry: task.entry.clone(),
                        task_spec,
                        worker_id: worker_id.to_string(),
                    };
                    let terminal = FleetWorkerTerminalEvent {
                        payload: FleetWorkerEventPayload::Failed {
                            reason: format!("worker launch preparation failed: {err:#}"),
                            recoverable: false,
                        },
                        exit_code: None,
                        tail_payloads: Vec::new(),
                        reported_route: None,
                        requires_reported_route: false,
                    };
                    let _ = self.record_task_outcome(&task, terminal)?;
                    continue;
                }
            };
            match executor.start_worker_attempt_on_host(
                worker_id,
                &worker_spec.host,
                command,
                Some(cwd),
                attempt,
            ) {
                Ok(handle) => {
                    let artifact = self.host_log_artifact(&handle.log_path);
                    if self
                        .ledger
                        .append_event_if_leased(
                            run_id,
                            worker_id,
                            &task.entry.task_id,
                            task.entry.attempts,
                            &timestamp(),
                            FleetWorkerEventPayload::Artifact(artifact),
                        )?
                        .is_none()
                    {
                        executor.stop_worker(worker_id)?;
                        executor.forget_worker(worker_id);
                        continue;
                    }
                    started += 1;
                }
                Err(err) => {
                    let recoverable = matches!(err.kind, FleetHostErrorKind::Retryable);
                    let task = FleetExecutorTaskContext {
                        entry: task.entry.clone(),
                        task_spec,
                        worker_id: worker_id.to_string(),
                    };
                    let terminal = FleetWorkerTerminalEvent {
                        payload: FleetWorkerEventPayload::Failed {
                            reason: err.message,
                            recoverable,
                        },
                        exit_code: None,
                        tail_payloads: Vec::new(),
                        reported_route: None,
                        requires_reported_route: false,
                    };
                    let _ = self.record_task_outcome(&task, terminal)?;
                }
            }
        }
        Ok(started)
    }

    fn executor_task_context(&self, worker_id: &str) -> Result<Option<FleetExecutorTaskContext>> {
        let state = self.ledger.rebuild_state()?;
        let Some(task) = active_task_for_worker(&state, worker_id)
            .or_else(|| latest_task_for_worker(&state, worker_id))
        else {
            return Ok(None);
        };
        let Some(run) = state.runs.get(&task.entry.run_id.0) else {
            return Ok(None);
        };
        let Some(task_spec) = run
            .task_specs
            .iter()
            .find(|spec| spec.id == task.entry.task_id)
            .cloned()
        else {
            return Ok(None);
        };
        Ok(Some(FleetExecutorTaskContext {
            entry: task.entry.clone(),
            task_spec,
            worker_id: worker_id.to_string(),
        }))
    }

    fn executor_task_context_for_attempt(
        &self,
        worker_id: &str,
        attempt: &FleetExecutorAttempt,
    ) -> Result<Option<FleetExecutorTaskContext>> {
        let state = self.ledger.rebuild_state()?;
        let key = task_key(&attempt.run_id.0, &attempt.task_id);
        let Some(task) = state.tasks.get(&key) else {
            return Ok(None);
        };
        if task.status != FleetTaskLedgerStatus::Leased
            || task.leased_to.as_deref() != Some(worker_id)
            || task.entry.attempts != attempt.attempt
        {
            return Ok(None);
        }
        let Some(run) = state.runs.get(&attempt.run_id.0) else {
            return Ok(None);
        };
        let Some(task_spec) = run
            .task_specs
            .iter()
            .find(|spec| spec.id == attempt.task_id)
            .cloned()
        else {
            return Ok(None);
        };
        Ok(Some(FleetExecutorTaskContext {
            entry: task.entry.clone(),
            task_spec,
            worker_id: worker_id.to_string(),
        }))
    }

    fn cancelled_executor_task_context(
        &self,
        worker_id: &str,
    ) -> Result<Option<FleetExecutorTaskContext>> {
        let state = self.ledger.rebuild_state()?;
        let Some(task) = latest_task_for_worker(&state, worker_id) else {
            return Ok(None);
        };
        if task.status != FleetTaskLedgerStatus::Cancelled {
            return Ok(None);
        }
        let Some(run) = state.runs.get(&task.entry.run_id.0) else {
            return Ok(None);
        };
        let Some(task_spec) = run
            .task_specs
            .iter()
            .find(|spec| spec.id == task.entry.task_id)
            .cloned()
        else {
            return Ok(None);
        };
        Ok(Some(FleetExecutorTaskContext {
            entry: task.entry.clone(),
            task_spec,
            worker_id: worker_id.to_string(),
        }))
    }

    fn record_task_outcome(
        &self,
        task: &FleetExecutorTaskContext,
        terminal: FleetWorkerTerminalEvent,
    ) -> Result<bool> {
        let state = self.ledger.rebuild_state()?;
        let key = task_key(&task.entry.run_id.0, &task.entry.task_id);
        let Some(current) = state.tasks.get(&key) else {
            return Ok(false);
        };
        if current.status != FleetTaskLedgerStatus::Leased
            || current.leased_to.as_deref() != Some(task.worker_id.as_str())
            || current.entry.attempts != task.entry.attempts
        {
            return Ok(false);
        }

        let FleetWorkerTerminalEvent {
            payload,
            exit_code,
            tail_payloads,
            reported_route,
            requires_reported_route,
        } = terminal;
        let (receipt_result, failure_kind, exit_code) = task_receipt_outcome(&payload, exit_code);
        let terminal_completed = matches!(&payload, FleetWorkerEventPayload::Completed { .. });
        let expected_terminal_status = match &payload {
            FleetWorkerEventPayload::Completed { .. } => FleetTaskLedgerStatus::Completed,
            FleetWorkerEventPayload::Failed { .. } => FleetTaskLedgerStatus::Failed,
            FleetWorkerEventPayload::Cancelled { .. } => FleetTaskLedgerStatus::Cancelled,
            _ => bail!("fleet executor outcome must contain a terminal worker event"),
        };
        for tail_payload in tail_payloads {
            if is_terminal_payload(&tail_payload) {
                continue;
            }
            if self
                .ledger
                .append_event_if_leased(
                    &task.entry.run_id,
                    &task.worker_id,
                    &task.entry.task_id,
                    task.entry.attempts,
                    &timestamp(),
                    tail_payload,
                )?
                .is_none()
            {
                return Ok(false);
            }
        }
        let artifacts = self.task_artifacts_for_receipt(
            &task.entry.run_id,
            &task.entry.task_id,
            &task.worker_id,
        )?;
        // A terminal worker report is the sole authority for provider/model
        // actually used. Never re-resolve those fields through manager-local
        // config: remote workers may intentionally run a different config.
        // A headless worker that omits or malforms the terminal route fails
        // closed to no actual route. Pre-launch transport/simulated paths have
        // no process evidence by design, so they retain the explicitly labeled
        // intent route rather than pretending it was observed.
        let resolved_route = match (reported_route.as_ref(), requires_reported_route) {
            (Some(reported_route), _) => {
                self.resolve_reported_task_route(&task.task_spec, reported_route)
            }
            (None, true) => None,
            (None, false) => self.resolve_task_route(&task.task_spec),
        };
        let effective_permissions = self.resolve_task_effective_permissions(task);
        let verification_input = FleetTaskVerificationInput {
            run_id: task.entry.run_id.clone(),
            task_id: task.entry.task_id.clone(),
            worker_id: task.worker_id.clone(),
            attempt: task.entry.attempts,
            exit_code,
            artifacts,
            resolved_route,
            effective_permissions,
        };
        let receipt = if task.task_spec.scorer.is_some() || terminal_completed {
            let verification =
                verify_task_result(&self.workspace, &task.task_spec, &verification_input);
            prepare_verification_receipt(&self.workspace, &verification_input, verification)?
        } else {
            FleetReceipt {
                run_id: task.entry.run_id.clone(),
                task_id: task.entry.task_id.clone(),
                worker_id: task.worker_id.clone(),
                attempt: Some(task.entry.attempts),
                terminal_seq: None,
                completed_at: timestamp(),
                result: receipt_result,
                failure_kind,
                artifacts: verification_input.artifacts,
                score: None,
                resolved_route: verification_input.resolved_route,
                effective_permissions: verification_input.effective_permissions,
            }
        };
        let final_status = (matches!(
            receipt.result,
            FleetTaskResult::Fail | FleetTaskResult::Timeout
        ) && expected_terminal_status != FleetTaskLedgerStatus::Failed)
            .then_some(FleetTaskLedgerStatus::Failed);
        Ok(self
            .ledger
            .finalize_task_attempt_if_leased(
                &task.entry.run_id,
                &task.worker_id,
                &task.entry.task_id,
                task.entry.attempts,
                &timestamp(),
                payload,
                final_status,
                receipt,
            )?
            .is_some())
    }

    /// Resolve the route snapshot to persist on a task's receipt (#3154).
    ///
    /// Loads the merged agent roster so role/loadout intent composes the same
    /// way as the worker-spec path, then mints a secret-free route candidate via
    /// the hermetic resolver bridge. Returns `None` (never a fabricated route)
    /// when resolution is unavailable.
    fn resolve_task_route(&self, task_spec: &FleetTaskSpec) -> Option<FleetResolvedRoute> {
        let roster = self.agent_roster();
        worker_runtime::resolve_fleet_route_with_config(
            task_spec,
            roster.members(),
            self.session_model(),
            self.route_config.as_ref(),
        )
    }

    fn resolve_reported_task_route(
        &self,
        task_spec: &FleetTaskSpec,
        reported_route: &FleetWorkerReportedRoute,
    ) -> Option<FleetResolvedRoute> {
        let roster = self.agent_roster();
        worker_runtime::resolve_fleet_route_from_worker_report(
            task_spec,
            roster.members(),
            self.session_model(),
            &reported_route.provider,
            reported_route.provider_exact_id.as_deref(),
            &reported_route.model,
        )
    }

    /// The adopted session route, if any — the operator's model.
    fn session_model(&self) -> Option<&str> {
        self.session_model.as_deref()
    }

    /// Resolve the effective worker authority to persist on a task's receipt
    /// (#3211). This mirrors Fleet worker registration and applies exec
    /// hardening before snapshotting the runtime profile. Failures degrade to
    /// `None` so receipt writing never widens or fabricates authority.
    fn resolve_task_effective_permissions(
        &self,
        task: &FleetExecutorTaskContext,
    ) -> Option<FleetEffectivePermissions> {
        let state = self.ledger.rebuild_state().ok()?;
        let run = state.runs.get(&task.entry.run_id.0)?;
        let worker_spec = run
            .worker_specs
            .iter()
            .find(|worker| worker.id == task.worker_id)
            .cloned()
            .unwrap_or_else(|| default_local_worker(&task.worker_id));
        let roster = self.agent_roster();
        let worker = worker_runtime::fleet_task_to_worker_spec_with_profiles(
            &task.worker_id,
            &task.entry.run_id.0,
            &task.task_spec,
            &worker_spec,
            self.run_model(),
            &self.workspace,
            roster.members(),
            None,
        )
        .ok()?;
        let worker = bind_fleet_launch_attempt(
            worker_runtime::apply_exec_hardening(worker, &self.exec_config),
            task.entry.attempts,
        );
        Some(worker_runtime::fleet_effective_permissions_for_task(
            &task.task_spec,
            roster.members(),
            &worker,
        ))
    }

    fn task_artifacts_for_receipt(
        &self,
        run_id: &FleetRunId,
        task_id: &str,
        worker_id: &str,
    ) -> Result<Vec<FleetArtifactRef>> {
        let state = self.ledger.rebuild_state()?;
        Ok(state
            .artifact_events
            .values()
            .filter(|event| {
                event.run_id == *run_id && event.task_id == task_id && event.worker_id == worker_id
            })
            .filter_map(|event| match &event.payload {
                FleetWorkerEventPayload::Artifact(artifact) => {
                    Some(self.refresh_artifact_size(artifact.clone()))
                }
                _ => None,
            })
            .collect())
    }

    fn refresh_artifact_size(&self, mut artifact: FleetArtifactRef) -> FleetArtifactRef {
        let path = if artifact.path.is_absolute() {
            artifact.path.clone()
        } else {
            self.workspace.join(&artifact.path)
        };
        artifact.size_bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
        artifact
    }

    fn host_log_artifact(&self, path: &Path) -> FleetArtifactRef {
        let rel_path = path
            .strip_prefix(&self.workspace)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf());
        let size_bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
        FleetArtifactRef {
            kind: FleetArtifactKind::Log,
            path: rel_path,
            checksum: None,
            mime_type: Some("application/x-ndjson".to_string()),
            size_bytes,
        }
    }

    fn append_worker_event(
        &self,
        run_id: &FleetRunId,
        worker_id: &str,
        task_id: &str,
        payload: FleetWorkerEventPayload,
    ) -> Result<FleetWorkerEvent> {
        self.ledger
            .append_event_next_seq(run_id, worker_id, task_id, &timestamp(), payload)
    }

    fn write_log_artifact(
        &self,
        run_id: &FleetRunId,
        worker_id: &str,
        task_spec: &FleetTaskSpec,
    ) -> Result<FleetArtifactRef> {
        let rel_path = PathBuf::from(".codewhale")
            .join("fleet")
            .join(safe_path_segment(&run_id.0))
            .join(safe_path_segment(&task_spec.id))
            .join(format!("{}.log", safe_path_segment(worker_id)));
        let abs_path = self.workspace.join(&rel_path);
        if let Some(parent) = abs_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating fleet artifact dir {}", parent.display()))?;
        }
        let contents = format!(
            "run_id={}\ntask_id={}\ntask_name={}\nworker_id={}\nstatus=started\n",
            run_id.0, task_spec.id, task_spec.name, worker_id
        );
        std::fs::write(&abs_path, contents)
            .with_context(|| format!("writing fleet worker log {}", abs_path.display()))?;
        let size_bytes = std::fs::metadata(&abs_path).ok().map(|m| m.len());
        Ok(FleetArtifactRef {
            kind: FleetArtifactKind::Log,
            path: rel_path,
            checksum: None,
            mime_type: Some("text/plain".to_string()),
            size_bytes,
        })
    }

    fn refresh_run_status(&self, run_id: &FleetRunId) -> Result<()> {
        let state = self.ledger.rebuild_state()?;
        let mut has_queued = false;
        let mut has_running = false;
        let mut has_failed = false;
        let mut has_cancelled = false;
        let mut has_tasks = false;
        for task in state
            .tasks
            .values()
            .filter(|task| task.entry.run_id == *run_id)
        {
            has_tasks = true;
            match task.status {
                FleetTaskLedgerStatus::Enqueued => has_queued = true,
                FleetTaskLedgerStatus::Leased => has_running = true,
                FleetTaskLedgerStatus::Failed => has_failed = true,
                FleetTaskLedgerStatus::Cancelled => has_cancelled = true,
                FleetTaskLedgerStatus::Completed => {}
            }
        }
        let status = if !has_tasks {
            FleetRunStatus::Completed
        } else if has_queued || has_running {
            FleetRunStatus::Running
        } else if has_failed {
            FleetRunStatus::Failed
        } else if has_cancelled {
            FleetRunStatus::Cancelled
        } else {
            FleetRunStatus::Completed
        };
        self.ledger
            .update_run_status(run_id, status, &timestamp())
            .context("updating fleet run status")
    }

    fn status_from_state(
        &self,
        run_filter: Option<&FleetRunId>,
        state: &FleetLedgerState,
    ) -> FleetStatusSnapshot {
        let mut snapshot = FleetStatusSnapshot {
            runs: state.runs.len(),
            workers: state.workers.clone(),
            ..FleetStatusSnapshot::default()
        };
        for task in state.tasks.values() {
            if run_filter.is_some_and(|run_id| task.entry.run_id != *run_id) {
                continue;
            }
            match task.status {
                FleetTaskLedgerStatus::Enqueued => snapshot.queued += 1,
                FleetTaskLedgerStatus::Leased => {
                    if self.task_is_stale(task, state) {
                        snapshot.stale += 1;
                    } else {
                        snapshot.running += 1;
                    }
                }
                FleetTaskLedgerStatus::Completed => snapshot.completed += 1,
                FleetTaskLedgerStatus::Failed => snapshot.failed += 1,
                FleetTaskLedgerStatus::Cancelled => snapshot.cancelled += 1,
            }
        }
        for receipt in state.receipts.values() {
            if run_filter.is_some_and(|run_id| receipt.run_id != *run_id) {
                continue;
            }
            if receipt.result == FleetTaskResult::Partial {
                snapshot.partial += 1;
            }
            match &receipt.failure_kind {
                Some(FleetTaskFailureKind::Transport) => snapshot.transport_failed += 1,
                Some(FleetTaskFailureKind::Task) => snapshot.task_failed += 1,
                Some(FleetTaskFailureKind::Verifier) => snapshot.verifier_failed += 1,
                None => {}
            }
        }
        snapshot.restarted = state
            .restarted_events
            .values()
            .filter(|event| run_filter.is_none_or(|run_id| event.run_id == *run_id))
            .count();
        snapshot.escalated = state
            .escalated_events
            .values()
            .filter(|event| run_filter.is_none_or(|run_id| event.run_id == *run_id))
            .count();
        snapshot
    }

    fn task_is_stale(&self, task: &FleetTaskState, state: &FleetLedgerState) -> bool {
        let Some(worker_id) = task.leased_to.as_deref() else {
            return true;
        };
        let Some(heartbeat) = state.heartbeats.get(worker_id) else {
            return true;
        };
        let Ok(last) = DateTime::parse_from_rfc3339(&heartbeat.timestamp) else {
            return true;
        };
        let age = Utc::now().signed_duration_since(last.with_timezone(&Utc));
        age.to_std()
            .is_ok_and(|duration| duration > self.stale_after)
    }
}

fn default_local_workers(run_id: &FleetRunId, max_workers: usize) -> Vec<FleetWorkerSpec> {
    (1..=max_workers)
        .map(|index| {
            default_local_worker_with_name(&format!("{}-local-{}", run_id.0, index), index)
        })
        .collect()
}

fn default_local_worker_with_name(worker_id: &str, index: usize) -> FleetWorkerSpec {
    FleetWorkerSpec {
        id: worker_id.to_string(),
        name: format!("Local worker {index}"),
        host: FleetHostSpec::Local,
        trust_level: Some(FleetTrustLevel::Local),
        labels: BTreeMap::new(),
        capabilities: vec!["local".to_string()],
        max_concurrent_tasks: Some(1),
    }
}

fn default_local_worker(worker_id: &str) -> FleetWorkerSpec {
    FleetWorkerSpec {
        id: worker_id.to_string(),
        name: worker_id.to_string(),
        host: FleetHostSpec::Local,
        trust_level: Some(FleetTrustLevel::Local),
        labels: BTreeMap::new(),
        capabilities: vec!["local".to_string()],
        max_concurrent_tasks: Some(1),
    }
}

fn worker_ids_for_run(run: &FleetRun, max_workers: usize) -> Vec<String> {
    run.worker_specs
        .iter()
        .take(max_workers)
        .map(|worker| worker.id.clone())
        .collect()
}

fn active_workers_for_run(state: &FleetLedgerState, run_id: &FleetRunId) -> BTreeSet<String> {
    active_tasks_for_run(state, run_id)
        .filter_map(|task| task.leased_to.clone())
        .collect()
}

fn active_tasks_for_run<'a>(
    state: &'a FleetLedgerState,
    run_id: &'a FleetRunId,
) -> impl Iterator<Item = &'a FleetTaskState> {
    state.tasks.values().filter(move |task| {
        task.entry.run_id == *run_id && matches!(task.status, FleetTaskLedgerStatus::Leased)
    })
}

fn active_task_for_worker<'a>(
    state: &'a FleetLedgerState,
    worker_id: &str,
) -> Option<&'a FleetTaskState> {
    state.tasks.values().find(|task| {
        task.leased_to.as_deref() == Some(worker_id)
            && matches!(task.status, FleetTaskLedgerStatus::Leased)
    })
}

fn latest_task_for_worker<'a>(
    state: &'a FleetLedgerState,
    worker_id: &str,
) -> Option<&'a FleetTaskState> {
    state
        .tasks
        .values()
        .filter(|task| task.leased_to.as_deref() == Some(worker_id))
        .max_by_key(|task| task.completed_at.as_deref().or(task.leased_at.as_deref()))
}

fn next_enqueued_task_for_run(
    state: &FleetLedgerState,
    run_id: &FleetRunId,
) -> Option<(FleetInboxEntry, FleetTaskSpec)> {
    let run = state.runs.get(&run_id.0)?;
    let task = state
        .tasks
        .values()
        .filter(|task| {
            task.entry.run_id == *run_id && matches!(task.status, FleetTaskLedgerStatus::Enqueued)
        })
        .min_by_key(|task| {
            (
                task.entry.priority,
                task.entry.enqueued_at.clone(),
                task.entry.task_id.clone(),
            )
        })?;
    let task_spec = run
        .task_specs
        .iter()
        .find(|spec| spec.id == task.entry.task_id)
        .cloned()?;
    Some((task.entry.clone(), task_spec))
}

fn task_spec_for_state(state: &FleetLedgerState, task: &FleetTaskState) -> Option<FleetTaskSpec> {
    state
        .runs
        .get(&task.entry.run_id.0)?
        .task_specs
        .iter()
        .find(|spec| spec.id == task.entry.task_id)
        .cloned()
}

fn worker_host_for_run(
    state: &FleetLedgerState,
    run_id: &FleetRunId,
    worker_id: &str,
) -> Option<String> {
    let run = state.runs.get(&run_id.0)?;
    let worker = run
        .worker_specs
        .iter()
        .find(|worker| worker.id == worker_id)?;
    Some(host_label(&worker.host))
}

fn host_label(host: &FleetHostSpec) -> String {
    match host {
        FleetHostSpec::Local => "local".to_string(),
        FleetHostSpec::Ssh { host, .. } => format!("ssh:{host}"),
        FleetHostSpec::Docker { image, .. } => format!("docker:{image}"),
    }
}

fn latest_event_for_worker<'a>(
    state: &'a FleetLedgerState,
    worker_id: &str,
) -> Option<&'a FleetWorkerEvent> {
    state
        .latest_events
        .values()
        .filter(|event| event.worker_id == worker_id)
        .max_by_key(|event| event.seq)
}

fn latest_alert_for_worker(state: &FleetLedgerState, worker_id: &str) -> Option<String> {
    state
        .escalated_events
        .values()
        .filter(|event| event.worker_id == worker_id)
        .filter_map(|event| match &event.payload {
            FleetWorkerEventPayload::Escalated { channel, alert_id } => Some((
                event.seq,
                alert_id
                    .as_ref()
                    .map(|alert_id| format!("escalated via {channel} alert_id={alert_id}"))
                    .unwrap_or_else(|| format!("escalated via {channel}")),
            )),
            _ => None,
        })
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, message)| message)
}

fn latest_receipt_for_worker<'a>(
    state: &'a FleetLedgerState,
    worker_id: &str,
) -> Option<&'a FleetReceipt> {
    state
        .receipts
        .values()
        .filter(|receipt| receipt.worker_id == worker_id)
        .max_by_key(|receipt| &receipt.completed_at)
}

fn receipt_summary(receipt: &FleetReceipt) -> String {
    let result = match receipt.result {
        FleetTaskResult::Pass => "pass",
        FleetTaskResult::Partial => "partial",
        FleetTaskResult::Fail => "fail",
        FleetTaskResult::Skip => "skip",
        FleetTaskResult::Timeout => "timeout",
    };
    let mut summary = format!("result={result}");
    if let Some(kind) = &receipt.failure_kind {
        let kind = match kind {
            FleetTaskFailureKind::Transport => "transport",
            FleetTaskFailureKind::Task => "task",
            FleetTaskFailureKind::Verifier => "verifier",
        };
        summary.push_str(&format!(" failure_kind={kind}"));
    }
    if let Some(notes) = receipt
        .score
        .as_ref()
        .and_then(|score| score.notes.as_deref())
        .filter(|notes| !notes.trim().is_empty())
    {
        summary.push_str(&format!(" notes={notes}"));
    }
    summary
}

fn latest_error_for_worker(state: &FleetLedgerState, worker_id: &str) -> Option<String> {
    state
        .latest_events
        .values()
        .filter(|event| event.worker_id == worker_id)
        .filter_map(|event| match &event.payload {
            FleetWorkerEventPayload::Failed { reason, .. } => {
                Some((event.seq, format!("failed: {reason}")))
            }
            FleetWorkerEventPayload::Cancelled { cancelled_by } => Some((
                event.seq,
                cancelled_by
                    .as_ref()
                    .map(|by| format!("cancelled by {by}"))
                    .unwrap_or_else(|| "cancelled".to_string()),
            )),
            FleetWorkerEventPayload::Interrupted { signal } => Some((
                event.seq,
                signal
                    .as_ref()
                    .map(|signal| format!("interrupted by {signal}"))
                    .unwrap_or_else(|| "interrupted".to_string()),
            )),
            FleetWorkerEventPayload::Stale { last_heartbeat_at } => Some((
                event.seq,
                last_heartbeat_at
                    .as_ref()
                    .map(|ts| format!("stale since {ts}"))
                    .unwrap_or_else(|| "stale".to_string()),
            )),
            _ => None,
        })
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, message)| message)
}

fn task_priority(task: &FleetTaskSpec) -> i32 {
    task.metadata
        .get("priority")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(0)
}

fn resolve_task_cwd(workspace: &Path, task: &FleetTaskSpec) -> Result<PathBuf> {
    let Some(root) = task
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.root.as_ref())
    else {
        return crate::tools::spec::resolve_strict_authority_path(
            &crate::tools::ToolContext::new(workspace.to_path_buf()),
            ".",
        )
        .map_err(anyhow::Error::new);
    };
    crate::tools::spec::resolve_strict_authority_path(
        &crate::tools::ToolContext::new(workspace.to_path_buf()),
        &root.to_string_lossy(),
    )
    .map_err(anyhow::Error::new)
}

fn bind_fleet_launch_attempt(mut spec: AgentWorkerSpec, attempt: u32) -> AgentWorkerSpec {
    // The outer machine-readable cap is workspace-relative and cannot yet be
    // intersected into a grandchild's narrower launch context. Fleet workers
    // are therefore truthful leaves in v0.9.1: the nested-agent surface is
    // disabled for the authority-bound subprocess.
    spec.max_spawn_depth = 0;
    spec.runtime_profile.max_spawn_depth = 0;
    if let Some(manifest) = spec.launch_manifest.as_mut() {
        manifest.generation = attempt.max(1);
        manifest.profile.max_spawn_depth = 0;
    }
    spec
}

fn validate_registered_launch_spec(
    registered: &AgentWorkerSpec,
    expected: &AgentWorkerSpec,
) -> Result<()> {
    let Some(registered_manifest) = registered.launch_manifest.as_ref() else {
        bail!(
            "Fleet worker {} has no persisted launch manifest",
            registered.worker_id
        );
    };
    if registered_manifest.prompt != registered.objective
        || !registered_prompt_matches_expected(&registered.objective, &expected.objective)
    {
        bail!(
            "Fleet worker {} has an inconsistent persisted prompt",
            registered.worker_id
        );
    }

    // Coordination may append a bounded decision projection to the prompt.
    // Every identity, route, permission, workspace, scope, and attempt field
    // must otherwise match a fresh derivation from this exact leased task.
    let mut registered_identity = registered.clone();
    let mut expected_identity = expected.clone();
    registered_identity.objective.clear();
    expected_identity.objective.clear();
    if let Some(manifest) = registered_identity.launch_manifest.as_mut() {
        manifest.prompt.clear();
    }
    if let Some(manifest) = expected_identity.launch_manifest.as_mut() {
        manifest.prompt.clear();
    }
    if registered_identity != expected_identity {
        bail!(
            "Fleet worker {} persisted launch spec does not match the exact task lease and attempt",
            registered.worker_id
        );
    }
    Ok(())
}

fn registered_prompt_matches_expected(registered: &str, expected: &str) -> bool {
    const HEADER: &str = "Accepted coordination decisions relevant to this child (bounded):\n";
    if registered == expected {
        return true;
    }
    let Some(projection) = registered
        .strip_prefix(expected)
        .and_then(|suffix| suffix.strip_prefix("\n\n"))
    else {
        return false;
    };
    let Some(lines) = projection.strip_prefix(HEADER) else {
        return false;
    };
    !lines.is_empty()
        && projection.len() <= 4096
        && lines.lines().count() <= 8
        && lines
            .lines()
            .all(|line| line.starts_with("- ") && line.len() <= 512)
}

fn validate_task_cwd_for_host(
    workspace: &Path,
    host: &FleetHostSpec,
    task_cwd: &Path,
) -> Result<()> {
    if !matches!(host, FleetHostSpec::Ssh { .. }) {
        return Ok(());
    }
    let workspace_root = crate::tools::spec::resolve_strict_authority_path(
        &crate::tools::ToolContext::new(workspace.to_path_buf()),
        ".",
    )
    .map_err(anyhow::Error::new)?;
    if task_cwd != workspace_root {
        bail!(
            "SSH Fleet workers do not yet support nested workspace.root values; task cwd '{}' cannot be mapped safely beneath the remote working_directory",
            task_cwd.display()
        );
    }
    Ok(())
}

fn task_receipt_outcome(
    payload: &FleetWorkerEventPayload,
    exit_code: Option<i32>,
) -> (FleetTaskResult, Option<FleetTaskFailureKind>, Option<i32>) {
    match payload {
        FleetWorkerEventPayload::Completed {
            exit_code: payload_exit_code,
            ..
        } => (
            FleetTaskResult::Pass,
            None,
            exit_code.or(*payload_exit_code),
        ),
        FleetWorkerEventPayload::Cancelled { .. } => (FleetTaskResult::Skip, None, exit_code),
        FleetWorkerEventPayload::Failed { .. } => {
            let failure_kind = if exit_code.is_none() {
                FleetTaskFailureKind::Transport
            } else {
                FleetTaskFailureKind::Task
            };
            (FleetTaskResult::Fail, Some(failure_kind), exit_code)
        }
        _ => (FleetTaskResult::Partial, None, exit_code),
    }
}

fn is_terminal_payload(payload: &FleetWorkerEventPayload) -> bool {
    matches!(
        payload,
        FleetWorkerEventPayload::Completed { .. }
            | FleetWorkerEventPayload::Failed { .. }
            | FleetWorkerEventPayload::Cancelled { .. }
            | FleetWorkerEventPayload::Interrupted { .. }
    )
}

fn task_key(run_id: &str, task_id: &str) -> String {
    format!("{run_id}:{task_id}")
}

fn event_key(worker_id: &str, run_id: &str, task_id: &str) -> String {
    format!("{worker_id}:{run_id}:{task_id}")
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn task(id: &str) -> FleetTaskSpec {
        FleetTaskSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            objective: Some(format!("Complete {id}")),
            instructions: format!("do {id}"),
            worker: None,
            workspace: None,
            input_files: Vec::new(),
            context: Vec::new(),
            budget: None,
            tags: Vec::new(),
            expected_artifacts: vec![FleetArtifactKind::Log],
            scorer: None,
            retry_policy: None,
            alert_policy: None,
            timeout_seconds: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn ssh_workers_fail_closed_for_nested_task_roots() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("nested")).unwrap();
        let mut nested = task("nested");
        nested.workspace = Some(FleetWorkspaceRequirements {
            root: Some(PathBuf::from("nested")),
            ..FleetWorkspaceRequirements::default()
        });
        let nested_cwd = resolve_task_cwd(tmp.path(), &nested).unwrap();
        let ssh = FleetHostSpec::Ssh {
            host: "builder.example.test".to_string(),
            port: None,
            user: None,
            identity: None,
            known_hosts: None,
            host_key_fingerprint: None,
            working_directory: Some(PathBuf::from("/srv/codewhale")),
            env_allowlist: Vec::new(),
            codewhale_binary: Some("/usr/local/bin/codewhale".to_string()),
        };

        let error = validate_task_cwd_for_host(tmp.path(), &ssh, &nested_cwd)
            .expect_err("nested SSH task roots must fail closed");
        assert!(error.to_string().contains("cannot be mapped safely"));

        let root_cwd = resolve_task_cwd(tmp.path(), &task("root")).unwrap();
        validate_task_cwd_for_host(tmp.path(), &ssh, &root_cwd).unwrap();
        validate_task_cwd_for_host(tmp.path(), &FleetHostSpec::Local, &nested_cwd).unwrap();
    }

    #[test]
    fn ssh_nested_root_failure_commits_neither_lease_nor_coordination_record() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("nested")).unwrap();
        let coordination =
            crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_sub_agent_manager(coordination.clone());
        let mut nested = task("nested");
        nested.worker = Some(FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: Vec::new(),
            capabilities: Vec::new(),
        });
        nested.workspace = Some(FleetWorkspaceRequirements {
            root: Some(PathBuf::from("nested")),
            ..FleetWorkspaceRequirements::default()
        });
        let worker = FleetWorkerSpec {
            id: "ssh-worker".to_string(),
            name: "SSH worker".to_string(),
            host: FleetHostSpec::Ssh {
                host: "builder.example.test".to_string(),
                port: None,
                user: None,
                identity: None,
                known_hosts: None,
                host_key_fingerprint: None,
                working_directory: Some(PathBuf::from("/srv/codewhale")),
                env_allowlist: Vec::new(),
                codewhale_binary: Some("/usr/local/bin/codewhale".to_string()),
            },
            trust_level: None,
            labels: BTreeMap::new(),
            capabilities: Vec::new(),
            max_concurrent_tasks: Some(1),
        };
        let error = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("nested SSH".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: None,
                    workers: vec![worker],
                    tasks: vec![nested],
                },
                1,
            )
            .expect_err("nested SSH root must fail before leasing");
        assert!(error.to_string().contains("cannot be mapped safely"));

        let state = manager.rebuild_state().unwrap();
        let task = state.tasks.values().next().expect("queued task remains");
        assert_eq!(task.status, FleetTaskLedgerStatus::Enqueued);
        assert_eq!(task.entry.attempts, 0);
        assert!(task.leased_to.is_none());
        assert!(
            coordination
                .try_read()
                .unwrap()
                .list_worker_records()
                .is_empty()
        );
    }

    #[test]
    fn ledger_append_failure_rolls_back_coordination_registration() {
        let tmp = TempDir::new().unwrap();
        let coordination =
            crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_sub_agent_manager(coordination.clone());
        let mut spec = task("read-only");
        spec.worker = Some(FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: Vec::new(),
            capabilities: Vec::new(),
        });
        manager.ledger.fail_next_start_append_after_callback();

        let error = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("forced rollback".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: None,
                    workers: Vec::new(),
                    tasks: vec![spec],
                },
                1,
            )
            .expect_err("forced append failure");
        assert!(error.to_string().contains("forced Fleet ledger append"));

        let state = manager.rebuild_state().unwrap();
        let task = state.tasks.values().next().expect("queued task remains");
        assert_eq!(task.status, FleetTaskLedgerStatus::Enqueued);
        assert_eq!(task.entry.attempts, 0);
        assert!(task.leased_to.is_none());
        let guard = coordination.try_read().unwrap();
        assert!(guard.list_worker_records().is_empty());
        assert!(guard.coordination_snapshot().write_claims.is_empty());
    }

    #[test]
    fn invalid_worker_identity_is_rejected_before_run_journal_creation() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let error = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("invalid identity".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: None,
                    workers: vec![resume_worker_spec("worker\r\nforged")],
                    tasks: vec![task("task-a")],
                },
                1,
            )
            .expect_err("multiline worker identity must fail before journaling");
        assert!(
            error
                .to_string()
                .contains("worker id must be a simple ASCII token")
        );

        let state = manager.rebuild_state().unwrap();
        assert!(state.runs.is_empty());
        assert!(state.tasks.is_empty());
    }

    #[test]
    fn restored_lease_without_launch_record_fails_durably_instead_of_poisoning_ticks() {
        let tmp = TempDir::new().unwrap();
        let coordination =
            crate::tools::subagent::new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let empty_snapshot = coordination
            .try_read()
            .unwrap()
            .coordination_registration_snapshot();
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_sub_agent_manager(coordination.clone());
        let mut spec = task("task-a");
        spec.worker = Some(FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: Vec::new(),
            capabilities: Vec::new(),
        });
        let report = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("restored lease".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: None,
                    workers: Vec::new(),
                    tasks: vec![spec],
                },
                1,
            )
            .unwrap();
        coordination
            .try_write()
            .unwrap()
            .restore_coordination_registration_snapshot(empty_snapshot)
            .unwrap();

        let mut executor = FleetExecutor::new(tmp.path());
        manager
            .drive_executor_tick(&report.run_id, &mut executor, "unused-codewhale", None)
            .expect("missing restored launch state must become a durable task failure");
        manager
            .drive_executor_tick(&report.run_id, &mut executor, "unused-codewhale", None)
            .expect("the next scheduler tick must not remain poisoned");

        let state = manager.rebuild_state().unwrap();
        let key = task_key(&report.run_id.0, "task-a");
        assert_eq!(state.tasks[&key].status, FleetTaskLedgerStatus::Failed);
        assert_eq!(state.receipts[&key].result, FleetTaskResult::Fail);
        assert!(
            latest_error_for_worker(&state, &report.worker_ids[0])
                .is_some_and(|error| error.contains("no coordination-registered launch spec"))
        );
    }

    fn read_only_launch_spec(workspace: &Path, task_id: &str, attempt: u32) -> AgentWorkerSpec {
        let mut spec = task(task_id);
        spec.worker = Some(FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: Vec::new(),
            capabilities: Vec::new(),
        });
        bind_fleet_launch_attempt(
            worker_runtime::fleet_task_to_worker_spec_with_profiles(
                "worker-1",
                "run-1",
                &spec,
                &default_local_worker("worker-1"),
                "auto",
                workspace,
                &[],
                None,
            )
            .unwrap(),
            attempt,
        )
    }

    #[test]
    fn persisted_launch_identity_is_bound_to_task_and_attempt() {
        let tmp = TempDir::new().unwrap();
        let task_a = read_only_launch_spec(tmp.path(), "task-a", 1);
        let task_b = read_only_launch_spec(tmp.path(), "task-b", 1);
        let task_b_retry = read_only_launch_spec(tmp.path(), "task-b", 2);

        assert_eq!(task_b.max_spawn_depth, 0);
        assert_eq!(task_b.runtime_profile.max_spawn_depth, 0);
        assert_eq!(
            task_b
                .launch_manifest
                .as_ref()
                .unwrap()
                .profile
                .max_spawn_depth,
            0
        );
        validate_registered_launch_spec(&task_b, &task_b).unwrap();
        assert!(validate_registered_launch_spec(&task_a, &task_b).is_err());
        assert!(validate_registered_launch_spec(&task_b, &task_b_retry).is_err());

        let mut projected = task_b.clone();
        projected.objective.push_str(
            "\n\nAccepted coordination decisions relevant to this child (bounded):\n- api v1 [decision-1] owner=planner: keep scope bounded",
        );
        projected.launch_manifest.as_mut().unwrap().prompt = projected.objective.clone();
        validate_registered_launch_spec(&projected, &task_b)
            .expect("a bounded coordination prompt projection preserves launch identity");

        let mut corrupt = task_b.clone();
        corrupt.objective.push_str("\n\narbitrary stale prompt");
        corrupt.launch_manifest.as_mut().unwrap().prompt = corrupt.objective.clone();
        assert!(validate_registered_launch_spec(&corrupt, &task_b).is_err());
    }

    #[test]
    fn with_session_model_adopts_route_and_ignores_auto_or_empty() {
        let tmp = TempDir::new().unwrap();

        // No session: legacy auto sentinel.
        let manager = FleetManager::open(tmp.path()).unwrap();
        assert_eq!(manager.run_model(), "auto");
        assert_eq!(manager.session_model(), None);

        // The session route becomes the run model — the operator's model.
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_session_model("deepseek-v4-pro");
        assert_eq!(manager.run_model(), "deepseek-v4-pro");
        assert_eq!(manager.session_model(), Some("deepseek-v4-pro"));

        // "auto" and empty/whitespace inputs keep the resolver default.
        for noop in ["auto", "AUTO", "", "   "] {
            let manager = FleetManager::open(tmp.path())
                .unwrap()
                .with_session_model(noop);
            assert_eq!(manager.run_model(), "auto");
            assert_eq!(manager.session_model(), None);
        }
    }

    fn task_spec_file(dir: &TempDir, tasks: Vec<FleetTaskSpec>) -> PathBuf {
        let path = dir.path().join("fleet-tasks.json");
        let doc = json!({
            "name": "manager smoke",
            "tasks": tasks,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
        path
    }

    #[cfg(unix)]
    fn fake_codewhale(dir: &TempDir, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.path().join("fake-codewhale");
        std::fs::write(&path, body).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[cfg(unix)]
    fn complete_with_fake_codewhale(
        manager: &FleetManager,
        run_id: &FleetRunId,
        max_workers: usize,
        binary: &Path,
    ) -> FleetStatusSnapshot {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut executor = FleetExecutor::new(&manager.workspace);
        rt.block_on(async {
            manager
                .run_to_completion(
                    run_id,
                    max_workers,
                    &mut executor,
                    &binary.display().to_string(),
                    None,
                    Duration::from_millis(10),
                )
                .await
                .unwrap()
        })
    }

    const RESUME_T0: &str = "2026-06-13T01:00:00Z";

    fn role_task_with_retry(id: &str, role: &str, max_attempts: u32) -> FleetTaskSpec {
        let mut spec = task(id);
        spec.worker = Some(FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some(role.to_string()),
            loadout: None,
            model: None,
            model_class: None,
            tool_profile: None,
            tools: Vec::new(),
            capabilities: Vec::new(),
        });
        spec.retry_policy = Some(FleetRetryPolicy {
            max_attempts,
            ..FleetRetryPolicy::default()
        });
        spec
    }

    fn resume_worker_spec(id: &str) -> FleetWorkerSpec {
        FleetWorkerSpec {
            id: id.to_string(),
            name: id.to_string(),
            host: FleetHostSpec::Local,
            trust_level: Some(FleetTrustLevel::Local),
            labels: BTreeMap::new(),
            capabilities: vec!["local".to_string()],
            max_concurrent_tasks: Some(1),
        }
    }

    fn resume_now(offset_secs: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(RESUME_T0)
            .unwrap()
            .with_timezone(&Utc)
            + chrono::Duration::seconds(offset_secs)
    }

    /// Seed the durable ledger with the state a crashed manager would leave: a
    /// running run whose `completed` task ids finished with receipts, and whose
    /// `orphaned` (task_id, worker_id) pairs are still `Leased` to workers that
    /// last heartbeat at `heartbeat_ts` — stale once the resume clock advances
    /// past `stale_after`.
    fn seed_crashed_run(
        ledger: &FleetLedger,
        run_id: &FleetRunId,
        tasks: &[FleetTaskSpec],
        workers: &[FleetWorkerSpec],
        completed: &[&str],
        orphaned: &[(&str, &str)],
        heartbeat_ts: &str,
    ) {
        ledger
            .create_run(&FleetRun {
                id: run_id.clone(),
                name: "resume smoke".to_string(),
                status: FleetRunStatus::Running,
                max_workers: Some(workers.len().max(1)),
                task_specs: tasks.to_vec(),
                worker_specs: workers.to_vec(),
                labels: BTreeMap::new(),
                security_policy: None,
                created_at: heartbeat_ts.to_string(),
                updated_at: Some(heartbeat_ts.to_string()),
                completed_at: None,
            })
            .unwrap();
        for spec in tasks {
            ledger
                .enqueue(FleetInboxEntry {
                    run_id: run_id.clone(),
                    task_id: spec.id.clone(),
                    priority: 0,
                    enqueued_at: heartbeat_ts.to_string(),
                    lease_deadline: None,
                    attempts: 0,
                })
                .unwrap();
        }
        for (idx, &task_id) in completed.iter().enumerate() {
            let worker_id = format!("done-worker-{idx}");
            ledger
                .lease_task(run_id, task_id, &worker_id, heartbeat_ts, None)
                .unwrap();
            ledger
                .mark_task_terminal_status(
                    run_id,
                    task_id,
                    Some(worker_id.as_str()),
                    heartbeat_ts,
                    FleetTaskLedgerStatus::Completed,
                )
                .unwrap();
            ledger
                .record_receipt(FleetReceipt {
                    run_id: run_id.clone(),
                    task_id: task_id.to_string(),
                    worker_id,
                    attempt: Some(1),
                    terminal_seq: None,
                    completed_at: heartbeat_ts.to_string(),
                    result: FleetTaskResult::Pass,
                    failure_kind: None,
                    artifacts: Vec::new(),
                    score: None,
                    resolved_route: None,
                    effective_permissions: None,
                })
                .unwrap();
        }
        for &(task_id, worker_id) in orphaned {
            ledger
                .lease_task(run_id, task_id, worker_id, heartbeat_ts, None)
                .unwrap();
            ledger
                .heartbeat(worker_id, heartbeat_ts, None, None)
                .unwrap();
        }
    }

    #[test]
    fn fleet_resume_reconciles_orphaned_lease_and_retries_within_budget() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let run_id = FleetRunId::from("resume-run");
        // Three roles, three workers; scout and verifier finished, builder is
        // orphaned mid-flight (its worker stopped heartbeating at the crash).
        let tasks = vec![
            role_task_with_retry("scout-1", "read-only", 3),
            role_task_with_retry("build-1", "builder", 3),
            role_task_with_retry("verify-1", "smoke-runner", 3),
        ];
        let workers = vec![
            resume_worker_spec("w-scout"),
            resume_worker_spec("w-build"),
            resume_worker_spec("w-verify"),
        ];
        seed_crashed_run(
            &ledger,
            &run_id,
            &tasks,
            &workers,
            &["scout-1", "verify-1"],
            &[("build-1", "w-build")],
            RESUME_T0,
        );

        // Restart: a fresh manager over the same workspace resumes from ledger.
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_stale_after(Duration::from_secs(5));
        let outcome = manager.resume_run_at(&run_id, resume_now(30)).unwrap();

        assert_eq!(
            outcome.reclaimed_stale, 1,
            "orphaned builder lease detected stale"
        );
        assert_eq!(outcome.restarted, 1, "builder retried within budget");
        assert_eq!(outcome.failed, 0);
        assert_eq!(outcome.escalated, 0);
        assert_eq!(
            outcome.status.completed, 2,
            "pre-crash completions preserved"
        );
        assert_eq!(outcome.status.restarted, 1);

        let state = manager.rebuild_state().unwrap();
        assert_eq!(state.receipts.len(), 2, "pre-crash receipts survive resume");
        let builder = &state.tasks["resume-run:build-1"];
        assert_eq!(builder.status, FleetTaskLedgerStatus::Leased);
        assert_eq!(builder.entry.attempts, 2, "retry leased a second attempt");

        let text = std::fs::read_to_string(manager.ledger_path()).unwrap();
        assert!(
            text.contains("\"state\":\"stale\""),
            "stale event durably recorded"
        );
        assert!(
            text.contains("\"state\":\"restarted\""),
            "restart durably recorded"
        );
    }

    #[test]
    fn fleet_resume_exhausted_retry_fails_and_escalates_idempotently() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let run_id = FleetRunId::from("resume-run");
        let mut builder = role_task_with_retry("build-1", "builder", 1);
        builder.alert_policy = Some(FleetAlertPolicy {
            events: vec![FleetAlertEventClass::RestartExhausted],
            channels: vec![FleetAlertChannel::Slack {
                webhook: FleetAlertEndpoint::inline("https://hooks.slack.invalid/secret"),
            }],
            after_attempts: Some(1),
            after_minutes_stale: Some(1),
        });
        let tasks = vec![builder];
        let workers = vec![resume_worker_spec("w-build")];
        seed_crashed_run(
            &ledger,
            &run_id,
            &tasks,
            &workers,
            &[],
            &[("build-1", "w-build")],
            RESUME_T0,
        );

        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_stale_after(Duration::from_secs(5));
        let outcome = manager.resume_run_at(&run_id, resume_now(30)).unwrap();

        assert_eq!(outcome.reclaimed_stale, 1);
        assert_eq!(outcome.restarted, 0);
        assert_eq!(outcome.failed, 1, "exhausted retry budget fails the task");
        assert_eq!(
            outcome.escalated, 1,
            "exhaustion escalates per alert policy"
        );
        assert_eq!(outcome.status.failed, 1);
        assert_eq!(outcome.status.escalated, 1);

        let text = std::fs::read_to_string(manager.ledger_path()).unwrap();
        assert!(text.contains("\"state\":\"failed\""));
        assert!(text.contains("\"record\":\"alert_sent\""));
        assert_eq!(manager.rebuild_state().unwrap().escalated_events.len(), 1);
        assert!(
            !text.contains("hooks.slack.invalid/secret"),
            "secret webhook redacted in ledger"
        );

        // Resuming again must not resurrect or re-escalate a terminal failure.
        let again = manager.resume_run_at(&run_id, resume_now(30)).unwrap();
        assert_eq!(again.reclaimed_stale, 0);
        assert_eq!(again.failed, 0);
        assert_eq!(again.escalated, 0);
        assert_eq!(
            manager.run_status(&run_id).unwrap().escalated,
            1,
            "no duplicate escalation across resumes"
        );
    }

    #[test]
    fn fleet_resume_retry_is_idempotent_at_same_instant() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let run_id = FleetRunId::from("resume-run");
        let tasks = vec![role_task_with_retry("build-1", "builder", 3)];
        let workers = vec![resume_worker_spec("w-build")];
        seed_crashed_run(
            &ledger,
            &run_id,
            &tasks,
            &workers,
            &[],
            &[("build-1", "w-build")],
            RESUME_T0,
        );

        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_stale_after(Duration::from_secs(5));
        let first = manager.resume_run_at(&run_id, resume_now(30)).unwrap();
        assert_eq!(first.restarted, 1);

        // Re-leased at the resume instant, the task is no longer stale, so a
        // second resume at the same instant is a no-op (no double retry).
        let second = manager.resume_run_at(&run_id, resume_now(30)).unwrap();
        assert_eq!(second.reclaimed_stale, 0);
        assert_eq!(second.restarted, 0);
        assert_eq!(
            manager.rebuild_state().unwrap().tasks["resume-run:build-1"]
                .entry
                .attempts,
            2,
            "attempts did not double on the second resume"
        );
    }

    #[test]
    fn fleet_resume_uses_wall_clock_for_stale_detection() {
        let tmp = TempDir::new().unwrap();
        let ledger = FleetLedger::open(tmp.path()).unwrap();
        let run_id = FleetRunId::from("resume-run");
        let tasks = vec![role_task_with_retry("build-1", "builder", 3)];
        let workers = vec![resume_worker_spec("w-build")];
        // Heartbeat an hour in the past so it is reliably stale under the real
        // wall clock used by the production `resume_run` entrypoint.
        let stale_ts = (Utc::now() - chrono::Duration::seconds(3600))
            .to_rfc3339_opts(SecondsFormat::Secs, true);
        seed_crashed_run(
            &ledger,
            &run_id,
            &tasks,
            &workers,
            &[],
            &[("build-1", "w-build")],
            &stale_ts,
        );

        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_stale_after(Duration::from_secs(5));
        let outcome = manager.resume_run(&run_id).unwrap();

        assert_eq!(outcome.reclaimed_stale, 1);
        assert_eq!(outcome.restarted, 1);
    }

    #[test]
    fn fleet_manager_creates_run_and_starts_workers_up_to_cap() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a"), task("task-b"), task("task-c")]);

        let report = manager.create_run_from_task_spec_path(&path, 2).unwrap();

        assert_eq!(report.task_count, 3);
        assert_eq!(report.leased, 2);
        assert_eq!(report.queued, 1);
        assert_eq!(report.worker_ids.len(), 2);
        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.queued, 1);
        assert_eq!(status.running, 2);
        assert_eq!(status.completed, 0);
    }

    #[test]
    fn fleet_manager_rejects_unknown_agent_profile_before_run_creation() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let mut task = task("task-a");
        task.worker = Some(FleetTaskWorkerProfile {
            role: None,
            agent_profile: Some("missing".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: None,
            tools: Vec::new(),
            capabilities: Vec::new(),
        });
        let doc = FleetTaskSpecDocument {
            name: Some("profile guard".to_string()),
            labels: BTreeMap::new(),
            security_policy: None,
            workers: Vec::new(),
            tasks: vec![task],
        };

        let err = manager
            .create_run(doc, 1)
            .expect_err("unknown agent profile must reject the run");

        assert!(
            err.to_string()
                .contains("references unknown agent profile \"missing\"")
        );
        assert!(manager.ledger.rebuild_state().unwrap().runs.is_empty());
    }

    #[test]
    fn fleet_manager_inspect_exposes_heartbeat_artifacts_and_errors() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = &report.worker_ids[0];

        let inspection = manager.inspect_worker(worker_id).unwrap();
        assert_eq!(inspection.status, FleetWorkerStatus::Busy);
        assert_eq!(inspection.current_task_id.as_deref(), Some("task-a"));
        assert!(inspection.latest_heartbeat_at.is_some());
        assert_eq!(inspection.artifacts.len(), 1);
        assert!(inspection.last_error.is_none());

        let inspection = manager.interrupt_worker(worker_id).unwrap();
        assert_eq!(inspection.status, FleetWorkerStatus::Online);
        assert_eq!(
            inspection.last_error.as_deref(),
            Some("cancelled by operator")
        );
        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.cancelled, 1);
    }

    #[cfg(unix)]
    #[test]
    fn separate_manager_interrupt_stops_live_worker_and_stays_terminal() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let controller = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a"), task("task-b")]);
        let pid_path = tmp.path().join("live-worker.pid");
        let first_worker_marker = tmp.path().join("first-worker-started");
        let stopped_marker = tmp.path().join("first-worker-stopped");
        let fake = fake_codewhale(
            &tmp,
            &format!(
                r#"#!/bin/sh
if [ -e '{first_worker_marker}' ]; then
  printf '{{"type":"content","content":"second task"}}\n'
  exit 0
fi
touch '{first_worker_marker}'
printf '%s' "$$" > '{}'
printf '{{"type":"content","content":"running"}}\n'
trap 'touch "{stopped_marker}"; exit 0' INT TERM
sleep 30
"#,
                pid_path.display(),
                first_worker_marker = first_worker_marker.display(),
                stopped_marker = stopped_marker.display(),
            ),
        );
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = report.worker_ids[0].clone();
        let mut executor = FleetExecutor::new(&manager.workspace);
        let rt = tokio::runtime::Runtime::new().unwrap();

        let (status, interrupted) = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(15), async {
                tokio::join!(
                    async {
                        manager
                            .run_to_completion(
                                &report.run_id,
                                1,
                                &mut executor,
                                &fake.display().to_string(),
                                None,
                                Duration::from_millis(10),
                            )
                            .await
                            .unwrap()
                    },
                    async {
                        tokio::time::timeout(Duration::from_secs(10), async {
                            while !pid_path.is_file() {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                        })
                        .await
                        .expect("fake worker never started");
                        controller.interrupt_worker(&worker_id).unwrap()
                    }
                )
            })
            .await
            .expect("Fleet cancellation did not beat the worker's natural exit")
        });

        assert_eq!(interrupted.status, FleetWorkerStatus::Online);
        assert_eq!(status.cancelled, 1);
        assert_eq!(status.completed, 1);
        assert_eq!(status.running, 0);
        assert!(executor.worker_ids().is_empty());
        assert!(
            stopped_marker.is_file(),
            "cancelled Fleet worker did not observe the stop signal"
        );

        let inspection = controller.inspect_worker(&worker_id).unwrap();
        assert_eq!(inspection.status, FleetWorkerStatus::Online);
        let state = controller.rebuild_state().unwrap();
        let task_key = task_key(&report.run_id.0, "task-a");
        assert_eq!(
            state.tasks[&task_key].status,
            FleetTaskLedgerStatus::Cancelled
        );
        let event_key = event_key(&worker_id, &report.run_id.0, "task-a");
        assert!(matches!(
            &state.latest_events[&event_key].payload,
            FleetWorkerEventPayload::Cancelled { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn live_restart_fences_old_process_and_only_attempt_two_completes() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let controller = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let first_worker_marker = tmp.path().join("first-attempt-started");
        let stopped_marker = tmp.path().join("first-attempt-stopped");
        let fake = fake_codewhale(
            &tmp,
            &format!(
                r#"#!/bin/sh
if [ -e '{first_worker_marker}' ]; then
  printf '{{"type":"content","content":"attempt two"}}\n'
  exit 0
fi
touch '{first_worker_marker}'
printf '{{"type":"content","content":"attempt one still running"}}\n'
trap 'touch "{stopped_marker}"; exit 0' INT TERM
while :; do sleep 1; done
"#,
                first_worker_marker = first_worker_marker.display(),
                stopped_marker = stopped_marker.display(),
            ),
        );
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = report.worker_ids[0].clone();
        let mut executor = FleetExecutor::new(&manager.workspace);
        let rt = tokio::runtime::Runtime::new().unwrap();

        let status = rt.block_on(async {
            tokio::time::timeout(Duration::from_secs(15), async {
                tokio::join!(
                    async {
                        manager
                            .run_to_completion(
                                &report.run_id,
                                1,
                                &mut executor,
                                &fake.display().to_string(),
                                None,
                                Duration::from_millis(10),
                            )
                            .await
                            .unwrap()
                    },
                    async {
                        tokio::time::timeout(Duration::from_secs(10), async {
                            while !first_worker_marker.is_file() {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                            }
                        })
                        .await
                        .expect("first Fleet attempt never started");
                        controller.restart_worker(&worker_id).unwrap();
                    }
                )
                .0
            })
            .await
            .expect("restarted Fleet task did not finish")
        });

        assert_eq!(status.completed, 1);
        assert_eq!(status.running, 0);
        assert_eq!(status.restarted, 1);
        assert!(executor.worker_ids().is_empty());
        assert!(
            stopped_marker.is_file(),
            "the restarted attempt's old host process was not stopped"
        );
        let state = controller.rebuild_state().unwrap();
        let task_key = task_key(&report.run_id.0, "task-a");
        assert_eq!(state.tasks[&task_key].entry.attempts, 2);
        assert_eq!(
            state.tasks[&task_key].status,
            FleetTaskLedgerStatus::Completed
        );
        let receipt = &state.receipts[&task_key];
        assert_eq!(receipt.attempt, Some(2));
        assert!(receipt.terminal_seq.is_some());
        assert_eq!(receipt.result, FleetTaskResult::Partial);
    }

    #[test]
    fn fleet_manager_restart_and_stop_all_are_ledgered() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a"), task("task-b")]);
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = &report.worker_ids[0];

        manager.interrupt_worker(worker_id).unwrap();
        let restart = manager.restart_worker(worker_id).unwrap();
        assert_eq!(restart.run_id, report.run_id);
        assert_eq!(restart.max_workers, 1);
        assert_eq!(restart.inspection.status, FleetWorkerStatus::Busy);
        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.running, 1);
        assert_eq!(status.queued, 1);

        let stopped = manager.stop_all().unwrap();
        assert_eq!(stopped, 2);
        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.cancelled, 2);
        assert_eq!(status.running, 0);
    }

    #[cfg(unix)]
    #[test]
    fn standalone_restart_drives_replacement_attempt_to_terminal_receipt() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let marker = tmp.path().join("replacement-attempt-ran");
        let fake = fake_codewhale(
            &tmp,
            &format!(
                r#"#!/bin/sh
touch '{}'
printf '{{"type":"content","content":"replacement attempt"}}\n'
exit 0
"#,
                marker.display()
            ),
        );
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = &report.worker_ids[0];

        manager.interrupt_worker(worker_id).unwrap();
        let restart = manager.restart_worker(worker_id).unwrap();
        let mut executor = FleetExecutor::new(&manager.workspace);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let status = rt
            .block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(5),
                    manager.run_to_completion(
                        &restart.run_id,
                        restart.max_workers,
                        &mut executor,
                        &fake.display().to_string(),
                        None,
                        Duration::from_millis(10),
                    ),
                )
                .await
            })
            .expect("standalone restart left a ghost running task")
            .unwrap();

        assert!(
            marker.is_file(),
            "replacement worker process never launched"
        );
        assert_eq!(status.completed, 1);
        assert_eq!(status.running, 0);
        assert_eq!(status.restarted, 1);
        let state = manager.rebuild_state().unwrap();
        let key = task_key(&report.run_id.0, "task-a");
        assert_eq!(state.tasks[&key].entry.attempts, 2);
        assert_eq!(state.tasks[&key].status, FleetTaskLedgerStatus::Completed);
        assert_eq!(state.receipts[&key].attempt, Some(2));
        assert!(state.receipts[&key].terminal_seq.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_manager_loops_launch_each_attempt_once() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let standby = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let starts = tmp.path().join("worker-starts");
        let fake = fake_codewhale(
            &tmp,
            &format!(
                r#"#!/bin/sh
printf 'started\n' >> '{}'
sleep 1
printf '{{"type":"content","content":"done"}}\n'
exit 0
"#,
                starts.display()
            ),
        );
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let mut primary_executor = FleetExecutor::new(&manager.workspace);
        let mut standby_executor = FleetExecutor::new(&manager.workspace);
        let binary = fake.display().to_string();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let (primary_status, standby_status) = rt
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(5), async {
                    tokio::join!(
                        manager.run_to_completion(
                            &report.run_id,
                            1,
                            &mut primary_executor,
                            &binary,
                            None,
                            Duration::from_millis(10),
                        ),
                        standby.run_to_completion(
                            &report.run_id,
                            1,
                            &mut standby_executor,
                            &binary,
                            None,
                            Duration::from_millis(10),
                        )
                    )
                })
                .await
            })
            .expect("competing Fleet managers did not converge");

        assert_eq!(primary_status.unwrap().completed, 1);
        assert_eq!(standby_status.unwrap().completed, 1);
        let starts = std::fs::read_to_string(starts).unwrap();
        assert_eq!(
            starts.lines().count(),
            1,
            "competing managers launched the same leased attempt more than once"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fleet_manager_can_record_completed_local_smoke_tasks() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a"), task("task-b"), task("task-c")]);
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
printf '{"type":"tool_use","name":"read_file","id":"fake","input":{}}\n'
printf '{"type":"done"}\n'
exit 0
"#,
        );

        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();

        assert_eq!(report.leased, 1);
        assert_eq!(report.queued, 2);
        let status = complete_with_fake_codewhale(&manager, &report.run_id, 1, &fake);
        assert_eq!(status.completed, 3);
        assert_eq!(status.running, 0);
        let state = manager.ledger.rebuild_state().unwrap();
        assert_eq!(state.receipts.len(), 3);
    }

    #[test]
    fn fleet_task_spec_sample_launches_independent_worker_tasks() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(
            &tmp,
            vec![
                task("release-triage"),
                task("risk-review"),
                task("docs-check"),
            ],
        );

        let report = manager.create_run_from_task_spec_path(&path, 2).unwrap();

        assert_eq!(report.task_count, 3);
        assert_eq!(report.leased, 2);
        assert_eq!(report.queued, 1);
        assert_ne!(report.worker_ids[0], report.worker_ids[1]);
        let state = manager.ledger.rebuild_state().unwrap();
        assert!(
            state
                .tasks
                .contains_key(&format!("{}:release-triage", report.run_id.0))
        );
        assert!(
            state
                .tasks
                .contains_key(&format!("{}:risk-review", report.run_id.0))
        );
        assert!(
            state
                .tasks
                .contains_key(&format!("{}:docs-check", report.run_id.0))
        );
    }

    #[cfg(unix)]
    #[test]
    fn fleet_task_spec_local_scorer_records_receipt_artifact() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let mut completed = task("task-a");
        completed.scorer = Some(FleetScorerSpec::ExitCode);
        let path = task_spec_file(&tmp, vec![completed]);
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
printf '{"type":"done"}\n'
exit 0
"#,
        );

        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let status = complete_with_fake_codewhale(&manager, &report.run_id, 1, &fake);

        assert_eq!(status.completed, 1);
        assert_eq!(status.failed, 0);
        assert_eq!(status.partial, 0);
        let state = manager.ledger.rebuild_state().unwrap();
        let receipt = &state.receipts[&format!("{}:task-a", report.run_id.0)];
        assert_eq!(receipt.result, FleetTaskResult::Pass);
        assert_eq!(receipt.failure_kind, None);
        assert_eq!(
            receipt.resolved_route, None,
            "a headless worker with no terminal route report must fail closed"
        );
        assert!(receipt.score.as_ref().unwrap().value > 0.99);
        assert!(
            receipt
                .artifacts
                .iter()
                .any(|artifact| matches!(artifact.kind, FleetArtifactKind::Receipt))
        );
    }

    #[cfg(unix)]
    #[test]
    fn fleet_task_spec_unscored_zero_exit_records_partial_receipt() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
printf '{"type":"done"}\n'
exit 0
"#,
        );

        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = report.worker_ids[0].clone();
        let status = complete_with_fake_codewhale(&manager, &report.run_id, 1, &fake);

        assert_eq!(status.completed, 1);
        assert_eq!(status.partial, 1);
        assert_eq!(status.failed, 0);
        let state = manager.ledger.rebuild_state().unwrap();
        let receipt = &state.receipts[&format!("{}:task-a", report.run_id.0)];
        assert_eq!(receipt.result, FleetTaskResult::Partial);
        assert_eq!(receipt.failure_kind, None);
        assert!(
            receipt
                .score
                .as_ref()
                .and_then(|score| score.notes.as_deref())
                .unwrap_or_default()
                .contains("no verifiable output")
        );
        assert!(
            receipt
                .artifacts
                .iter()
                .any(|artifact| matches!(artifact.kind, FleetArtifactKind::Receipt))
        );
        let inspection = manager.inspect_worker(&worker_id).unwrap();
        let summary = inspection.receipt_summary.as_deref().unwrap_or_default();
        assert!(summary.contains("result=partial"));
        assert!(summary.contains("no verifiable output"));
    }

    #[cfg(unix)]
    #[test]
    fn fleet_task_spec_unscored_worker_error_records_failed_receipt() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
printf '{"type":"error","error":"tool failed"}\n'
exit 7
"#,
        );

        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let status = complete_with_fake_codewhale(&manager, &report.run_id, 1, &fake);

        assert_eq!(status.completed, 0);
        assert_eq!(status.partial, 0);
        assert_eq!(status.failed, 1);
        assert_eq!(status.task_failed, 1);
        let state = manager.ledger.rebuild_state().unwrap();
        let receipt = &state.receipts[&format!("{}:task-a", report.run_id.0)];
        assert_eq!(receipt.result, FleetTaskResult::Fail);
        assert_eq!(receipt.failure_kind, Some(FleetTaskFailureKind::Task));
    }

    #[cfg(unix)]
    #[test]
    fn fleet_task_spec_status_distinguishes_failure_sources() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let mut task_failed = task("a-task-failure");
        task_failed.scorer = Some(FleetScorerSpec::ExitCode);
        task_failed.instructions = "task-failure".to_string();
        let mut transport = task("b-transport-failure");
        transport.scorer = Some(FleetScorerSpec::ExitCode);
        let mut verifier_failed = task("c-verifier-failure");
        verifier_failed.scorer = Some(FleetScorerSpec::RegexMatch {
            path: PathBuf::from("missing.log"),
            pattern: "[".to_string(),
        });
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
case "$*" in
  *task-failure*)
    printf '{"type":"error","error":"task failed"}\n'
    exit 7
    ;;
  *)
    printf '{"type":"done"}\n'
    exit 0
    ;;
esac
"#,
        );
        let doc = FleetTaskSpecDocument {
            name: Some("failure source smoke".to_string()),
            labels: BTreeMap::new(),
            security_policy: None,
            workers: vec![
                default_local_worker("local-task"),
                FleetWorkerSpec {
                    id: "docker-transport".to_string(),
                    name: "Docker transport".to_string(),
                    host: FleetHostSpec::Docker {
                        image: "fake".to_string(),
                        args: Vec::new(),
                    },
                    trust_level: Some(FleetTrustLevel::Sandbox),
                    labels: BTreeMap::new(),
                    capabilities: vec![],
                    max_concurrent_tasks: Some(1),
                },
                default_local_worker("local-verifier"),
            ],
            tasks: vec![task_failed, transport, verifier_failed],
        };

        let report = manager.create_run(doc, 3).unwrap();
        let status = complete_with_fake_codewhale(&manager, &report.run_id, 3, &fake);

        assert_eq!(status.failed, 3);
        assert_eq!(status.transport_failed, 1);
        assert_eq!(status.task_failed, 1);
        assert_eq!(status.verifier_failed, 1);
        assert_eq!(status.running, 0);
    }

    #[cfg(unix)]
    #[test]
    fn remote_terminal_route_x_wins_manager_config_y_and_receipt_is_secret_free() {
        let tmp = TempDir::new().unwrap();
        let manager_config = Config {
            provider: Some("manager-y".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom: std::collections::HashMap::from([(
                    "manager-y".to_string(),
                    crate::config::ProviderConfig {
                        kind: Some("openai-compatible".to_string()),
                        base_url: Some("https://manager-y.invalid/v1".to_string()),
                        model: Some("manager-model-y".to_string()),
                        api_key: Some("sk-manager-y-must-not-leak".to_string()),
                        ..Default::default()
                    },
                )]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_session_model("manager-model-y")
            .with_route_config(manager_config);
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
printf '%s\n' '{"type":"metadata","meta":{"receipt_kind":"terminal","provider":"custom","provider_id":"remote-x","model":"worker-model-x","base_url":"https://remote-x.invalid/v1","api_key":"sk-remote-x-must-not-leak"}}'
printf '%s\n' '{"type":"done"}'
"#,
        );
        let report = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("remote config drift".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: None,
                    workers: vec![],
                    tasks: vec![task("route-drift")],
                },
                1,
            )
            .unwrap();

        let status = complete_with_fake_codewhale(&manager, &report.run_id, 1, &fake);
        assert_eq!(status.completed, 1);
        let state = manager.ledger.rebuild_state().unwrap();
        let receipt = &state.receipts[&format!("{}:route-drift", report.run_id.0)];
        let route = receipt
            .resolved_route
            .as_ref()
            .expect("terminal-reported route receipt");
        assert_eq!(route.provider_id, "remote-x");
        assert_eq!(route.provider_exact_id.as_deref(), Some("remote-x"));
        assert_eq!(route.provider_kind, "custom");
        assert_eq!(route.wire_model_id, "worker-model-x");
        assert_eq!(route.canonical_model, None);
        assert_eq!(route.protocol, "unreported");
        assert_eq!(route.source, "worker_terminal_metadata");

        let output = serde_json::to_string(receipt).unwrap().to_ascii_lowercase();
        for forbidden in [
            "manager-y",
            "manager-model-y",
            "base_url",
            "https://",
            "api_key",
            "sk-manager-y-must-not-leak",
            "sk-remote-x-must-not-leak",
        ] {
            assert!(
                !output.contains(forbidden),
                "receipt leaked manager drift or secret field {forbidden:?}: {output}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn headless_terminal_with_invalid_route_metadata_does_not_fall_back_to_manager_config() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path())
            .unwrap()
            .with_session_model("manager-model-y")
            .with_route_config(Config {
                provider: Some("manager-y".to_string()),
                providers: Some(crate::config::ProvidersConfig {
                    custom: std::collections::HashMap::from([(
                        "manager-y".to_string(),
                        crate::config::ProviderConfig {
                            kind: Some("openai-compatible".to_string()),
                            base_url: Some("https://manager-y.invalid/v1".to_string()),
                            model: Some("manager-model-y".to_string()),
                            api_key: Some("sk-manager-y-must-not-leak".to_string()),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                }),
                ..Default::default()
            });
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
printf '%s\n' '{"type":"metadata","meta":{"receipt_kind":"terminal","provider":"deepseek","provider_id":"custom-x","model":"deepseek-v4-pro"}}'
printf '%s\n' '{"type":"done"}'
"#,
        );
        let report = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("invalid terminal route".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: None,
                    workers: vec![],
                    tasks: vec![task("invalid-route")],
                },
                1,
            )
            .unwrap();

        let status = complete_with_fake_codewhale(&manager, &report.run_id, 1, &fake);
        assert_eq!(status.completed, 1);
        let state = manager.ledger.rebuild_state().unwrap();
        let receipt = &state.receipts[&format!("{}:invalid-route", report.run_id.0)];
        assert_eq!(receipt.resolved_route, None);
        let output = serde_json::to_string(receipt).unwrap().to_ascii_lowercase();
        assert!(!output.contains("manager-y"));
        assert!(!output.contains("manager-model-y"));
        assert!(!output.contains("sk-manager-y-must-not-leak"));
    }

    #[cfg(unix)]
    #[test]
    fn fleet_smoke_runs_three_roles_ten_tasks_with_receipts_and_failure() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let fake = fake_codewhale(
            &tmp,
            r#"#!/bin/sh
case "$*" in
  *intentional-failure*)
    printf '{"type":"tool_use","name":"exec_shell","id":"fail","input":{}}\n'
    printf '{"type":"metadata","meta":{"receipt_kind":"terminal","provider":"deepseek","model":"deepseek-v4-pro"}}\n'
    printf '{"type":"error","error":"intentional failure"}\n'
    exit 7
    ;;
  *)
    printf '{"type":"tool_use","name":"read_file","id":"ok","input":{}}\n'
    printf '{"type":"content","delta":"ok"}\n'
    printf '{"type":"metadata","meta":{"receipt_kind":"terminal","provider":"deepseek","model":"deepseek-v4-pro"}}\n'
    printf '{"type":"done"}\n'
    exit 0
    ;;
esac
"#,
        );
        let smoke_task = |id: &str, role: &str, tools: Vec<&str>, marker: &str| {
            let mut task = task(id);
            task.name = format!("{role} {id}");
            task.objective = Some(format!("{role} smoke task {id}"));
            task.instructions = format!("run deterministic fleet smoke lane {marker}");
            task.worker = Some(FleetTaskWorkerProfile {
                role: Some(role.to_string()),
                agent_profile: None,
                loadout: None,
                model_class: None,
                model: None,
                tool_profile: Some("explicit".to_string()),
                tools: tools.into_iter().map(str::to_string).collect(),
                capabilities: vec!["local-smoke".to_string()],
            });
            task.expected_artifacts = vec![FleetArtifactKind::Log, FleetArtifactKind::Receipt];
            task.scorer = Some(FleetScorerSpec::ExitCode);
            task.retry_policy = Some(FleetRetryPolicy {
                max_attempts: 1,
                ..Default::default()
            });
            task
        };
        let tasks = vec![
            smoke_task("scout-1", "scout", vec!["read_file", "grep_files"], "ok"),
            smoke_task(
                "builder-1",
                "builder",
                vec!["read_file", "apply_patch"],
                "ok",
            ),
            smoke_task(
                "verifier-1",
                "verifier",
                vec!["exec_shell", "read_file"],
                "ok",
            ),
            smoke_task("scout-2", "scout", vec!["read_file", "grep_files"], "ok"),
            smoke_task(
                "builder-2",
                "builder",
                vec!["read_file", "apply_patch"],
                "ok",
            ),
            smoke_task(
                "verifier-2",
                "verifier",
                vec!["exec_shell", "read_file"],
                "ok",
            ),
            smoke_task("scout-3", "scout", vec!["read_file", "grep_files"], "ok"),
            smoke_task(
                "builder-3",
                "builder",
                vec!["read_file", "apply_patch"],
                "ok",
            ),
            smoke_task(
                "verifier-3",
                "verifier",
                vec!["exec_shell", "read_file"],
                "ok",
            ),
            smoke_task(
                "verifier-4-fail",
                "verifier",
                vec!["exec_shell", "read_file"],
                "intentional-failure",
            ),
        ];

        let report = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("fleet route parity smoke".to_string()),
                    labels: BTreeMap::from([("issue".to_string(), "3166".to_string())]),
                    security_policy: Some(FleetSecurityPolicy {
                        default_trust_level: FleetTrustLevel::Local,
                        ..Default::default()
                    }),
                    workers: vec![],
                    tasks,
                },
                3,
            )
            .unwrap();

        assert_eq!(report.task_count, 10);
        assert_eq!(report.worker_ids.len(), 3);
        assert_eq!(report.leased, 3);
        assert_eq!(report.queued, 7);

        let status = complete_with_fake_codewhale(&manager, &report.run_id, 3, &fake);
        assert_eq!(status.completed, 9);
        assert_eq!(status.failed, 1);
        assert_eq!(status.task_failed, 1);
        assert_eq!(status.partial, 0);
        assert_eq!(status.running, 0);
        assert_eq!(status.queued, 0);

        let state = manager.ledger.rebuild_state().unwrap();
        let run = &state.runs[&report.run_id.0];
        let roles = run
            .task_specs
            .iter()
            .filter_map(|task| task.worker.as_ref()?.role.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            roles,
            BTreeSet::from([
                "builder".to_string(),
                "scout".to_string(),
                "verifier".to_string()
            ])
        );
        assert_eq!(state.receipts.len(), 10);

        // #3166 scope #10: every receipt persists a resolved-route snapshot
        // (#3154) with non-empty provider/wire-model, a role, and the resolver
        // source — and the serialized receipt leaks no credential material.
        for (key, receipt) in &state.receipts {
            let route = receipt
                .resolved_route
                .as_ref()
                .unwrap_or_else(|| panic!("receipt {key} should carry a resolved route"));
            assert!(
                !route.provider_id.is_empty(),
                "receipt {key} resolved-route provider_id must be non-empty"
            );
            assert!(
                !route.wire_model_id.is_empty(),
                "receipt {key} resolved-route wire_model_id must be non-empty"
            );
            assert!(
                route.role.as_deref().is_some_and(|role| !role.is_empty()),
                "receipt {key} resolved-route should record a role"
            );
            assert_eq!(
                route.source, "worker_terminal_metadata",
                "receipt {key} resolved-route source must be the worker terminal"
            );
            assert!(
                route
                    .model_route
                    .as_deref()
                    .is_some_and(|route| !route.is_empty()),
                "receipt {key} resolved-route should record the model route seam"
            );
            assert!(
                route
                    .role_source
                    .as_deref()
                    .is_some_and(|source| !source.is_empty()),
                "receipt {key} resolved-route should record role source"
            );
            assert!(
                route
                    .model_source
                    .as_deref()
                    .is_some_and(|source| !source.is_empty()),
                "receipt {key} resolved-route should record model source"
            );
            let permissions = receipt
                .effective_permissions
                .as_ref()
                .unwrap_or_else(|| panic!("receipt {key} should carry effective permissions"));
            assert_eq!(
                permissions.source, "worker_runtime_profile",
                "receipt {key} permissions source must be the worker runtime profile"
            );
            assert!(
                permissions.background,
                "receipt {key} should record background worker execution"
            );
            assert_eq!(
                permissions.tool_scope, "explicit",
                "receipt {key} should preserve explicit tool scope"
            );
            assert!(
                !permissions.tools.is_empty(),
                "receipt {key} should record explicit tool names"
            );
            match route.role.as_deref() {
                Some("builder") => {
                    assert!(permissions.write, "builder receipt {key} should write");
                    assert_eq!(permissions.shell, "full");
                }
                Some("scout") => {
                    assert!(
                        !permissions.write,
                        "scout receipt {key} must stay read-only"
                    );
                    assert_eq!(permissions.shell, "read_only");
                }
                Some("verifier") => {
                    assert!(
                        !permissions.write,
                        "verifier receipt {key} must stay read-only"
                    );
                    assert_eq!(permissions.shell, "full");
                }
                role => panic!("unexpected receipt role for {key}: {role:?}"),
            }

            let receipt_json = serde_json::to_string(receipt).unwrap();
            let haystack = receipt_json.to_ascii_lowercase();
            for needle in [
                "api_key",
                "apikey",
                "api-key",
                "authorization",
                "bearer ",
                "auth_token",
                "auth-token",
                "password",
                "credential",
                "sk-ant-",
                "sk-proj-",
                "sk-or-",
                "secret",
            ] {
                assert!(
                    !haystack.contains(needle),
                    "receipt {key} JSON must not contain secret marker {needle:?}: {receipt_json}"
                );
            }
        }

        let failed_receipt = &state.receipts[&format!("{}:verifier-4-fail", report.run_id.0)];
        assert_eq!(failed_receipt.result, FleetTaskResult::Fail);
        assert_eq!(
            failed_receipt.failure_kind,
            Some(FleetTaskFailureKind::Task)
        );
        assert!(failed_receipt.artifacts.iter().any(|artifact| {
            matches!(artifact.kind, FleetArtifactKind::Log)
                && artifact.mime_type.as_deref() == Some("application/x-ndjson")
                && artifact.size_bytes.unwrap_or_default() > 0
        }));
        assert!(
            failed_receipt
                .artifacts
                .iter()
                .any(|artifact| matches!(artifact.kind, FleetArtifactKind::Receipt))
        );

        for worker_id in &report.worker_ids {
            let inspection = manager.inspect_worker(worker_id).unwrap();
            assert_eq!(inspection.status, FleetWorkerStatus::Online);
            assert!(inspection.latest_heartbeat_at.is_some());
            assert!(
                inspection.receipt_summary.is_some(),
                "{worker_id} should expose latest receipt summary"
            );
            assert!(
                inspection.artifacts.iter().any(|artifact| matches!(
                    artifact.kind,
                    FleetArtifactKind::Log | FleetArtifactKind::Receipt
                )),
                "{worker_id} should expose artifact refs"
            );
        }
    }

    #[test]
    fn fleet_status_counts_restarted_and_escalated_events() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let path = task_spec_file(&tmp, vec![task("task-a")]);
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = &report.worker_ids[0];

        manager.restart_worker(worker_id).unwrap();
        manager
            .append_worker_event(
                &report.run_id,
                worker_id,
                "task-a",
                FleetWorkerEventPayload::Escalated {
                    channel: "slack".to_string(),
                    alert_id: None,
                },
            )
            .unwrap();

        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.restarted, 1);
        assert_eq!(status.escalated, 1);

        manager.ledger.compact().unwrap();
        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.restarted, 1);
        assert_eq!(status.escalated, 1);
    }

    #[test]
    fn fleet_status_inspect_exposes_task_context_host_and_alert() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        let mut contextual = task("task-a");
        contextual.objective = Some("Review the release ledger".to_string());
        contextual.worker = Some(FleetTaskWorkerProfile {
            agent_profile: None,
            role: Some("release-reviewer".to_string()),
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: Some("read-only".to_string()),
            tools: vec!["git".to_string()],
            capabilities: vec!["rust".to_string()],
        });
        let path = task_spec_file(&tmp, vec![contextual]);
        let report = manager.create_run_from_task_spec_path(&path, 1).unwrap();
        let worker_id = &report.worker_ids[0];
        manager
            .append_worker_event(
                &report.run_id,
                worker_id,
                "task-a",
                FleetWorkerEventPayload::Escalated {
                    channel: "pagerduty".to_string(),
                    alert_id: Some("alert-1".to_string()),
                },
            )
            .unwrap();

        let inspection = manager.inspect_worker(worker_id).unwrap();

        assert_eq!(
            inspection.objective.as_deref(),
            Some("Review the release ledger")
        );
        assert_eq!(inspection.role.as_deref(), Some("release-reviewer"));
        assert_eq!(inspection.host.as_deref(), Some("local"));
        assert_eq!(
            inspection.alert_state.as_deref(),
            Some("escalated via pagerduty alert_id=alert-1")
        );
    }

    #[test]
    fn fleet_dogfood_smoke_run_two_local_workers_two_tasks() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("repo");
        std::fs::create_dir_all(&workspace).unwrap();
        // Create a minimal Cargo.toml so the cargo-check task can succeed.
        std::fs::write(
            workspace.join("Cargo.toml"),
            "[package]\nname = \"smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(
            workspace.join("src").join("lib.rs"),
            "pub fn answer() -> u8 { 42 }\n",
        )
        .unwrap();

        let tasks = vec![
            FleetTaskSpec {
                id: "check".to_string(),
                name: "check".to_string(),
                description: None,
                objective: Some("cargo check".to_string()),
                instructions: "run cargo check and report result".to_string(),
                worker: Some(FleetTaskWorkerProfile {
                    agent_profile: None,
                    role: Some("release-checker".to_string()),
                    loadout: None,
                    model_class: None,
                    model: None,
                    tool_profile: Some("read-only".to_string()),
                    tools: vec!["cargo".to_string()],
                    capabilities: vec!["rust".to_string()],
                }),
                workspace: Some(FleetWorkspaceRequirements {
                    root: None,
                    required_files: vec![PathBuf::from("Cargo.toml")],
                    writable_paths: vec![PathBuf::from(".codewhale/fleet")],
                    environment: Some(FleetEnvironmentRequirements {
                        required: vec!["PATH".to_string()],
                        allowlist: vec![],
                    }),
                }),
                input_files: vec![],
                context: vec![],
                budget: None,
                tags: vec!["smoke".to_string()],
                expected_artifacts: vec![FleetArtifactKind::Log, FleetArtifactKind::Receipt],
                scorer: Some(FleetScorerSpec::ExitCode),
                retry_policy: Some(FleetRetryPolicy {
                    max_attempts: 1,
                    ..Default::default()
                }),
                alert_policy: None,
                timeout_seconds: Some(60),
                metadata: BTreeMap::new(),
            },
            FleetTaskSpec {
                id: "review".to_string(),
                name: "review".to_string(),
                description: None,
                objective: Some("review source".to_string()),
                instructions: "read src/lib.rs and report findings".to_string(),
                worker: Some(FleetTaskWorkerProfile {
                    agent_profile: None,
                    role: Some("reviewer".to_string()),
                    loadout: None,
                    model_class: None,
                    model: None,
                    tool_profile: Some("read-only".to_string()),
                    tools: vec!["cargo".to_string()],
                    capabilities: vec!["rust".to_string()],
                }),
                workspace: Some(FleetWorkspaceRequirements {
                    root: None,
                    required_files: vec![],
                    writable_paths: vec![],
                    environment: Some(FleetEnvironmentRequirements {
                        required: vec!["PATH".to_string()],
                        allowlist: vec![],
                    }),
                }),
                input_files: vec![],
                context: vec![],
                budget: None,
                tags: vec!["smoke".to_string()],
                expected_artifacts: vec![FleetArtifactKind::Log, FleetArtifactKind::Receipt],
                scorer: None,
                retry_policy: Some(FleetRetryPolicy {
                    max_attempts: 1,
                    ..Default::default()
                }),
                alert_policy: None,
                timeout_seconds: Some(60),
                metadata: BTreeMap::new(),
            },
        ];

        let manager = FleetManager::open(&workspace).unwrap();
        let report = manager
            .create_run(
                FleetTaskSpecDocument {
                    name: Some("dogfood smoke".to_string()),
                    labels: BTreeMap::new(),
                    security_policy: Some(FleetSecurityPolicy {
                        default_trust_level: FleetTrustLevel::Local,
                        ..Default::default()
                    }),
                    workers: vec![],
                    tasks,
                },
                2,
            )
            .unwrap();

        assert_eq!(report.task_count, 2);
        assert!(!report.worker_ids.is_empty());
        assert_eq!(report.worker_ids.len(), 2);
        // After immediate scheduling, tasks may already be leased,
        // so queued+running should total 2.
        let status = manager.run_status(&report.run_id).unwrap();
        assert_eq!(status.queued + status.running, 2);
    }

    #[test]
    fn fleet_security_policy_propagates_from_task_spec_document_to_run() {
        let tmp = TempDir::new().unwrap();
        let manager = FleetManager::open(tmp.path()).unwrap();
        // Rewrite the spec file with a security_policy block.
        let doc = serde_json::json!({
            "name": "secure smoke",
            "tasks": [{
                "id": "task-a",
                "name": "task-a",
                "instructions": "report ok",
                "expected_artifacts": ["log"]
            }],
            "security_policy": {
                "default_trust_level": "local",
                "allowed_secrets": [{"key": "GH_TOKEN", "source": "env"}],
                "max_trust_level": "remote_verified",
                "require_identity_verification": true
            }
        });
        let spec_path = tmp.path().join("secure-tasks.json");
        std::fs::write(&spec_path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

        let report = manager
            .create_run_from_task_spec_path(&spec_path, 1)
            .unwrap();

        let state = manager.ledger.rebuild_state().unwrap();
        let run = state.runs.get(&report.run_id.0).unwrap();
        let policy = run.security_policy.as_ref().unwrap();
        assert_eq!(policy.default_trust_level, FleetTrustLevel::Local);
        assert_eq!(policy.allowed_secrets.len(), 1);
        assert_eq!(policy.allowed_secrets[0].key, "GH_TOKEN");
        assert_eq!(policy.max_trust_level, FleetTrustLevel::RemoteVerified);
        assert!(policy.require_identity_verification);
    }
}
