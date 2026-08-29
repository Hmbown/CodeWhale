//! Operate: always-on pod operation matching landed CWC `OperateRecord`
//! (`Hmbown/cwc` `20de981`, PR #284).
//!
//! One schema for `cw · operate` and CWC `/operate`. Burn rate is optional
//! (`null` = unbounded). The lead plans before workers. Pace throttles or
//! widens; it never stops the operation. Supervisor over nested instances —
//! no second `Engine::run_turn`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::automation_manager::{
    AutomationManager, AutomationStatus, CreateAutomationRequest, UpdateAutomationRequest,
};

pub const CWC_OPERATE_SCHEMA_VERSION: u32 = 1;
pub const CWC_OPERATE_DEFAULT_LEAD_MODEL: &str = "GLM-5.3";
pub const CWC_OPERATE_DEFAULT_WORKER_MODEL: &str = "GLM-5.3-Flash";
pub const OPERATE_LEAD_MODEL: &str = CWC_OPERATE_DEFAULT_LEAD_MODEL;
pub const OPERATE_WORKER_MODEL: &str = CWC_OPERATE_DEFAULT_WORKER_MODEL;
pub const OPERATE_MAX_WRITERS: usize = 3;
pub const OPERATE_KEEPALIVE_ID: &str = "cw-operate";
pub const AUTO_MERGE_CHECKER_ENV: &str = "CODEWHALE_AUTO_MERGE_CHECKER";
pub const DIRECTION_PATH_ENV: &str = "CODEWHALE_DIRECTION_PATH";
pub const CHECK_AUTO_MERGE_SCRIPT: &str = "scripts/check-auto-merge.py";
pub const AUTO_MERGE_SCRIPT: &str = "scripts/auto_merge.py";
pub const AUTO_MERGE_PR_SCRIPT: &str = "scripts/auto-merge-pr.py";

