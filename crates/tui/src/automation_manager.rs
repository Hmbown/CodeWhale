//! Durable automation records and scheduler support.
//!
//! Automations are local-first recurring jobs that enqueue standard background
//! tasks. This module stores automation definitions and run history under
//! `~/.codewhale/automations` (or `DEEPSEEK_AUTOMATIONS_DIR` override).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::task_manager::{NewTaskRequest, SharedTaskManager, TaskStatus};
use crate::utils::spawn_supervised;

const CURRENT_AUTOMATION_SCHEMA_VERSION: u32 = 1;
const CURRENT_RUN_SCHEMA_VERSION: u32 = 1;
const DEFAULT_AUTOMATION_MODE: &str = "agent";
const DEFAULT_AUTOMATION_ALLOW_SHELL: bool = false;
const DEFAULT_AUTOMATION_TRUST_MODE: bool = false;
const DEFAULT_AUTOMATION_AUTO_APPROVE: bool = true;

const fn default_automation_schema_version() -> u32 {
    CURRENT_AUTOMATION_SCHEMA_VERSION
}

const fn default_run_schema_version() -> u32 {
    CURRENT_RUN_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStatus {
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRecord {
    #[serde(default = "default_automation_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub rrule: String,
    #[serde(default)]
    pub cwds: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_shell: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_approve: Option<bool>,
    pub status: AutomationStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
}

impl AutomationRecord {
    fn task_mode(&self) -> String {
        self.mode
            .as_deref()
            .map(str::trim)
            .filter(|mode| !mode.is_empty())
            .unwrap_or(DEFAULT_AUTOMATION_MODE)
            .to_string()
    }

    fn task_allow_shell(&self) -> bool {
        self.allow_shell.unwrap_or(DEFAULT_AUTOMATION_ALLOW_SHELL)
    }

    fn task_trust_mode(&self) -> bool {
        self.trust_mode.unwrap_or(DEFAULT_AUTOMATION_TRUST_MODE)
    }

    fn task_auto_approve(&self) -> bool {
        self.auto_approve.unwrap_or(DEFAULT_AUTOMATION_AUTO_APPROVE)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRunRecord {
    #[serde(default = "default_run_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub automation_id: String,
    pub scheduled_for: DateTime<Utc>,
    pub status: AutomationRunStatus,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAutomationRequest {
    pub name: String,
    pub prompt: String,
    pub rrule: String,
    #[serde(default)]
    pub cwds: Vec<PathBuf>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub allow_shell: Option<bool>,
    #[serde(default)]
    pub trust_mode: Option<bool>,
    #[serde(default)]
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub status: Option<AutomationStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateAutomationRequest {
    pub name: Option<String>,
    pub prompt: Option<String>,
    pub rrule: Option<String>,
    pub cwds: Option<Vec<PathBuf>>,
    pub mode: Option<String>,
    pub allow_shell: Option<bool>,
    pub trust_mode: Option<bool>,
    pub auto_approve: Option<bool>,
    pub status: Option<AutomationStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomationFrequency {
    Hourly,
    Weekly,
}

#[derive(Debug, Clone)]
pub enum AutomationSchedule {
    Hourly {
        interval_hours: u32,
        byday: Option<Vec<Weekday>>,
    },
    Weekly {
        byday: Vec<Weekday>,
        byhour: u32,
        byminute: u32,
    },
}

impl AutomationSchedule {
    pub fn parse_rrule(rrule: &str) -> Result<Self> {
        let mut parts: BTreeMap<String, String> = BTreeMap::new();
        for raw in rrule.split(';') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            let Some((k, v)) = item.split_once('=') else {
                bail!("Invalid RRULE segment '{item}'");
            };
            parts.insert(k.trim().to_ascii_uppercase(), v.trim().to_ascii_uppercase());
        }

        let freq = match parts.get("FREQ").map(String::as_str) {
            Some("HOURLY") => AutomationFrequency::Hourly,
            Some("WEEKLY") => AutomationFrequency::Weekly,
            Some(other) => bail!("Unsupported RRULE FREQ '{other}'. Supported: HOURLY and WEEKLY"),
            None => bail!("RRULE must include FREQ"),
        };

        match freq {
            AutomationFrequency::Hourly => {
                for key in parts.keys() {
                    if key != "FREQ" && key != "INTERVAL" && key != "BYDAY" {
                        bail!(
                            "Unsupported RRULE field '{key}' for HOURLY. Allowed: FREQ,INTERVAL,BYDAY"
                        );
                    }
                }
                let interval_hours = parts
                    .get("INTERVAL")
                    .map(|v| v.parse::<u32>())
                    .transpose()
                    .context("Failed to parse INTERVAL")?
                    .unwrap_or(1);
                if interval_hours == 0 {
                    bail!("INTERVAL must be >= 1 for HOURLY schedules");
                }
                let byday = parts
                    .get("BYDAY")
                    .map(|value| parse_byday(value))
                    .transpose()?;
                Ok(Self::Hourly {
                    interval_hours,
                    byday,
                })
            }
            AutomationFrequency::Weekly => {
                for key in parts.keys() {
                    if key != "FREQ" && key != "BYDAY" && key != "BYHOUR" && key != "BYMINUTE" {
                        bail!(
                            "Unsupported RRULE field '{key}' for WEEKLY. Allowed: FREQ,BYDAY,BYHOUR,BYMINUTE"
                        );
                    }
                }
                let byday_raw = parts
                    .get("BYDAY")
                    .ok_or_else(|| anyhow::anyhow!("WEEKLY schedules require BYDAY"))?;
                let byday = parse_byday(byday_raw)?;
                if byday.is_empty() {
                    bail!("BYDAY cannot be empty for WEEKLY schedules");
                }
                let byhour = parts
                    .get("BYHOUR")
                    .ok_or_else(|| anyhow::anyhow!("WEEKLY schedules require BYHOUR"))?
                    .parse::<u32>()
                    .context("Failed to parse BYHOUR")?;
                let byminute = parts
                    .get("BYMINUTE")
                    .ok_or_else(|| anyhow::anyhow!("WEEKLY schedules require BYMINUTE"))?
                    .parse::<u32>()
                    .context("Failed to parse BYMINUTE")?;

                if byhour > 23 {
                    bail!("BYHOUR must be between 0 and 23");
                }
                if byminute > 59 {
                    bail!("BYMINUTE must be between 0 and 59");
                }

                Ok(Self::Weekly {
                    byday,
                    byhour,
                    byminute,
                })
            }
        }
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Result<DateTime<Utc>> {
        let local_after = after.with_timezone(&Local);
        match self {
            Self::Hourly {
                interval_hours,
                byday,
            } => {
                let mut candidate = local_after + Duration::hours(i64::from(*interval_hours))
                    - Duration::seconds(i64::from(local_after.second()))
                    - Duration::nanoseconds(i64::from(local_after.nanosecond()));

                if let Some(days) = byday {
                    for _ in 0..(24 * 21) {
                        if days.contains(&candidate.weekday()) {
                            return Ok(candidate.with_timezone(&Utc));
                        }
                        candidate += Duration::hours(i64::from(*interval_hours));
                    }
                    bail!("Unable to compute next HOURLY run for BYDAY filter");
                }

                Ok(candidate.with_timezone(&Utc))
            }
            Self::Weekly {
                byday,
                byhour,
                byminute,
            } => {
                for day_offset in 0..15 {
                    let date = local_after.date_naive() + Duration::days(i64::from(day_offset));
                    if !byday.contains(&date.weekday()) {
                        continue;
                    }
                    let Some(candidate_naive) = date.and_hms_opt(*byhour, *byminute, 0) else {
                        continue;
                    };
                    if let Some(candidate) = resolve_local_datetime(candidate_naive)
                        && candidate > local_after
                    {
                        return Ok(candidate.with_timezone(&Utc));
                    }
                }
                bail!("Unable to compute next WEEKLY run");
            }
        }
    }
}

fn resolve_local_datetime(naive: chrono::NaiveDateTime) -> Option<DateTime<Local>> {
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .or_else(|| Local.from_local_datetime(&naive).latest())
}

fn parse_byday(value: &str) -> Result<Vec<Weekday>> {
    let mut days = Vec::new();
    for token in value.split(',') {
        let day = match token.trim().to_ascii_uppercase().as_str() {
            "MO" => Weekday::Mon,
            "TU" => Weekday::Tue,
            "WE" => Weekday::Wed,
            "TH" => Weekday::Thu,
            "FR" => Weekday::Fri,
            "SA" => Weekday::Sat,
            "SU" => Weekday::Sun,
            other => bail!("Invalid BYDAY value '{other}'"),
        };
        if !days.contains(&day) {
            days.push(day);
        }
    }
    Ok(days)
}

#[derive(Debug, Clone)]
pub struct AutomationManager {
    automations_dir: PathBuf,
    runs_dir: PathBuf,
}

impl AutomationManager {
    pub fn open(root: PathBuf) -> Result<Self> {
        let automations_dir = root.join("automations");
        let runs_dir = root.join("runs");
        fs::create_dir_all(&automations_dir)
            .with_context(|| format!("Failed to create {}", automations_dir.display()))?;
        fs::create_dir_all(&runs_dir)
            .with_context(|| format!("Failed to create {}", runs_dir.display()))?;
        Ok(Self {
            automations_dir,
            runs_dir,
        })
    }

    pub fn default_location() -> Result<Self> {
        Self::open(default_automations_dir())
    }

    fn automation_path(&self, id: &str) -> Result<PathBuf> {
        ensure_safe_storage_id("automation id", id)?;
        Ok(self.automations_dir.join(format!("{id}.json")))
    }

    fn runs_dir_for(&self, automation_id: &str) -> Result<PathBuf> {
        ensure_safe_storage_id("automation id", automation_id)?;
        Ok(self.runs_dir.join(automation_id))
    }

    fn run_path(&self, automation_id: &str, run_id: &str) -> Result<PathBuf> {
        ensure_safe_storage_id("run id", run_id)?;
        Ok(self
            .runs_dir_for(automation_id)?
            .join(format!("{run_id}.json")))
    }

    pub fn create_automation(&self, req: CreateAutomationRequest) -> Result<AutomationRecord> {
        validate_name_and_prompt(&req.name, &req.prompt)?;
        let schedule = AutomationSchedule::parse_rrule(&req.rrule)?;
        let now = Utc::now();
        let status = req.status.unwrap_or(AutomationStatus::Active);
        let next_run_at = if matches!(status, AutomationStatus::Active) {
            Some(schedule.next_after(now)?)
        } else {
            None
        };

        let record = AutomationRecord {
            schema_version: CURRENT_AUTOMATION_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            name: req.name.trim().to_string(),
            prompt: req.prompt.trim().to_string(),
            rrule: req.rrule.trim().to_ascii_uppercase(),
            cwds: req.cwds,
            mode: normalize_optional_string(req.mode),
            allow_shell: req.allow_shell,
            trust_mode: req.trust_mode,
            auto_approve: req.auto_approve,
            status,
            created_at: now,
            updated_at: now,
            next_run_at,
            last_run_at: None,
        };

        self.save_automation(&record)?;
        Ok(record)
    }

    pub fn get_automation(&self, id: &str) -> Result<AutomationRecord> {
        let path = self.automation_path(id)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read automation {}", path.display()))?;
        let record: AutomationRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse automation {}", path.display()))?;
        if record.schema_version > CURRENT_AUTOMATION_SCHEMA_VERSION {
            bail!(
                "Automation schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_AUTOMATION_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn save_automation(&self, record: &AutomationRecord) -> Result<()> {
        write_json_atomic(&self.automation_path(&record.id)?, record)
    }

    pub fn list_automations(&self) -> Result<Vec<AutomationRecord>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.automations_dir)
            .with_context(|| format!("Failed to read {}", self.automations_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let record: AutomationRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if record.schema_version > CURRENT_AUTOMATION_SCHEMA_VERSION {
                bail!(
                    "Automation schema v{} is newer than supported v{}",
                    record.schema_version,
                    CURRENT_AUTOMATION_SCHEMA_VERSION
                );
            }
            out.push(record);
        }
        out.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
        Ok(out)
    }

    pub fn update_automation(
        &self,
        id: &str,
        req: UpdateAutomationRequest,
    ) -> Result<AutomationRecord> {
        let mut existing = self.get_automation(id)?;

        if let Some(name) = req.name {
            if name.trim().is_empty() {
                bail!("Automation name cannot be empty");
            }
            existing.name = name.trim().to_string();
        }
        if let Some(prompt) = req.prompt {
            if prompt.trim().is_empty() {
                bail!("Automation prompt cannot be empty");
            }
            existing.prompt = prompt.trim().to_string();
        }
        if let Some(rrule) = req.rrule {
            let normalized = rrule.trim().to_ascii_uppercase();
            AutomationSchedule::parse_rrule(&normalized)?;
            existing.rrule = normalized;
            if matches!(existing.status, AutomationStatus::Active) {
                let schedule = AutomationSchedule::parse_rrule(&existing.rrule)?;
                existing.next_run_at = Some(schedule.next_after(Utc::now())?);
            }
        }
        if let Some(cwds) = req.cwds {
            existing.cwds = cwds;
        }
        if let Some(mode) = req.mode {
            existing.mode = normalize_optional_string(Some(mode));
        }
        if let Some(allow_shell) = req.allow_shell {
            existing.allow_shell = Some(allow_shell);
        }
        if let Some(trust_mode) = req.trust_mode {
            existing.trust_mode = Some(trust_mode);
        }
        if let Some(auto_approve) = req.auto_approve {
            existing.auto_approve = Some(auto_approve);
        }
        if let Some(status) = req.status {
            existing.status = status;
            if matches!(status, AutomationStatus::Paused) {
                existing.next_run_at = None;
            } else {
                let schedule = AutomationSchedule::parse_rrule(&existing.rrule)?;
                existing.next_run_at = Some(schedule.next_after(Utc::now())?);
            }
        }

        existing.updated_at = Utc::now();
        self.save_automation(&existing)?;
        Ok(existing)
    }

    pub fn pause_automation(&self, id: &str) -> Result<AutomationRecord> {
        self.update_automation(
            id,
            UpdateAutomationRequest {
                status: Some(AutomationStatus::Paused),
                ..UpdateAutomationRequest::default()
            },
        )
    }

    pub fn resume_automation(&self, id: &str) -> Result<AutomationRecord> {
        self.update_automation(
            id,
            UpdateAutomationRequest {
                status: Some(AutomationStatus::Active),
                ..UpdateAutomationRequest::default()
            },
        )
    }

    pub fn delete_automation(&self, id: &str) -> Result<AutomationRecord> {
        let existing = self.get_automation(id)?;
        let path = self.automation_path(id)?;
        fs::remove_file(&path)
            .with_context(|| format!("Failed to delete automation {}", path.display()))?;

        let runs_dir = self.runs_dir_for(id)?;
        if runs_dir.exists() {
            fs::remove_dir_all(&runs_dir).with_context(|| {
                format!("Failed to delete automation runs {}", runs_dir.display())
            })?;
        }

        Ok(existing)
    }

    pub fn list_runs(
        &self,
        automation_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<AutomationRunRecord>> {
        let dir = self.runs_dir_for(automation_id)?;
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        for entry in
            fs::read_dir(&dir).with_context(|| format!("Failed to read {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let run: AutomationRunRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if run.schema_version > CURRENT_RUN_SCHEMA_VERSION {
                bail!(
                    "Automation run schema v{} is newer than supported v{}",
                    run.schema_version,
                    CURRENT_RUN_SCHEMA_VERSION
                );
            }
            out.push(run);
        }

        out.sort_by_key(|r| std::cmp::Reverse(r.created_at));
        if let Some(limit) = limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    fn save_run(&self, run: &AutomationRunRecord) -> Result<()> {
        let dir = self.runs_dir_for(&run.automation_id)?;
        fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
        write_json_atomic(&self.run_path(&run.automation_id, &run.id)?, run)
    }

    async fn enqueue_run_task(
        &self,
        automation: &AutomationRecord,
        run: &mut AutomationRunRecord,
        task_manager: &SharedTaskManager,
    ) -> Result<()> {
        let workspace = automation.cwds.first().cloned();

        let new_task = NewTaskRequest {
            prompt: automation.prompt.clone(),
            model: None,
            workspace,
            mode: Some(automation.task_mode()),
            allow_shell: Some(automation.task_allow_shell()),
            trust_mode: Some(automation.task_trust_mode()),
            auto_approve: Some(automation.task_auto_approve()),
        };

        match task_manager.add_task(new_task).await {
            Ok(task) => {
                run.status = AutomationRunStatus::Running;
                run.started_at = Some(Utc::now());
                run.task_id = Some(task.id.clone());
                run.thread_id = task.thread_id.clone();
                run.turn_id = task.turn_id.clone();
                run.error = None;
                Ok(())
            }
            Err(err) => {
                run.status = AutomationRunStatus::Failed;
                run.ended_at = Some(Utc::now());
                run.error = Some(format!("Failed to enqueue task: {err}"));
                Ok(())
            }
        }
    }

    pub async fn run_now(
        &self,
        automation_id: &str,
        task_manager: &SharedTaskManager,
    ) -> Result<AutomationRunRecord> {
        let mut automation = self.get_automation(automation_id)?;
        let now = Utc::now();
        let mut run = AutomationRunRecord {
            schema_version: CURRENT_RUN_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            automation_id: automation.id.clone(),
            scheduled_for: now,
            status: AutomationRunStatus::Queued,
            created_at: now,
            started_at: None,
            ended_at: None,
            task_id: None,
            thread_id: None,
            turn_id: None,
            error: None,
        };

        self.enqueue_run_task(&automation, &mut run, task_manager)
            .await?;
        self.save_run(&run)?;

        automation.updated_at = Utc::now();
        if matches!(
            run.status,
            AutomationRunStatus::Completed
                | AutomationRunStatus::Failed
                | AutomationRunStatus::Canceled
        ) {
            automation.last_run_at = run.ended_at.or(Some(Utc::now()));
        }
        self.save_automation(&automation)?;

        Ok(run)
    }

    pub async fn scheduler_tick(&self, task_manager: &SharedTaskManager) -> Result<()> {
        let now = Utc::now();
        let mut automations = self.list_automations()?;

        for automation in &mut automations {
            if !matches!(automation.status, AutomationStatus::Active) {
                continue;
            }

            let schedule = AutomationSchedule::parse_rrule(&automation.rrule)?;
            if automation.next_run_at.is_none() {
                automation.next_run_at = Some(schedule.next_after(now)?);
                automation.updated_at = now;
                self.save_automation(automation)?;
                continue;
            }

            let due_at = automation.next_run_at.expect("checked above");
            if due_at > now {
                continue;
            }

            // Idempotency: if a run already exists for this schedule slot, skip enqueue and
            // advance next_run_at.
            let existing_for_slot = self
                .list_runs(&automation.id, Some(25))?
                .into_iter()
                .any(|run| run.scheduled_for == due_at);

            if existing_for_slot {
                automation.next_run_at = Some(schedule.next_after(due_at)?);
                automation.updated_at = now;
                self.save_automation(automation)?;
                continue;
            }

            let mut run = AutomationRunRecord {
                schema_version: CURRENT_RUN_SCHEMA_VERSION,
                id: Uuid::new_v4().to_string(),
                automation_id: automation.id.clone(),
                scheduled_for: due_at,
                status: AutomationRunStatus::Queued,
                created_at: now,
                started_at: None,
                ended_at: None,
                task_id: None,
                thread_id: None,
                turn_id: None,
                error: None,
            };

            self.enqueue_run_task(automation, &mut run, task_manager)
                .await?;
            self.save_run(&run)?;

            automation.updated_at = now;
            automation.next_run_at = Some(schedule.next_after(due_at)?);
            self.save_automation(automation)?;
        }

        Ok(())
    }

    pub async fn reconcile_run_statuses(&self, task_manager: &SharedTaskManager) -> Result<()> {
        let automations = self.list_automations()?;
        for automation in automations {
            let runs = self.list_runs(&automation.id, Some(100))?;
            for mut run in runs {
                if !matches!(
                    run.status,
                    AutomationRunStatus::Queued | AutomationRunStatus::Running
                ) {
                    continue;
                }
                let Some(task_id) = run.task_id.clone() else {
                    continue;
                };
                let task = match task_manager.get_task(&task_id).await {
                    Ok(task) => task,
                    Err(_) => continue,
                };

                run.thread_id = task.thread_id.clone();
                run.turn_id = task.turn_id.clone();

                let mut changed = false;
                match task.status {
                    TaskStatus::Queued => {
                        if !matches!(run.status, AutomationRunStatus::Queued) {
                            run.status = AutomationRunStatus::Queued;
                            changed = true;
                        }
                    }
                    TaskStatus::Running => {
                        if !matches!(run.status, AutomationRunStatus::Running) {
                            run.status = AutomationRunStatus::Running;
                            changed = true;
                        }
                        if run.started_at.is_none() {
                            run.started_at = Some(task.started_at.unwrap_or_else(Utc::now));
                            changed = true;
                        }
                    }
                    TaskStatus::Completed => {
                        run.status = AutomationRunStatus::Completed;
                        run.started_at = run.started_at.or(task.started_at);
                        run.ended_at = task.ended_at.or(Some(Utc::now()));
                        run.error = None;
                        changed = true;
                    }
                    TaskStatus::Failed => {
                        run.status = AutomationRunStatus::Failed;
                        run.started_at = run.started_at.or(task.started_at);
                        run.ended_at = task.ended_at.or(Some(Utc::now()));
                        run.error = task.error.clone();
                        changed = true;
                    }
                    TaskStatus::Canceled => {
                        run.status = AutomationRunStatus::Canceled;
                        run.started_at = run.started_at.or(task.started_at);
                        run.ended_at = task.ended_at.or(Some(Utc::now()));
                        changed = true;
                    }
                }

                if changed {
                    self.save_run(&run)?;
                    if matches!(
                        run.status,
                        AutomationRunStatus::Completed
                            | AutomationRunStatus::Failed
                            | AutomationRunStatus::Canceled
                    ) {
                        let mut updated_automation = self.get_automation(&automation.id)?;
                        updated_automation.last_run_at = run.ended_at.or(Some(Utc::now()));
                        updated_automation.updated_at = Utc::now();
                        self.save_automation(&updated_automation)?;
                    }
                }
            }
        }

        Ok(())
    }
}

fn ensure_safe_storage_id(kind: &str, value: &str) -> Result<()> {
    let mut components = Path::new(value).components();
    let Some(component) = components.next() else {
        bail!("{kind} must not be empty");
    };
    if components.next().is_some() || !matches!(component, std::path::Component::Normal(_)) {
        bail!("{kind} must be a single path component");
    }
    Ok(())
}

fn validate_name_and_prompt(name: &str, prompt: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("Automation name is required");
    }
    if prompt.trim().is_empty() {
        bail!("Automation prompt is required");
    }
    Ok(())
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "Failed to move temporary file {} to {}",
            tmp.display(),
            path.display()
        )
    })?;
    Ok(())
}

pub fn default_automations_dir() -> PathBuf {
    // Most-specific override: an explicit automations dir.
    if let Ok(path) = std::env::var("DEEPSEEK_AUTOMATIONS_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    // $CODEWHALE_HOME is a hard override of the base data directory
    // (docs/CONFIGURATION.md): when SET, automations live under it and we do
    // NOT fall back to the legacy ~/.deepseek path — silent fallback would
    // defeat the isolation the override promises. Check the env var directly
    // (not codewhale_home()'s Ok/Err, which succeeds for the default home too).
    if let Some(home) = std::env::var_os("CODEWHALE_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join("automations");
    }
    dirs::home_dir()
        .map(|home| {
            let primary = home.join(".codewhale").join("automations");
            let legacy = home.join(".deepseek").join("automations");
            if primary.exists() || !legacy.exists() {
                return primary;
            }
            legacy
        })
        .unwrap_or_else(|| PathBuf::from(".codewhale").join("automations"))
}

pub type SharedAutomationManager = Arc<Mutex<AutomationManager>>;

#[derive(Debug, Clone)]
pub struct AutomationSchedulerConfig {
    pub tick_interval_secs: u64,
}

impl Default for AutomationSchedulerConfig {
    fn default() -> Self {
        Self {
            tick_interval_secs: 15,
        }
    }
}

pub fn spawn_scheduler(
    automations: SharedAutomationManager,
    task_manager: SharedTaskManager,
    cancel: CancellationToken,
    config: AutomationSchedulerConfig,
) -> tokio::task::JoinHandle<()> {
    spawn_supervised(
        "automation-scheduler",
        std::panic::Location::caller(),
        async move {
            let interval = config.tick_interval_secs.max(5);
            loop {
                if cancel.is_cancelled() {
                    break;
                }

                {
                    let manager = automations.lock().await;
                    if let Err(err) = manager.scheduler_tick(&task_manager).await {
                        tracing::warn!("automation scheduler tick failed: {err}");
                    }
                    if let Err(err) = manager.reconcile_run_statuses(&task_manager).await {
                        tracing::warn!("automation reconcile failed: {err}");
                    }
                }

                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = sleep(std::time::Duration::from_secs(interval)) => {}
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use crate::task_manager::{
        ExecutionTask, TaskExecutionEvent, TaskExecutionResult, TaskExecutor, TaskManager,
        TaskManagerConfig,
    };

    struct AutomationNoopExecutor;

    #[async_trait]
    impl TaskExecutor for AutomationNoopExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            _events: mpsc::UnboundedSender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some("done".to_string()),
                error: None,
            }
        }
    }

    fn automation_task_config(root: PathBuf) -> TaskManagerConfig {
        TaskManagerConfig {
            data_dir: root,
            worker_count: 1,
            default_workspace: PathBuf::from("."),
            default_model: "deepseek-v4-flash".to_string(),
            default_mode: "plan".to_string(),
            allow_shell: true,
            trust_mode: true,
        }
    }

    fn automation_record_with_settings(
        mode: Option<&str>,
        allow_shell: Option<bool>,
        trust_mode: Option<bool>,
        auto_approve: Option<bool>,
    ) -> AutomationRecord {
        let now = Utc::now();
        AutomationRecord {
            schema_version: CURRENT_AUTOMATION_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            name: "Test automation".to_string(),
            prompt: "Run the automation".to_string(),
            rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
            cwds: Vec::new(),
            mode: mode.map(ToString::to_string),
            allow_shell,
            trust_mode,
            auto_approve,
            status: AutomationStatus::Active,
            created_at: now,
            updated_at: now,
            next_run_at: None,
            last_run_at: None,
        }
    }

    fn queued_run_for(automation: &AutomationRecord) -> AutomationRunRecord {
        let now = Utc::now();
        AutomationRunRecord {
            schema_version: CURRENT_RUN_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            automation_id: automation.id.clone(),
            scheduled_for: now,
            status: AutomationRunStatus::Queued,
            created_at: now,
            started_at: None,
            ended_at: None,
            task_id: None,
            thread_id: None,
            turn_id: None,
            error: None,
        }
    }

    #[test]
    fn parses_hourly_rrule() {
        let parsed =
            AutomationSchedule::parse_rrule("FREQ=HOURLY;INTERVAL=2;BYDAY=MO,TU").expect("parse");
        match parsed {
            AutomationSchedule::Hourly {
                interval_hours,
                byday,
            } => {
                assert_eq!(interval_hours, 2);
                assert_eq!(byday.expect("byday").len(), 2);
            }
            _ => panic!("expected hourly"),
        }
    }

    #[test]
    fn parses_weekly_rrule() {
        let parsed =
            AutomationSchedule::parse_rrule("FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=9;BYMINUTE=30")
                .expect("parse");
        match parsed {
            AutomationSchedule::Weekly {
                byday,
                byhour,
                byminute,
            } => {
                assert_eq!(byday.len(), 2);
                assert_eq!(byhour, 9);
                assert_eq!(byminute, 30);
            }
            _ => panic!("expected weekly"),
        }
    }

    #[test]
    fn rejects_invalid_rrule_fields() {
        let err =
            AutomationSchedule::parse_rrule("FREQ=WEEKLY;BYSECOND=5").expect_err("should fail");
        assert!(err.to_string().contains("Unsupported RRULE field"));
    }

    #[test]
    fn deletes_automation_and_runs() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let manager = AutomationManager::open(tempdir.path().to_path_buf()).expect("manager");

        let created = manager
            .create_automation(CreateAutomationRequest {
                name: "Delete me".to_string(),
                prompt: "prompt".to_string(),
                rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
                cwds: Vec::new(),
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                status: Some(AutomationStatus::Active),
            })
            .expect("create");

        let run = AutomationRunRecord {
            schema_version: CURRENT_RUN_SCHEMA_VERSION,
            id: Uuid::new_v4().to_string(),
            automation_id: created.id.clone(),
            scheduled_for: Utc::now(),
            status: AutomationRunStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            ended_at: None,
            task_id: None,
            thread_id: None,
            turn_id: None,
            error: None,
        };
        manager.save_run(&run).expect("save run");
        assert!(
            manager
                .runs_dir_for(&created.id)
                .expect("runs dir")
                .exists()
        );

        manager
            .delete_automation(&created.id)
            .expect("delete automation");

        assert!(manager.get_automation(&created.id).is_err());
        assert!(
            !manager
                .runs_dir_for(&created.id)
                .expect("runs dir")
                .exists()
        );
    }

    #[test]
    fn automation_storage_rejects_traversal_ids() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let manager = AutomationManager::open(tempdir.path().join("root")).expect("manager");
        let escaped_file = tempdir.path().join("escape.json");
        let escaped_runs = tempdir.path().join("escape-runs");

        let err = manager
            .get_automation("../escape")
            .expect_err("traversal automation ids must be rejected");
        assert!(err.to_string().contains("single path component"));
        assert!(!escaped_file.exists());

        let err = manager
            .list_runs("../escape-runs", None)
            .expect_err("traversal run dirs must be rejected");
        assert!(err.to_string().contains("single path component"));
        assert!(!escaped_runs.exists());

        let run = AutomationRunRecord {
            schema_version: CURRENT_RUN_SCHEMA_VERSION,
            id: "../escape-run".to_string(),
            automation_id: Uuid::new_v4().to_string(),
            scheduled_for: Utc::now(),
            status: AutomationRunStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            ended_at: None,
            task_id: None,
            thread_id: None,
            turn_id: None,
            error: None,
        };
        let err = manager
            .save_run(&run)
            .expect_err("traversal run ids must be rejected");
        assert!(err.to_string().contains("single path component"));
        assert!(!tempdir.path().join("escape-run.json").exists());
    }

    #[test]
    fn automation_task_settings_default_for_legacy_records() {
        let now = Utc::now().to_rfc3339();
        let record: AutomationRecord = serde_json::from_value(serde_json::json!({
            "schema_version": CURRENT_AUTOMATION_SCHEMA_VERSION,
            "id": Uuid::new_v4().to_string(),
            "name": "Legacy automation",
            "prompt": "Run legacy automation",
            "rrule": "FREQ=HOURLY;INTERVAL=1",
            "cwds": [],
            "status": "active",
            "created_at": now,
            "updated_at": now
        }))
        .expect("legacy automation record should deserialize");

        assert_eq!(record.mode, None);
        assert_eq!(record.task_mode(), "agent");
        assert!(!record.task_allow_shell());
        assert!(!record.task_trust_mode());
        assert!(record.task_auto_approve());
    }

    #[tokio::test]
    async fn automation_enqueue_uses_default_and_explicit_task_settings() -> Result<()> {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let automation_manager =
            AutomationManager::open(tempdir.path().join("automations")).expect("manager");
        let task_manager = TaskManager::start_with_executor(
            automation_task_config(tempdir.path().join("tasks")),
            std::sync::Arc::new(AutomationNoopExecutor),
        )
        .await?;

        let default_automation = automation_record_with_settings(None, None, None, None);
        let mut default_run = queued_run_for(&default_automation);
        automation_manager
            .enqueue_run_task(&default_automation, &mut default_run, &task_manager)
            .await?;
        let default_task = task_manager
            .get_task(default_run.task_id.as_deref().expect("task id"))
            .await?;
        assert_eq!(default_task.mode, "agent");
        assert!(!default_task.allow_shell);
        assert!(!default_task.trust_mode);
        assert!(default_task.auto_approve);

        let explicit_automation =
            automation_record_with_settings(Some("plan"), Some(true), Some(true), Some(false));
        let mut explicit_run = queued_run_for(&explicit_automation);
        automation_manager
            .enqueue_run_task(&explicit_automation, &mut explicit_run, &task_manager)
            .await?;
        let explicit_task = task_manager
            .get_task(explicit_run.task_id.as_deref().expect("task id"))
            .await?;
        assert_eq!(explicit_task.mode, "plan");
        assert!(explicit_task.allow_shell);
        assert!(explicit_task.trust_mode);
        assert!(!explicit_task.auto_approve);

        task_manager.shutdown();
        Ok(())
    }

    #[test]
    fn default_automations_dir_honors_codewhale_home_as_hard_override() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().unwrap();
        // SAFETY: serialised by lock_test_env.
        unsafe {
            std::env::remove_var("DEEPSEEK_AUTOMATIONS_DIR");
            std::env::set_var("CODEWHALE_HOME", tmp.path());
        }
        // $CODEWHALE_HOME IS the home dir (no ".codewhale" appended); the
        // legacy ~/.deepseek fallback is bypassed entirely.
        assert_eq!(default_automations_dir(), tmp.path().join("automations"));
        // SAFETY: cleanup under the same lock.
        unsafe {
            std::env::remove_var("CODEWHALE_HOME");
        }
    }

    #[test]
    fn default_automations_dir_prefers_deepseek_automations_dir_over_codewhale_home() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().unwrap();
        // SAFETY: serialised by lock_test_env.
        unsafe {
            std::env::set_var("DEEPSEEK_AUTOMATIONS_DIR", tmp.path());
            std::env::set_var("CODEWHALE_HOME", "/should/not/be/used");
        }
        // The most-specific override wins over the base-data-dir override.
        assert_eq!(default_automations_dir(), tmp.path());
        // SAFETY: cleanup under the same lock.
        unsafe {
            std::env::remove_var("DEEPSEEK_AUTOMATIONS_DIR");
            std::env::remove_var("CODEWHALE_HOME");
        }
    }
}