const PACE_BAND: f64 = 0.08;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperateBurnRate {
    pub kind: String,
    pub amount_usd_per_hour: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperateStatus {
    Planning,
    Running,
    #[serde(rename = "idle_blocked")]
    IdleBlocked,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperateIdleReason {
    MissingCredentials,
    AwaitingLeadPlan,
    DirectionEmpty,
    HumanGated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatePace {
    Unbounded,
    Hold,
    Throttle,
    Widen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperateRosterMember {
    pub id: String,
    pub display_name: String,
    pub role: String,
    pub model: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatePlanSlice {
    pub id: String,
    pub title: String,
    pub owner_id: String,
    pub depends_on: Vec<String>,
    pub est_cost_usd: f64,
    pub start_offset_sec: u32,
    pub duration_sec: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperateLeadPlan {
    pub slices: Vec<OperatePlanSlice>,
}

/// Landed CWC `OperateRecord` (packages/contracts/src/operate.js @ 20de981).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub schema_version: u32,
    pub direction: String,
    pub burn_rate: Option<OperateBurnRate>,
    pub lead_operator: OperateRosterMember,
    pub roster: Vec<OperateRosterMember>,
    pub lead_plan: Option<OperateLeadPlan>,
    pub status: OperateStatus,
    pub idle_blocked_reason: Option<OperateIdleReason>,
    pub pace: OperatePace,
    pub writers_in_flight: usize,
    pub workers_admitted: bool,
    pub spent_usd: f64,
    pub observed_burn_usd_per_hour: Option<f64>,
    pub credentials_present: bool,
    pub human_gated: bool,
    pub human_gate: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_keep_alive_at: String,
    #[serde(default)]
    pub cancelled_at: String,
}

impl Operation {
    #[must_use]
    pub fn new(direction: impl Into<String>, burn_usd_per_hour: Option<f64>) -> Self {
        let now = Utc::now().to_rfc3339();
        let lead = OperateRosterMember {
            id: "lead".to_string(),
            display_name: "Lead operator".to_string(),
            role: "lead".to_string(),
            model: CWC_OPERATE_DEFAULT_LEAD_MODEL.to_string(),
            state: "planning".to_string(),
        };
        let mut op = Self {
            id: format!("op_{}", Uuid::new_v4()),
            schema_version: CWC_OPERATE_SCHEMA_VERSION,
            direction: normalize_direction(direction.into()),
            burn_rate: normalize_burn_rate(burn_usd_per_hour),
            lead_operator: lead.clone(),
            roster: vec![lead],
            lead_plan: None,
            status: OperateStatus::Planning,
            idle_blocked_reason: None,
            pace: OperatePace::Unbounded,
            writers_in_flight: 0,
            workers_admitted: false,
            spent_usd: 0.0,
            observed_burn_usd_per_hour: None,
            credentials_present: false,
            human_gated: false,
            human_gate: String::new(),
            created_at: now.clone(),
            updated_at: now.clone(),
            last_keep_alive_at: now,
            cancelled_at: String::new(),
        };
        op.project();
        op
    }

    pub fn plan_from_direction(&mut self) {
        self.lead_plan = slices_from_direction(&self.direction);
        if let Some(plan) = &self.lead_plan {
            for slice in &plan.slices {
                if slice.owner_id != "lead"
                    && !self.roster.iter().any(|member| member.id == slice.owner_id)
                {
                    self.roster.push(OperateRosterMember {
                        id: slice.owner_id.clone(),
                        display_name: slice.owner_id.clone(),
                        role: "worker".to_string(),
                        model: OPERATE_WORKER_MODEL.to_string(),
                        state: "idle".to_string(),
                    });
                }
            }
        }
        self.project();
    }

    pub fn project(&mut self) {
        if self.status == OperateStatus::Cancelled {
            self.idle_blocked_reason = None;
            self.workers_admitted = false;
            self.writers_in_flight = 0;
            for member in &mut self.roster {
                member.state = "idle".to_string();
            }
            return;
        }
        let (status, reason) = derive_status(self);
        self.status = status;
        self.idle_blocked_reason = reason;
        self.workers_admitted = workers_admitted(self);
        self.pace = derive_pace(self);
        live_roster(self);
        self.writers_in_flight = if self.workers_admitted {
            self.roster
                .iter()
                .filter(|member| member.role == "worker" && member.state == "in_flight")
                .count()
                .min(OPERATE_MAX_WRITERS)
        } else {
            0
        };
        if let Some(lead) = self.roster.iter().find(|member| member.role == "lead") {
            self.lead_operator = lead.clone();
        }
    }
}

fn normalize_direction(value: String) -> String {
    value.trim().chars().take(4000).collect()
}

fn normalize_burn_rate(amount: Option<f64>) -> Option<OperateBurnRate> {
    parse_burn_amount(amount).ok().flatten()
}

/// CWC `normalizeOperateBurnRate`: number, `$/hr` object, or null.
pub fn parse_burn_rate(value: Option<&serde_json::Value>) -> Result<Option<OperateBurnRate>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null()
        || value == &serde_json::Value::Bool(false)
        || value.as_str().is_some_and(str::is_empty)
    {
        return Ok(None);
    }
    if let Some(kind) = value.get("kind").and_then(|kind| kind.as_str()) {
        if kind == "unbounded" {
            return Ok(None);
        }
    }
    let amount = if value.is_number() || value.is_string() {
        json_number(value)
    } else {
        json_number(
            value
                .get("amountUsdPerHour")
                .or_else(|| value.get("usdPerHour"))
                .or_else(|| value.get("amount"))
                .unwrap_or(&serde_json::Value::Null),
        )
    };
    if amount.is_none() {
        anyhow::bail!("Burn rate is optional. When set, it must be a positive $/hr.");
    }
    parse_burn_amount(amount)
}

fn json_number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|n| n as f64))
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn parse_burn_amount(amount: Option<f64>) -> Result<Option<OperateBurnRate>> {
    let Some(amount) = amount else {
        return Ok(None);
    };
    if !amount.is_finite() || amount <= 0.0 {
        anyhow::bail!("Burn rate is optional. When set, it must be a positive $/hr.");
    }
    if amount > 10_000.0 {
        anyhow::bail!("Burn rate must be 10000 $/hr or less.");
    }
    Ok(Some(OperateBurnRate {
        kind: "usd_per_hour".to_string(),
        amount_usd_per_hour: (amount * 100.0).round() / 100.0,
    }))
}

fn derive_status(op: &Operation) -> (OperateStatus, Option<OperateIdleReason>) {
    if op.status == OperateStatus::Cancelled {
        return (OperateStatus::Cancelled, None);
    }
    if !op.credentials_present {
        return (
            OperateStatus::IdleBlocked,
            Some(OperateIdleReason::MissingCredentials),
        );
    }
    if op.direction.is_empty() {
        return (
            OperateStatus::IdleBlocked,
            Some(OperateIdleReason::DirectionEmpty),
        );
    }
    if op.human_gated {
        return (OperateStatus::IdleBlocked, Some(OperateIdleReason::HumanGated));
    }
    if op.lead_plan.as_ref().is_none_or(|plan| plan.slices.is_empty()) {
        return (
            OperateStatus::IdleBlocked,
            Some(OperateIdleReason::AwaitingLeadPlan),
        );
    }
    (OperateStatus::Running, None)
}

fn workers_admitted(op: &Operation) -> bool {
    op.status != OperateStatus::Cancelled
        && op.credentials_present
        && !op.direction.is_empty()
        && op.lead_plan.as_ref().is_some_and(|plan| !plan.slices.is_empty())
        && !op.human_gated
}

fn derive_pace(op: &Operation) -> OperatePace {
    let Some(rate) = &op.burn_rate else {
        return OperatePace::Unbounded;
    };
    let Some(observed) = op.observed_burn_usd_per_hour.filter(|value| *value > 0.0) else {
        return OperatePace::Widen;
    };
    let target = rate.amount_usd_per_hour;
    if target <= 0.0 {
        return OperatePace::Unbounded;
    }
    let delta = (observed - target) / target;
    if delta > PACE_BAND {
        OperatePace::Throttle
    } else if delta < -PACE_BAND {
        OperatePace::Widen
    } else {
        OperatePace::Hold
    }
}

fn live_roster(op: &mut Operation) {
    let admitted = op.workers_admitted;
    let cancelled = op.status == OperateStatus::Cancelled;
    let has_plan = op.lead_plan.as_ref().is_some_and(|plan| !plan.slices.is_empty());
    for member in &mut op.roster {
        if cancelled {
            member.state = "idle".to_string();
        } else if !op.credentials_present {
            member.state = "blocked".to_string();
        } else if member.role == "lead" && !has_plan {
            member.state = "planning".to_string();
        } else if !admitted {
            member.state = if member.role == "lead" {
                "planning".to_string()
            } else {
                "idle".to_string()
            };
        } else if member.role == "worker" {
            member.state = "in_flight".to_string();
        } else {
            member.state = "planning".to_string();
        }
    }
}

fn slices_from_direction(direction: &str) -> Option<OperateLeadPlan> {
    let items = direction_items(direction);
    if items.is_empty() {
        return None;
    }
    let mut cursor = 0u32;
    let slices = items
        .into_iter()
        .enumerate()
        .map(|(index, title)| {
            let duration_sec = 1800;
            let slice = OperatePlanSlice {
                id: format!("slice-{}", index + 1),
                title,
                owner_id: if index == 0 {
                    "lead".to_string()
                } else {
                    format!("worker-{index}")
                },
                depends_on: if index == 0 {
                    Vec::new()
                } else {
                    vec![format!("slice-{index}")]
                },
                est_cost_usd: 0.25,
                start_offset_sec: cursor,
                duration_sec,
            };
            cursor = cursor.saturating_add(duration_sec);
            slice
        })
        .collect();
    Some(OperateLeadPlan { slices })
}

fn direction_items(direction: &str) -> Vec<String> {
    let mut items = Vec::new();
    for raw in direction.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let item = line
            .trim_start_matches(|c: char| {
                c.is_ascii_digit() || c == '.' || c == ')' || c == '-' || c == '*' || c == '#'
            })
            .trim();
        if !item.is_empty() {
            items.push(item.to_string());
        }
    }
    if items.is_empty() && !direction.trim().is_empty() {
        items.push(direction.trim().to_string());
    }
    items
}

#[must_use]
pub fn render_plan_board(op: &Operation) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Operate {id}  [{status}]  pace={pace}  writers={writers}\n",
        id = op.id,
        status = status_label(op),
        pace = pace_label(op.pace),
        writers = op.writers_in_flight
    ));
    match &op.burn_rate {
        Some(rate) => out.push_str(&format!(
            "burn  ${actual}/hr observed  vs  ${target}/hr target\n",
            actual = op.observed_burn_usd_per_hour.unwrap_or(0.0),
            target = rate.amount_usd_per_hour
        )),
        None => out.push_str("burn  No cap\n"),
    }
    if op.direction.is_empty() {
        out.push_str("direction  (empty — idle-blocked)\n");
    } else {
        out.push_str(&format!(
            "direction  {}\n",
            op.direction.lines().next().unwrap_or("")
        ));
    }
    let Some(plan) = &op.lead_plan else {
        out.push_str("leadPlan  (none — workers not admitted)\n");
        return out;
    };
    out.push_str(
        "leadPlan  id        owner    start  dur   est$   depends  title\n",
    );
    for slice in &plan.slices {
        out.push_str(&format!(
            "          {:<9} {:<8} {:>5} {:>5} {:>6.2}  {:<7} {}\n",
            slice.id,
            slice.owner_id,
            slice.start_offset_sec,
            slice.duration_sec,
            slice.est_cost_usd,
            if slice.depends_on.is_empty() {
                "-".to_string()
            } else {
                slice.depends_on.join(",")
            },
            slice.title
        ));
    }
    out.push_str(&render_timeline(&plan.slices));
    out
}

fn render_timeline(slices: &[OperatePlanSlice]) -> String {
    let max_end = slices
        .iter()
        .map(|slice| slice.start_offset_sec.saturating_add(slice.duration_sec))
        .max()
        .unwrap_or(0)
        .max(1);
    let width = 24u32;
    let mut out = String::from("gantt  time →\n");
    for slice in slices {
        let start = (slice.start_offset_sec.saturating_mul(width)) / max_end;
        let end = (slice
            .start_offset_sec
            .saturating_add(slice.duration_sec)
            .saturating_mul(width))
            / max_end;
        let end = end.max(start.saturating_add(1)).min(width);
        let mut bar = vec!['.'; width as usize];
        for idx in start..end {
            if let Some(cell) = bar.get_mut(idx as usize) {
                *cell = '#';
            }
        }
        out.push_str(&format!(
            "      {:<9} {}\n",
            slice.id,
            bar.into_iter().collect::<String>()
        ));
    }
    out
}

fn status_label(op: &Operation) -> String {
    match (op.status, op.idle_blocked_reason) {
        (OperateStatus::Cancelled, _) => "cancelled".to_string(),
        (OperateStatus::Running, _) => "running".to_string(),
        (OperateStatus::Planning, _) => "planning".to_string(),
        (_, Some(OperateIdleReason::DirectionEmpty)) => "idle_blocked: direction_empty".to_string(),
        (_, Some(OperateIdleReason::AwaitingLeadPlan)) => {
            "idle_blocked: awaiting_lead_plan".to_string()
        }
        (_, Some(OperateIdleReason::MissingCredentials)) => {
            "idle_blocked: missing_credentials".to_string()
        }
        (_, Some(OperateIdleReason::HumanGated)) => {
            format!("idle_blocked: human_gated {}", op.human_gate)
        }
        (OperateStatus::IdleBlocked, None) => "idle_blocked".to_string(),
    }
}

fn pace_label(pace: OperatePace) -> &'static str {
    match pace {
        OperatePace::Unbounded => "unbounded",
        OperatePace::Hold => "hold",
        OperatePace::Throttle => "throttle",
        OperatePace::Widen => "widen",
    }
}

#[must_use]
pub fn discover_direction_path(workspace: &Path) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(DIRECTION_PATH_ENV) {
        let path = PathBuf::from(explicit.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let local = workspace.join("DIRECTION.md");
    if local.is_file() {
        return Some(local);
    }
    materialize_ops_origin_main()
        .ok()
        .map(|root| root.join("DIRECTION.md"))
        .filter(|path| path.is_file())
}

pub fn read_direction(workspace: &Path) -> Result<String> {
    match discover_direction_path(workspace) {
        Some(path) => fs::read_to_string(&path)
            .with_context(|| format!("Failed to read direction {}", path.display())),
        None => Ok(String::new()),
    }
}

#[must_use]
pub fn glm_credentials_present(lookup: impl Fn(&str) -> bool) -> bool {
    lookup("ZAI_API_KEY") || lookup("Z_AI_API_KEY") || lookup("ZAI_AUTH_TOKEN")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoMergeRequest<'a> {
    pub pr: &'a str,
    pub role: &'a str,
    pub repo: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoMergeDecision {
    Allow,
    Deny { reason: String },
}

#[must_use]
pub fn check_auto_merge_args(repo: &str, pr: &str, agent: &str) -> Vec<String> {
    vec![
        CHECK_AUTO_MERGE_SCRIPT.to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--pr".to_string(),
        pr.to_string(),
        "--agent".to_string(),
        agent.to_string(),
    ]
}

#[must_use]
pub fn auto_merge_pr_args(repo: &str, pr: &str, agent: &str) -> Vec<String> {
    vec![
        AUTO_MERGE_PR_SCRIPT.to_string(),
        "--repo".to_string(),
        repo.to_string(),
        "--pr".to_string(),
        pr.to_string(),
        "--agent".to_string(),
        agent.to_string(),
    ]
}

pub fn evaluate_auto_merge(request: AutoMergeRequest<'_>, checker: Option<&Path>) -> AutoMergeDecision {
    let Some(checker) = checker else {
        return AutoMergeDecision::Deny {
            reason: "auto-merge checker missing; fail-closed".to_string(),
        };
    };
    if !checker.exists() {
        return AutoMergeDecision::Deny {
            reason: "auto-merge checker missing; fail-closed".to_string(),
        };
    }
    match Command::new("python3")
        .arg(checker)
        .arg("--repo")
        .arg(request.repo)
        .arg("--pr")
        .arg(request.pr)
        .arg("--agent")
        .arg(request.role)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => AutoMergeDecision::Allow,
        Ok(_) => AutoMergeDecision::Deny {
            reason: "auto-merge checker refused".to_string(),
        },
        Err(error) => AutoMergeDecision::Deny {
            reason: format!("auto-merge checker failed to start: {error}"),
        },
    }
}

#[must_use]
pub fn discover_auto_merge_checker(_workspace: &Path) -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(AUTO_MERGE_CHECKER_ENV) {
        let path = PathBuf::from(explicit.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    materialize_ops_origin_main()
        .ok()
        .map(|root| root.join(CHECK_AUTO_MERGE_SCRIPT))
        .filter(|path| path.is_file())
}

fn ops_git_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for key in ["CODEWHALE_OPS_GIT", "CODEWHALE_OPS_ROOT"] {
        if let Ok(path) = std::env::var(key) {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                out.push(PathBuf::from(trimmed));
            }
        }
    }
    out
}

fn git_origin_main_sha(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "origin/main"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (sha.len() >= 7).then_some(sha)
}

pub fn materialize_ops_origin_main() -> Result<PathBuf> {
    let repo = ops_git_candidates()
        .into_iter()
        .find(|path| git_origin_main_sha(path).is_some())
        .context("no codewhale-ops git checkout with origin/main")?;
    let sha = git_origin_main_sha(&repo).context("origin/main sha")?;
    let dest = default_operate_dir()
        .parent()
        .unwrap_or(Path::new("."))
        .join("ops-main")
        .join(&sha[..12.min(sha.len())]);
    let marker = dest.join(CHECK_AUTO_MERGE_SCRIPT);
    if marker.is_file() && dest.join("DIRECTION.md").is_file() && dest.join(AUTO_MERGE_SCRIPT).is_file()
    {
        return Ok(dest);
    }
    fs::create_dir_all(&dest)
        .with_context(|| format!("Failed to create {}", dest.display()))?;
    let archive = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args([
            "archive",
            "origin/main",
            "--",
            "DIRECTION.md",
            CHECK_AUTO_MERGE_SCRIPT,
            AUTO_MERGE_SCRIPT,
            AUTO_MERGE_PR_SCRIPT,
            "agent-workstreams/AUTO_MERGE.toml",
        ])
        .stdin(Stdio::null())
        .output()
        .context("git archive origin/main")?;
    if !archive.status.success() {
        anyhow::bail!(
            "git archive origin/main failed: {}",
            String::from_utf8_lossy(&archive.stderr).trim()
        );
    }
    let mut child = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(&dest)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("tar extract ops origin/main")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(&archive.stdout)?;
    }
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("failed to extract ops origin/main archive");
    }
    if !marker.is_file() {
        anyhow::bail!("ops origin/main archive missing {CHECK_AUTO_MERGE_SCRIPT}");
    }
    Ok(dest)
}

pub fn default_operate_dir() -> PathBuf {
    if let Ok(path) = std::env::var("CODEWHALE_OPERATE_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    crate::automation_manager::default_automations_dir()
        .parent()
        .map(|parent| parent.join("operate"))
        .unwrap_or_else(|| PathBuf::from("operate"))
}

pub struct OperationStore {
    path: PathBuf,
}

impl OperationStore {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        Ok(Self {
            path: dir.join("current.json"),
        })
    }

    pub fn load(&self) -> Result<Option<Operation>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&self.path)
            .with_context(|| format!("Failed to read {}", self.path.display()))?;
        let op: Operation = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse {}", self.path.display()))?;
        Ok(Some(op))
    }

    pub fn save(&self, op: &Operation) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(op)?)
            .with_context(|| format!("Failed to write {}", tmp.display()))?;
        fs::rename(&tmp, &self.path).with_context(|| {
            format!(
                "Failed to move {} to {}",
                tmp.display(),
                self.path.display()
            )
        })?;
        Ok(())
    }
}

pub fn start_operation(
    store: &OperationStore,
    workspace: &Path,
    direction: Option<String>,
    burn_usd_per_hour: Option<f64>,
    credentials_present: bool,
) -> Result<Operation> {
    let mut direction = match direction {
        Some(text) if !text.trim().is_empty() => text,
        _ => read_direction(workspace)?,
    };
    if direction.trim().is_empty() {
        if let Some(existing) = store.load()? {
            direction = existing.direction;
        }
    }
    let mut op = Operation::new(direction, burn_usd_per_hour);
    op.credentials_present = credentials_present;
    op.project();
    store.save(&op)?;
    Ok(op)
}

pub fn apply_operate_patch(op: &mut Operation, patch: &serde_json::Value) -> Result<()> {
    if op.status == OperateStatus::Cancelled {
        anyhow::bail!("A cancelled Operation cannot be edited.");
    }
    if let Some(direction) = patch.get("direction") {
        op.direction = normalize_direction(
            direction
                .as_str()
                .map(str::to_string)
                .unwrap_or_default(),
        );
    }
    if patch.get("burnRate").is_some() {
        op.burn_rate = parse_burn_rate(patch.get("burnRate"))?;
    }
    if let Some(plan) = patch.get("leadPlan") {
        op.lead_plan = if plan.is_null() {
            None
        } else {
            Some(serde_json::from_value(plan.clone()).context("leadPlan is invalid")?)
        };
    }
    if let Some(flag) = patch.get("humanGated").and_then(serde_json::Value::as_bool) {
        op.human_gated = flag;
    }
    if let Some(gate) = patch.get("humanGate").and_then(serde_json::Value::as_str) {
        op.human_gate = gate.trim().chars().take(160).collect();
        if human_gate_for(&op.human_gate) {
            op.human_gated = true;
        }
    }
    if let Some(flag) = patch
        .get("credentialsPresent")
        .and_then(serde_json::Value::as_bool)
    {
        op.credentials_present = flag;
    }
    op.updated_at = Utc::now().to_rfc3339();
    op.project();
    Ok(())
}

pub fn cancel_operation(store: &OperationStore) -> Result<Option<Operation>> {
    let Some(mut op) = store.load()? else {
        return Ok(None);
    };
    let now = Utc::now().to_rfc3339();
    op.status = OperateStatus::Cancelled;
    op.cancelled_at = now.clone();
    op.updated_at = now.clone();
    op.last_keep_alive_at = now;
    op.project();
    store.save(&op)?;
    Ok(Some(op))
}

pub fn keep_alive_observation(
    op: &mut Operation,
    observed_burn_usd_per_hour: Option<f64>,
    spent_usd: Option<f64>,
    credentials_present: Option<bool>,
    human_gated: Option<bool>,
) {
    let now = Utc::now().to_rfc3339();
    op.last_keep_alive_at = now.clone();
    op.updated_at = now;
    if let Some(burn) = observed_burn_usd_per_hour {
        op.observed_burn_usd_per_hour = Some(burn.max(0.0));
    }
    if let Some(spent) = spent_usd {
        op.spent_usd = spent.max(0.0);
    }
    if let Some(credentials) = credentials_present {
        op.credentials_present = credentials;
    }
    if let Some(gated) = human_gated {
        op.human_gated = gated;
    }
    op.project();
}

pub fn upsert_keepalive(manager: &AutomationManager, workspace: &Path) -> Result<()> {
    let prompt = format!(
        "Keep Operate alive. Read direction, refresh the lead plan, dispatch ready slices up to the burn-rate governor, and never stop for a wallet cap. Workspace: {}",
        workspace.display()
    );
    if manager.get_automation(OPERATE_KEEPALIVE_ID).is_ok() {
        manager.update_automation(
            OPERATE_KEEPALIVE_ID,
            UpdateAutomationRequest {
                name: Some("Operate keep-alive".to_string()),
                prompt: Some(prompt),
                rrule: Some("FREQ=HOURLY;INTERVAL=1".to_string()),
                model: Some(OPERATE_LEAD_MODEL.to_string()),
                mode: Some("operate".to_string()),
                status: Some(AutomationStatus::Active),
                ..UpdateAutomationRequest::default()
            },
        )?;
        return Ok(());
    }
    let created = manager.create_automation(CreateAutomationRequest {
        name: "Operate keep-alive".to_string(),
        prompt,
        rrule: "FREQ=HOURLY;INTERVAL=1".to_string(),
        cwds: vec![workspace.to_path_buf()],
        model: Some(OPERATE_LEAD_MODEL.to_string()),
        mode: Some("operate".to_string()),
        allow_shell: Some(true),
        trust_mode: Some(false),
        auto_approve: Some(false),
        delivery_mode: None,
        status: Some(AutomationStatus::Active),
    })?;
    let mut record = created;
    let _ = manager.delete_automation(&record.id);
    record.id = OPERATE_KEEPALIVE_ID.to_string();
    manager.save_automation(&record)?;
    Ok(())
}

#[must_use]
pub fn human_gate_for(action: &str) -> bool {
    matches!(
        action,
        "deploy" | "billing" | "force-push" | "forbidden-pr" | "red-ci"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_credentials(mut op: Operation) -> Operation {
        op.credentials_present = true;
        op.project();
        op
    }

    #[test]
    fn unbounded_start_matches_cwc_contract() {
        let op = with_credentials(Operation::new("Keep shipping honest slices", None));
        assert_eq!(op.schema_version, 1);
        assert!(op.burn_rate.is_none());
        assert_eq!(op.pace, OperatePace::Unbounded);
        assert_eq!(op.status, OperateStatus::IdleBlocked);
        assert_eq!(
            op.idle_blocked_reason,
            Some(OperateIdleReason::AwaitingLeadPlan)
        );
        assert!(!op.workers_admitted);
        assert_eq!(op.lead_operator.model, "GLM-5.3");
        let json = serde_json::to_value(&op).expect("json");
        assert!(json.get("burnRate").unwrap().is_null());
        assert_eq!(json["leadOperator"]["model"], "GLM-5.3");
        assert_eq!(json["schemaVersion"], 1);
        assert!(json.get("leadPlan").unwrap().is_null());
        assert_eq!(json["idleBlockedReason"], "awaiting_lead_plan");
        assert_eq!(json["workersAdmitted"], false);
        assert!(json["id"].as_str().unwrap().starts_with("op_"));
    }

    #[test]
    fn create_without_credentials_fails_closed() {
        let op = Operation::new("Keep shipping honest slices", None);
        assert_eq!(op.status, OperateStatus::IdleBlocked);
        assert_eq!(
            op.idle_blocked_reason,
            Some(OperateIdleReason::MissingCredentials)
        );
        assert!(!op.workers_admitted);
    }

    #[test]
    fn burn_rate_paces_and_never_stops() {
        let mut op = with_credentials(Operation::new("Hold a $12/hr burn", Some(12.0)));
        op.plan_from_direction();
        assert_eq!(op.status, OperateStatus::Running);
        assert!(op.workers_admitted);
        keep_alive_observation(&mut op, Some(20.0), Some(80.0), None, None);
        assert_eq!(op.status, OperateStatus::Running);
        assert_eq!(op.pace, OperatePace::Throttle);
        assert!(op.idle_blocked_reason.is_none());
        assert!(op.workers_admitted);
        let board = render_plan_board(&op);
        assert!(!board.contains("exhausted"));
        assert!(!board.contains("wallet"));
        assert_eq!(op.burn_rate.as_ref().unwrap().kind, "usd_per_hour");
        assert!((op.burn_rate.as_ref().unwrap().amount_usd_per_hour - 12.0).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_credentials_fail_closed() {
        let dir = TempDir::new().expect("temp");
        let store = OperationStore::open(dir.path()).expect("store");
        let op = start_operation(&store, dir.path(), Some("Do not spend silently".into()), None, false)
            .expect("start");
        assert_eq!(op.status, OperateStatus::IdleBlocked);
        assert_eq!(
            op.idle_blocked_reason,
            Some(OperateIdleReason::MissingCredentials)
        );
        assert!(!op.workers_admitted);
        assert_eq!(op.writers_in_flight, 0);
    }

    #[test]
    fn cancel_stays_cancelled_through_keep_alive() {
        let dir = TempDir::new().expect("temp");
        let store = OperationStore::open(dir.path()).expect("store");
        start_operation(&store, dir.path(), Some("Stop".into()), None, true).expect("start");
        let cancelled = cancel_operation(&store).expect("cancel").expect("present");
        let mut kept = cancelled;
        keep_alive_observation(&mut kept, Some(40.0), Some(999.0), None, None);
        assert_eq!(kept.status, OperateStatus::Cancelled);
        assert!(!kept.workers_admitted);
    }

    #[test]
    fn lead_plan_is_the_gantt_model() {
        let mut op = with_credentials(Operation::new("Scout\nWrite", None));
        op.plan_from_direction();
        let plan = op.lead_plan.as_ref().expect("plan");
        assert_eq!(plan.slices.len(), 2);
        assert_eq!(plan.slices[0].owner_id, "lead");
        assert_eq!(plan.slices[1].depends_on, vec!["slice-1".to_string()]);
        assert_eq!(plan.slices[0].start_offset_sec, 0);
        assert!(plan.slices[0].duration_sec >= 1);
        let board = render_plan_board(&op);
        assert!(board.contains("gantt  time →"), "{board}");
        assert!(board.contains("leadPlan"), "{board}");
        assert!(board.contains("No cap"), "{board}");
    }

    #[test]
    fn empty_direction_is_idle_blocked() {
        let op = with_credentials(Operation::new("", None));
        assert_eq!(op.status, OperateStatus::IdleBlocked);
        assert_eq!(
            op.idle_blocked_reason,
            Some(OperateIdleReason::DirectionEmpty)
        );
    }

    #[test]
    fn under_rate_widens() {
        let mut op = with_credentials(Operation::new("one\ntwo\nthree", Some(12.0)));
        op.plan_from_direction();
        keep_alive_observation(&mut op, Some(1.0), None, None, None);
        assert_eq!(op.pace, OperatePace::Widen);
        assert_eq!(op.status, OperateStatus::Running);
    }

    #[test]
    fn human_gates_do_not_include_merge() {
        assert!(human_gate_for("deploy"));
        assert!(human_gate_for("billing"));
        assert!(!human_gate_for("merge"));
    }

    #[test]
    fn calls_landed_checker_flags() {
        assert_eq!(
            check_auto_merge_args("Hmbown/CodeWhale", "1234", "keel"),
            vec![
                "scripts/check-auto-merge.py",
                "--repo",
                "Hmbown/CodeWhale",
                "--pr",
                "1234",
                "--agent",
                "keel"
            ]
        );
        assert_eq!(
            auto_merge_pr_args("Hmbown/CodeWhale", "1234", "keel")[0],
            "scripts/auto-merge-pr.py"
        );
        let deny = evaluate_auto_merge(
            AutoMergeRequest {
                pr: "12",
                role: "keel",
                repo: "Hmbown/CodeWhale",
            },
            None,
        );
        assert!(matches!(deny, AutoMergeDecision::Deny { .. }));
        let _ = AUTO_MERGE_CHECKER_ENV;
        let _ = discover_auto_merge_checker(Path::new("/no-ops-here"));
    }

    #[test]
    fn checker_exit_zero_allows() {
        let dir = TempDir::new().expect("temp");
        let checker = dir.path().join("check-auto-merge.py");
        fs::write(
            &checker,
            "#!/usr/bin/env python3\nimport argparse, sys\np=argparse.ArgumentParser()\np.add_argument('--repo')\np.add_argument('--pr')\np.add_argument('--agent', required=True)\np.parse_args()\nsys.exit(0)\n",
        )
        .expect("write");
        assert_eq!(
            evaluate_auto_merge(
                AutoMergeRequest {
                    pr: "42",
                    role: "keel",
                    repo: "Hmbown/CodeWhale",
                },
                Some(&checker),
            ),
            AutoMergeDecision::Allow
        );
    }

    #[test]
    fn keepalive_automation_and_defaults() {
        let dir = TempDir::new().expect("temp");
        let manager = AutomationManager::open(dir.path().to_path_buf()).expect("manager");
        upsert_keepalive(&manager, dir.path()).expect("upsert");
        let record = manager
            .get_automation(OPERATE_KEEPALIVE_ID)
            .expect("keepalive");
        assert_eq!(record.model.as_deref(), Some("GLM-5.3"));
        assert_eq!(record.mode.as_deref(), Some("operate"));
        assert_eq!(CWC_OPERATE_DEFAULT_WORKER_MODEL, "GLM-5.3-Flash");
        assert_eq!(OPERATE_WORKER_MODEL, "GLM-5.3-Flash");
        assert_eq!(OPERATE_MAX_WRITERS, 3);
    }
}
