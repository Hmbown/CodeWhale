//! Goal tools for the model-visible LLM-as-judge loop.
//!
//! The TUI already has a `/goal` command and passes its objective into the
//! engine prompt. This module keeps the runtime slice separate: a small
//! session-scoped state object plus tools the model can use to inspect and
//! close out that state.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

/// Shared reference to the current runtime goal.
pub type SharedGoalState = Arc<Mutex<GoalState>>;

/// Create an empty shared goal state.
#[must_use]
pub fn new_shared_goal_state() -> SharedGoalState {
    Arc::new(Mutex::new(GoalState::default()))
}

/// Create shared state seeded from the host goal surface with an explicit status.
#[must_use]
pub fn new_shared_goal_state_from_host_status(
    objective: Option<String>,
    token_budget: Option<u32>,
    status: GoalStatus,
) -> SharedGoalState {
    let mut state = GoalState::default();
    state.sync_from_host_status(objective.as_deref(), token_budget, status);
    Arc::new(Mutex::new(state))
}

/// Create shared state restored from a persisted goal record, keeping the
/// accumulated usage and continuation counters. See
/// [`GoalState::from_persisted`].
#[must_use]
pub fn new_shared_goal_state_from_persisted(
    objective: &str,
    token_budget: Option<u32>,
    status: GoalStatus,
    pause_reason: Option<GoalPauseReason>,
    tokens_used: u64,
    time_used_seconds: u64,
    continuation_count: u32,
) -> SharedGoalState {
    Arc::new(Mutex::new(GoalState::from_persisted(
        objective,
        token_budget,
        status,
        pause_reason,
        tokens_used,
        time_used_seconds,
        continuation_count,
    )))
}

/// A goal declaration stated in ordinary user prose rather than as a leading
/// `/goal <objective>` command.
///
/// This intentionally recognizes only a narrow, explicit `/goal` directive.
/// Ordinary long-running requests are not silently promoted to goals, and a
/// quoted multi-line transcript cannot authorize one through a later line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitGoalDirective {
    pub objective: String,
}

/// Extract an explicit natural-language `/goal` declaration from the user's
/// first non-empty line.
///
/// The provider never participates in this decision. That makes an explicit
/// declaration behave the same on DeepSeek, Zai, and every compatible route,
/// while keeping ordinary tasks and discussion *about* `/goal` untouched.
#[must_use]
pub fn explicit_goal_directive(input: &str) -> Option<ExplicitGoalDirective> {
    let line = input.lines().find(|line| !line.trim().is_empty())?.trim();
    let lower = line.to_ascii_lowercase();

    // Objective follows the directive.
    for pattern in [
        "make it your /goal to",
        "make this your /goal to",
        "make that your /goal to",
        "set your /goal to",
        "set my /goal to",
        "set the /goal to",
        "set /goal to",
        "your /goal is",
        "my /goal is",
        "the /goal is",
    ] {
        let Some(index) = lower.find(pattern) else {
            continue;
        };
        if !is_direct_goal_clause(&lower[..index]) {
            continue;
        }
        let objective = normalize_explicit_goal_objective(&line[index + pattern.len()..])?;
        return Some(ExplicitGoalDirective { objective });
    }

    // Objective precedes the marker: "make solving X your /goal".
    for marker in [" your /goal", " my /goal", " the /goal"] {
        let Some(marker_index) = lower.find(marker) else {
            continue;
        };
        let before_marker = &lower[..marker_index];
        let Some(make_index) = before_marker.rfind("make ") else {
            continue;
        };
        if !is_direct_goal_clause(&lower[..make_index]) {
            continue;
        }
        let objective =
            normalize_explicit_goal_objective(&line[make_index + "make ".len()..marker_index])?;
        if matches!(
            objective.to_ascii_lowercase().as_str(),
            "it" | "this" | "that"
        ) {
            continue;
        }
        return Some(ExplicitGoalDirective { objective });
    }

    None
}

fn is_direct_goal_clause(prefix: &str) -> bool {
    let prefix = prefix.trim_end();
    if prefix.is_empty() {
        return true;
    }
    [
        "please",
        "and",
        "then",
        "also",
        "now",
        "you to",
        "need to",
        "can you",
        "could you",
        "would you",
        "can we",
        "could we",
        "should",
        "must",
        "-",
        ";",
        ":",
        ",",
    ]
    .iter()
    .any(|ending| prefix.ends_with(ending))
}

fn normalize_explicit_goal_objective(raw: &str) -> Option<String> {
    let objective = raw
        .trim()
        .trim_start_matches([':', '-', '–', '—'])
        .trim()
        .trim_end_matches(['.', '!', '?'])
        .trim();
    (!objective.is_empty()).then(|| objective.to_string())
}

/// Operate's automatic goal: Operate turns an ordinary work prompt into the
/// session goal when no unfinished goal exists (docs/MODES.md, "Operate loop").
/// It feeds the same `GoalState::create` path as [`explicit_goal_directive`];
/// an explicit `/goal` declaration always wins, and Plan and Work never call
/// this.
///
/// The whole "is this real work?" rule lives here: the prompt is work when it
/// has at least [`OPERATE_GOAL_MIN_WORDS`] words, or opens (after "please")
/// with an imperative work verb and has at least three words. Greetings,
/// acknowledgements, and short questions stay chat. The objective is the whole
/// prompt, whitespace-collapsed and bounded so continuation prompts stay small;
/// the transcript still holds the full text.
#[must_use]
pub fn operate_goal_from_prompt(input: &str) -> Option<ExplicitGoalDirective> {
    let words: Vec<&str> = input.split_whitespace().collect();
    if words.len() < 3 {
        return None;
    }
    let normalize = |word: &str| {
        word.trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase()
    };
    let head = words
        .iter()
        .map(|word| normalize(word))
        .find(|word| word != "please")
        .unwrap_or_default();
    if OPERATE_CHAT_OPENERS.contains(&head.as_str()) {
        return None;
    }
    let first_line = input.lines().find(|line| !line.trim().is_empty())?.trim();
    if first_line.ends_with('?') && words.len() < OPERATE_GOAL_QUESTION_WORDS {
        return None;
    }
    if words.len() < OPERATE_GOAL_MIN_WORDS && !OPERATE_WORK_VERBS.contains(&head.as_str()) {
        return None;
    }
    let mut objective = words.join(" ");
    if objective.chars().count() > OPERATE_GOAL_MAX_OBJECTIVE_CHARS {
        objective = objective
            .chars()
            .take(OPERATE_GOAL_MAX_OBJECTIVE_CHARS)
            .collect::<String>()
            .trim_end()
            .to_string();
        objective.push('…');
    }
    Some(ExplicitGoalDirective { objective })
}

const OPERATE_GOAL_MIN_WORDS: usize = 8;
const OPERATE_GOAL_QUESTION_WORDS: usize = 12;
const OPERATE_GOAL_MAX_OBJECTIVE_CHARS: usize = 600;
const OPERATE_CHAT_OPENERS: &[&str] = &[
    "hi", "hello", "hey", "thanks", "thank", "ok", "okay", "yes", "no", "sure", "great", "cool",
    "nice", "lol",
];
const OPERATE_WORK_VERBS: &[&str] = &[
    "add",
    "build",
    "change",
    "clean",
    "convert",
    "create",
    "debug",
    "deploy",
    "design",
    "document",
    "extract",
    "finish",
    "fix",
    "implement",
    "improve",
    "integrate",
    "investigate",
    "make",
    "migrate",
    "move",
    "optimize",
    "port",
    "refactor",
    "remove",
    "rename",
    "replace",
    "resolve",
    "rewrite",
    "run",
    "ship",
    "split",
    "test",
    "update",
    "upgrade",
    "verify",
    "wire",
    "write",
];

/// Runtime status for a goal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GoalStatus {
    #[default]
    Active,
    Paused,
    Complete,
    Blocked,
}

impl GoalStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Complete => "complete",
            Self::Blocked => "blocked",
        }
    }
}

/// Why an otherwise unfinished goal is paused.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalPauseReason {
    User,
    Backoff,
    NoProgress,
    UsageLimit,
    BudgetLimit,
}

/// Whether a goal review is allowed to decide the judged contract.
///
/// Critical reviews fail closed and may satisfy the completion gate. Advisory
/// reviews are append-only context: malformed or negative advice must never
/// pause, block, or complete the goal.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalReviewRole {
    #[default]
    Critical,
    Advisory,
}

/// Best-effort review context kept separate from the judged completion
/// contract. Notes are append-only for the lifetime of one objective.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalAdvisoryNote {
    pub summary: String,
}

impl GoalPauseReason {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Backoff => "run limit",
            Self::NoProgress => "no progress",
            Self::UsageLimit => "usage limit",
            Self::BudgetLimit => "budget limit",
        }
    }
}

/// Session-local goal state. `Instant` stays runtime-only; snapshots expose
/// elapsed seconds so tool output remains serializable and stable.
#[derive(Debug, Clone, Default)]
pub struct GoalState {
    objective: Option<String>,
    token_budget: Option<u32>,
    status: Option<GoalStatus>,
    tokens_used: u64,
    time_used_seconds: u64,
    continuation_count: u32,
    started_at: Option<Instant>,
    finished_at: Option<Instant>,
    evidence: Option<String>,
    blocker: Option<String>,
    pause_reason: Option<GoalPauseReason>,
    completion_verification: Option<GoalCompletionVerification>,
    advisories: Vec<GoalAdvisoryNote>,
    last_gap_fingerprint: Option<String>,
    repeated_gap_count: u32,
}

impl GoalState {
    #[must_use]
    pub fn objective(&self) -> Option<&str> {
        self.objective.as_deref()
    }

    #[must_use]
    pub fn token_budget(&self) -> Option<u32> {
        self.token_budget
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == Some(GoalStatus::Active)
    }

    pub fn sync_from_host_status(
        &mut self,
        objective: Option<&str>,
        token_budget: Option<u32>,
        status: GoalStatus,
    ) {
        let objective = objective.map(str::trim).filter(|value| !value.is_empty());
        match objective {
            Some(objective) => {
                let changed = self.objective.as_deref() != Some(objective);
                let status_changed = self.status != Some(status);
                let resumed = !changed
                    && status == GoalStatus::Active
                    && self
                        .status
                        .is_some_and(|previous| previous != GoalStatus::Active);
                if changed {
                    self.objective = Some(objective.to_string());
                    self.token_budget = token_budget;
                    self.tokens_used = 0;
                    self.time_used_seconds = 0;
                    self.continuation_count = 0;
                    self.started_at = Some(Instant::now());
                    self.evidence = None;
                    self.blocker = None;
                    self.pause_reason = None;
                    self.completion_verification = None;
                    self.advisories.clear();
                    self.last_gap_fingerprint = None;
                    self.repeated_gap_count = 0;
                } else if self.token_budget != token_budget {
                    self.token_budget = token_budget;
                }

                if resumed {
                    self.evidence = None;
                    self.blocker = None;
                    self.pause_reason = None;
                    self.completion_verification = None;
                    self.last_gap_fingerprint = None;
                    self.repeated_gap_count = 0;
                }

                if changed || status_changed || self.status.is_none() {
                    self.status = Some(status);
                    self.pause_reason = if status == GoalStatus::Paused {
                        Some(GoalPauseReason::User)
                    } else {
                        None
                    };
                    self.finished_at = if status == GoalStatus::Active {
                        None
                    } else {
                        Some(Instant::now())
                    };
                }
            }
            None => self.clear(),
        }
    }

    pub fn create(
        &mut self,
        objective: String,
        token_budget: Option<u32>,
    ) -> Result<(), &'static str> {
        if self.objective.is_some() && self.status != Some(GoalStatus::Complete) {
            return Err(
                "An unfinished goal already exists. Complete or clear it before creating another.",
            );
        }
        self.objective = Some(objective);
        self.token_budget = token_budget;
        self.status = Some(GoalStatus::Active);
        self.tokens_used = 0;
        self.time_used_seconds = 0;
        self.continuation_count = 0;
        self.started_at = Some(Instant::now());
        self.finished_at = None;
        self.evidence = None;
        self.blocker = None;
        self.pause_reason = None;
        self.completion_verification = None;
        self.advisories.clear();
        self.last_gap_fingerprint = None;
        self.repeated_gap_count = 0;
        Ok(())
    }

    /// Restore goal state from a persisted runtime goal, keeping the
    /// accumulated usage and continuation counters.
    ///
    /// Unlike [`Self::sync_from_host_status`], which resets the counters
    /// whenever the objective changes, this constructor treats the persisted
    /// values as the authoritative history: the durable store owns them and
    /// the engine is rehydrating, not re-declaring, the goal. Evidence,
    /// blockers, and review notes are runtime-only and start empty; the
    /// durable loop re-derives them on the next pass.
    #[must_use]
    pub fn from_persisted(
        objective: &str,
        token_budget: Option<u32>,
        status: GoalStatus,
        pause_reason: Option<GoalPauseReason>,
        tokens_used: u64,
        time_used_seconds: u64,
        continuation_count: u32,
    ) -> Self {
        Self {
            objective: Some(objective.to_string()),
            token_budget,
            status: Some(status),
            tokens_used,
            time_used_seconds,
            continuation_count,
            started_at: Some(Instant::now()),
            finished_at: (status != GoalStatus::Active).then(Instant::now),
            evidence: None,
            blocker: None,
            pause_reason,
            completion_verification: None,
            advisories: Vec::new(),
            last_gap_fingerprint: None,
            repeated_gap_count: 0,
        }
    }

    pub fn record_usage(&mut self, token_delta: u64, time_delta_seconds: u64) {
        if self.is_active() {
            self.tokens_used = self.tokens_used.saturating_add(token_delta);
            self.time_used_seconds = self.time_used_seconds.saturating_add(time_delta_seconds);
        }
    }

    pub fn record_continuation(&mut self) {
        if self.is_active() {
            self.continuation_count = self.continuation_count.saturating_add(1);
        }
    }

    pub fn mark_complete(
        &mut self,
        evidence: String,
        mut verification: GoalCompletionVerification,
    ) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to complete.");
        }
        if self.status == Some(GoalStatus::Complete) || self.completion_verification.is_some() {
            return Err("The judged completion contract is already sealed and cannot be replaced.");
        }
        if verification.role != GoalReviewRole::Critical {
            return Err("An advisory review cannot complete the judged goal contract.");
        }
        verification.contract_fingerprint = completion_contract_fingerprint(
            self.objective.as_deref().unwrap_or_default(),
            &verification,
        );
        self.status = Some(GoalStatus::Complete);
        self.finished_at = Some(Instant::now());
        self.evidence = Some(evidence);
        self.blocker = None;
        self.pause_reason = None;
        self.completion_verification = Some(verification);
        Ok(())
    }

    pub fn record_advisory(&mut self, summary: String) -> Result<(), &'static str> {
        if !self.is_active() {
            return Err("Advisory notes require an active goal.");
        }
        const MAX_ADVISORY_NOTES: usize = 16;
        if self.advisories.len() == MAX_ADVISORY_NOTES {
            self.advisories.remove(0);
        }
        self.advisories.push(GoalAdvisoryNote { summary });
        Ok(())
    }

    pub fn record_not_achieved(
        &mut self,
        verification: GoalProgressVerification,
    ) -> Result<(), &'static str> {
        if !self.is_active() {
            return Err("Verifier progress requires an active goal.");
        }
        if verification.role == GoalReviewRole::Advisory {
            return self
                .record_advisory(format!("{}: {}", verification.check, verification.summary));
        }

        let fingerprint = gap_fingerprint(&verification.gaps)
            .ok_or("Critical not-achieved verification requires at least one concrete gap.")?;
        self.repeated_gap_count = if self.last_gap_fingerprint.as_deref() == Some(&fingerprint) {
            self.repeated_gap_count.saturating_add(1)
        } else {
            1
        };
        self.last_gap_fingerprint = Some(fingerprint);

        Ok(())
    }

    pub fn mark_blocked(&mut self, blocker: String) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to block.");
        }
        self.status = Some(GoalStatus::Blocked);
        self.finished_at = Some(Instant::now());
        self.blocker = Some(blocker);
        self.evidence = None;
        self.pause_reason = None;
        self.completion_verification = None;
        Ok(())
    }

    pub fn mark_paused(&mut self, reason: GoalPauseReason) -> Result<(), &'static str> {
        if self.objective.is_none() {
            return Err("No active goal exists to pause.");
        }
        self.status = Some(GoalStatus::Paused);
        self.finished_at = Some(Instant::now());
        self.pause_reason = Some(reason);
        self.evidence = None;
        self.blocker = None;
        self.completion_verification = None;
        Ok(())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn snapshot(&self) -> GoalSnapshot {
        // Once the goal is terminal, freeze elapsed at the finish time so the
        // sidebar timer (and any tool snapshot) stops growing after completion.
        let elapsed_seconds = match (self.started_at, self.finished_at) {
            (Some(started), Some(finished)) => {
                Some(finished.saturating_duration_since(started).as_secs())
            }
            (Some(started), None) => Some(started.elapsed().as_secs()),
            (None, _) => None,
        };
        GoalSnapshot {
            objective: self.objective.clone(),
            status: self
                .status
                .map(GoalStatus::as_str)
                .unwrap_or("none")
                .to_string(),
            token_budget: self.token_budget,
            tokens_used: self.tokens_used,
            time_used_seconds: self.time_used_seconds,
            continuation_count: self.continuation_count,
            elapsed_seconds,
            evidence: self.evidence.clone(),
            blocker: self.blocker.clone(),
            pause_reason: self.pause_reason,
            completion_verification: self.completion_verification.clone(),
            advisories: self.advisories.clone(),
            last_gap_fingerprint: self.last_gap_fingerprint.clone(),
            repeated_gap_count: self.repeated_gap_count,
        }
    }
}

/// Serializable tool output and prompt input for the current goal.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct GoalSnapshot {
    pub objective: Option<String>,
    pub status: String,
    pub token_budget: Option<u32>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuation_count: u32,
    pub elapsed_seconds: Option<u64>,
    pub evidence: Option<String>,
    pub blocker: Option<String>,
    pub pause_reason: Option<GoalPauseReason>,
    pub completion_verification: Option<GoalCompletionVerification>,
    pub advisories: Vec<GoalAdvisoryNote>,
    pub last_gap_fingerprint: Option<String>,
    pub repeated_gap_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalCompletionVerification {
    pub status: String,
    pub check: String,
    pub summary: String,
    #[serde(default)]
    pub role: GoalReviewRole,
    #[serde(default)]
    pub contract_fingerprint: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GoalProgressVerification {
    pub status: String,
    pub check: String,
    pub summary: String,
    #[serde(default)]
    pub role: GoalReviewRole,
    #[serde(default)]
    pub gaps: Vec<String>,
}

fn completion_contract_fingerprint(
    objective: &str,
    verification: &GoalCompletionVerification,
) -> String {
    let mut hasher = Sha256::new();
    for field in [
        objective.trim(),
        verification.status.trim(),
        verification.check.trim(),
        verification.summary.trim(),
    ] {
        hasher.update(field.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn gap_fingerprint(gaps: &[String]) -> Option<String> {
    let mut normalized = gaps
        .iter()
        .map(|gap| {
            gap.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .filter(|gap| !gap.is_empty())
        .collect::<Vec<_>>();
    normalized.sort_unstable();
    normalized.dedup();
    if normalized.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(b"codewhale-goal-gaps-v1\0");
    for gap in normalized {
        hasher.update(gap.as_bytes());
        hasher.update([0]);
    }
    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

impl GoalSnapshot {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.objective.is_some() && self.status == GoalStatus::Active.as_str()
    }

    #[must_use]
    pub fn from_thread_goal(goal: &codewhale_protocol::ThreadGoal) -> Self {
        let (status, pause_reason) = thread_goal_status_projection(goal.status.clone());
        Self {
            objective: Some(goal.objective.clone()),
            status: status.as_str().to_string(),
            token_budget: goal
                .token_budget
                .and_then(|value| u32::try_from(value.max(0)).ok()),
            tokens_used: u64::try_from(goal.tokens_used.max(0)).unwrap_or(u64::MAX),
            time_used_seconds: u64::try_from(goal.time_used_seconds.max(0)).unwrap_or(u64::MAX),
            continuation_count: u32::try_from(goal.continuation_count.max(0)).unwrap_or(u32::MAX),
            elapsed_seconds: None,
            evidence: None,
            blocker: None,
            pause_reason,
            completion_verification: None,
            advisories: Vec::new(),
            last_gap_fingerprint: None,
            repeated_gap_count: 0,
        }
    }
}

#[must_use]
pub fn thread_goal_status_projection(
    status: codewhale_protocol::ThreadGoalStatus,
) -> (GoalStatus, Option<GoalPauseReason>) {
    match status {
        codewhale_protocol::ThreadGoalStatus::Active => (GoalStatus::Active, None),
        codewhale_protocol::ThreadGoalStatus::Paused => {
            (GoalStatus::Paused, Some(GoalPauseReason::User))
        }
        codewhale_protocol::ThreadGoalStatus::Complete => (GoalStatus::Complete, None),
        codewhale_protocol::ThreadGoalStatus::Blocked => (GoalStatus::Blocked, None),
        codewhale_protocol::ThreadGoalStatus::UsageLimited => {
            (GoalStatus::Paused, Some(GoalPauseReason::UsageLimit))
        }
        codewhale_protocol::ThreadGoalStatus::BudgetLimited => {
            (GoalStatus::Paused, Some(GoalPauseReason::BudgetLimit))
        }
    }
}

/// Render the continuation prompt injected when a goal is still active after a
/// turn. This shows progress and lets the circuit breaker remain an
/// implementation detail rather than encouraging the model to spend the cap.
#[must_use]
pub fn render_continuation_prompt(snapshot: &GoalSnapshot, continuation_index: u32) -> String {
    let goal_json = serde_json::to_string_pretty(snapshot).unwrap_or_else(|_| "{}".to_string());
    format!(
        "{}\n\n## Active Goal State\n\n```json\n{}\n```\n\nContinuation pass #{}.\nIf a critical verifier finds remaining work, call `update_goal` with `status: \"not_achieved\"` and its concrete `verification.gaps`; repeated equivalent gap sets pause the loop for inspection instead of spending indefinitely. If the goal is complete, first run or cite a concrete verifier/check when one applies, then call `update_goal` with `status: \"complete\"`, concrete evidence, and `verification: {{\"status\":\"passed\",\"check\":\"...\",\"summary\":\"...\"}}`. For non-verifiable work (docs, research, writing), use `verification: {{\"status\":\"not_applicable\",\"check\":\"...\",\"summary\":\"...\"}}` with a clear rationale instead of fabricating a verifier receipt. If it is blocked, call `update_goal` with `status: \"blocked\"` and the blocker. Otherwise continue making progress toward the objective.",
        crate::prompts::GOAL_CONTINUATION_PROMPT.trim(),
        goal_json,
        continuation_index,
    )
}

fn lock_goal_state(
    state: &SharedGoalState,
) -> Result<std::sync::MutexGuard<'_, GoalState>, ToolError> {
    state
        .lock()
        .map_err(|_| ToolError::execution_failed("goal state lock poisoned"))
}

fn parse_token_budget(input: &Value) -> Result<Option<u32>, ToolError> {
    let Some(raw) = input.get("token_budget") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    let Some(value) = raw.as_u64() else {
        return Err(ToolError::invalid_input(
            "token_budget must be a non-negative integer",
        ));
    };
    u32::try_from(value)
        .map(Some)
        .map_err(|_| ToolError::invalid_input("token_budget is too large"))
}

fn parse_completion_verification(input: &Value) -> Result<GoalCompletionVerification, ToolError> {
    let Some(raw) = input.get("verification") else {
        return Err(ToolError::invalid_input(
            "verification is required when status is complete; run a verifier/check and pass verification: {status, check, summary}",
        ));
    };
    let verification: GoalCompletionVerification = serde_json::from_value(raw.clone())
        .map_err(|err| ToolError::invalid_input(format!("invalid verification: {err}")))?;
    let status = verification.status.trim();
    let normalized_status = match status {
        "passed" | "not_applicable" => status,
        other => {
            return Err(ToolError::invalid_input(format!(
                "verification.status must be 'passed' or 'not_applicable' before update_goal can mark a goal complete; got '{other}'"
            )));
        }
    };
    if verification.check.trim().is_empty() {
        return Err(ToolError::invalid_input("verification.check is required"));
    }
    if verification.summary.trim().is_empty() {
        return Err(ToolError::invalid_input("verification.summary is required"));
    }
    Ok(GoalCompletionVerification {
        status: normalized_status.to_string(),
        check: verification.check.trim().to_string(),
        summary: verification.summary.trim().to_string(),
        role: verification.role,
        contract_fingerprint: String::new(),
    })
}

fn parse_progress_verification(input: &Value) -> Result<GoalProgressVerification, ToolError> {
    let Some(raw) = input.get("verification") else {
        return Err(ToolError::invalid_input(
            "verification is required when status is not_achieved",
        ));
    };
    let mut verification: GoalProgressVerification = serde_json::from_value(raw.clone())
        .map_err(|err| ToolError::invalid_input(format!("invalid verification: {err}")))?;
    if verification.status.trim() != "not_achieved" {
        return Err(ToolError::invalid_input(
            "verification.status must be 'not_achieved' for progress review",
        ));
    }
    verification.check = verification.check.trim().to_string();
    verification.summary = verification.summary.trim().to_string();
    if verification.check.is_empty() {
        return Err(ToolError::invalid_input("verification.check is required"));
    }
    if verification.summary.is_empty() {
        return Err(ToolError::invalid_input("verification.summary is required"));
    }
    Ok(verification)
}

fn json_result(snapshot: &GoalSnapshot) -> Result<ToolResult, ToolError> {
    ToolResult::json(snapshot).map_err(|err| ToolError::execution_failed(err.to_string()))
}

fn require_root_goal_mutation(context: &ToolContext) -> Result<(), ToolError> {
    if context.owner_agent_id.is_some() {
        return Err(ToolError::invalid_input(
            "Goal lifecycle mutation is root-agent only; sub-agents may inspect the parent goal with get_goal.",
        ));
    }
    Ok(())
}

pub struct CreateGoalTool {
    goal_state: SharedGoalState,
}

impl CreateGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for CreateGoalTool {
    fn name(&self) -> &'static str {
        "create_goal"
    }

    fn description(&self) -> &'static str {
        "Create the session's one persistent goal: a completion objective Codewhale keeps working toward across turns until it is verified complete, blocked, or the user stops it. Call this only when the user explicitly asks to use `/goal`, make an objective the goal, or otherwise explicitly requests persistent goal tracking. When the request is explicit, call `create_goal` before doing the rest of the work; acknowledging it in prose is not sufficient. Never infer a goal from an ordinary task, its apparent length, a question, or a one-file edit. Keep the user's full objective, not a shortened one-turn version. Set token_budget only when the user explicitly provides one. Creating a goal shows the user a one-line receipt (they can /goal pause or /goal clear); do not also ask for confirmation. Only one unfinished goal exists at a time: complete or clear it before creating another."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "objective": {
                    "type": "string",
                    "description": "The full objective to pursue. Keep the complete user goal, not a shortened one-turn version."
                },
                "token_budget": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional soft token budget for the goal."
                }
            },
            "required": ["objective"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        require_root_goal_mutation(context)?;
        let objective = required_str(&input, "objective")?.trim().to_string();
        if objective.is_empty() {
            return Err(ToolError::invalid_input("objective cannot be empty"));
        }
        let token_budget = parse_token_budget(&input)?;
        let snapshot = {
            let mut state = lock_goal_state(&self.goal_state)?;
            state
                .create(objective, token_budget)
                .map_err(ToolError::invalid_input)?;
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

pub struct GetGoalTool {
    goal_state: SharedGoalState,
}

impl GetGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for GetGoalTool {
    fn name(&self) -> &'static str {
        "get_goal"
    }

    fn description(&self) -> &'static str {
        "Inspect the current runtime goal state, including objective, status, token budget, elapsed time, evidence, and blocker."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let snapshot = {
            let state = lock_goal_state(&self.goal_state)?;
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

pub struct UpdateGoalTool {
    goal_state: SharedGoalState,
}

impl UpdateGoalTool {
    #[must_use]
    pub fn new(goal_state: SharedGoalState) -> Self {
        Self { goal_state }
    }
}

#[async_trait]
impl ToolSpec for UpdateGoalTool {
    fn name(&self) -> &'static str {
        "update_goal"
    }

    fn description(&self) -> &'static str {
        "Update the runtime goal completion gate. Critical verification may seal one immutable completion contract. Advisory review is append-only context and never completes, blocks, or pauses the goal. Mark blocked when progress requires user input."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["complete", "blocked", "not_achieved", "advisory"],
                    "description": "Use complete only when a critical verifier proves the goal; not_achieved to record verifier gaps; blocked when meaningful progress cannot continue; advisory to append best-effort context without changing lifecycle state."
                },
                "evidence": {
                    "type": "string",
                    "description": "Required when status is complete. Briefly cite the proof that the goal is done."
                },
                "verification": {
                    "type": "object",
                    "description": "Required when status is complete or not_achieved. A verifier-as-judge receipt from a concrete check, such as Run action=\"verifiers\" or an equivalent project-specific gate.",
                    "properties": {
                        "status": {
                            "type": "string",
                            "enum": ["passed", "not_applicable", "not_achieved"],
                            "description": "Use passed when a concrete verifier/check succeeded; not_applicable when no automated verifier applies; not_achieved when the verifier found concrete remaining gaps."
                        },
                        "check": {
                            "type": "string",
                            "description": "The verifier/check that passed."
                        },
                        "summary": {
                            "type": "string",
                            "description": "Brief result summary from the verifier/check."
                        },
                        "role": {
                            "type": "string",
                            "enum": ["critical", "advisory"],
                            "description": "Critical reviews may satisfy the judged completion contract. Advisory reviews are fail-open and cannot complete it. Defaults to critical for compatibility."
                        },
                        "gaps": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Concrete remaining gaps. Required for critical not_achieved reviews; order and duplicate wording do not affect the stall fingerprint."
                        }
                    },
                    "required": ["status", "check", "summary"],
                    "additionalProperties": false
                },
                "blocker": {
                    "type": "string",
                    "description": "Required when status is blocked. Explain the condition preventing progress."
                },
                "advisory": {
                    "type": "string",
                    "description": "Required when status is advisory. Appended separately from the judged completion contract."
                }
            },
            "required": ["status"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        Vec::new()
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        require_root_goal_mutation(context)?;
        // #5123-class: `objective` used to be accepted and silently ignored
        // with a success receipt. The objective is immutable after
        // create_goal; fail fast and name the corrective path.
        if input
            .get("objective")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(ToolError::invalid_input(
                "update_goal cannot change the objective — it is immutable after create_goal. \
                 Mark the current goal complete or blocked, then create_goal with the new objective",
            ));
        }
        let status = required_str(&input, "status")?.trim().to_ascii_lowercase();
        let snapshot = {
            let mut state = lock_goal_state(&self.goal_state)?;
            match status.as_str() {
                "complete" => {
                    let evidence = input
                        .get("evidence")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if evidence.is_empty() {
                        return Err(ToolError::invalid_input(
                            "evidence is required when status is complete",
                        ));
                    }
                    let verification = parse_completion_verification(&input)?;
                    state
                        .mark_complete(evidence, verification)
                        .map_err(ToolError::invalid_input)?;
                }
                "blocked" => {
                    let blocker = input
                        .get("blocker")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if blocker.is_empty() {
                        return Err(ToolError::invalid_input(
                            "blocker is required when status is blocked",
                        ));
                    }
                    state
                        .mark_blocked(blocker)
                        .map_err(ToolError::invalid_input)?;
                }
                "not_achieved" => {
                    let verification = parse_progress_verification(&input)?;
                    state
                        .record_not_achieved(verification)
                        .map_err(ToolError::invalid_input)?;
                }
                "advisory" => {
                    let advisory = input
                        .get("advisory")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .unwrap_or_default()
                        .to_string();
                    if advisory.is_empty() {
                        return Err(ToolError::invalid_input(
                            "advisory is required when status is advisory",
                        ));
                    }
                    state
                        .record_advisory(advisory)
                        .map_err(ToolError::invalid_input)?;
                }
                other => {
                    return Err(ToolError::invalid_input(format!(
                        "unsupported goal status '{other}'; update_goal can only mark complete or blocked, record not_achieved verifier gaps, or append advisory context"
                    )));
                }
            }
            state.snapshot()
        };
        json_result(&snapshot)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    #[test]
    fn explicit_goal_directive_extracts_user_requested_objective() {
        let request = explicit_goal_directive(
            "hello - take over and make it your /goal to solve navier stokes",
        )
        .expect("explicit /goal request");
        assert_eq!(request.objective, "solve navier stokes");

        let request = explicit_goal_directive("Please make ship the release your /goal.")
            .expect("objective-before-marker request");
        assert_eq!(request.objective, "ship the release");

        let request = explicit_goal_directive("Could you set /goal to repair provider routing?")
            .expect("set /goal request");
        assert_eq!(request.objective, "repair provider routing");
    }

    #[test]
    fn explicit_goal_directive_rejects_ordinary_or_quoted_goal_discussion() {
        for input in [
            "solve navier stokes",
            "why didn't you make it your /goal to solve navier stokes?",
            "what does /goal do?",
            "review the /goal implementation",
            "see this transcript where /goal was ignored:\nhello - take over and make it your /goal to solve navier stokes",
        ] {
            assert_eq!(
                explicit_goal_directive(input),
                None,
                "must not activate from: {input}"
            );
        }
    }

    #[test]
    fn operate_goal_from_prompt_promotes_work_and_leaves_chat_alone() {
        let goal = operate_goal_from_prompt(
            "Migrate the settings loader to the new config crate\nand keep the old keys readable.",
        )
        .expect("multi-line work prompt");
        assert_eq!(
            goal.objective,
            "Migrate the settings loader to the new config crate and keep the old keys readable."
        );
        assert_eq!(
            operate_goal_from_prompt("Please fix the flaky CI test")
                .expect("imperative after please")
                .objective,
            "Please fix the flaky CI test"
        );
        assert!(
            operate_goal_from_prompt(
                "Could you look at why the provider table drops rows after a reload and repair it?"
            )
            .is_some(),
            "a long question is still work"
        );
        for chat in [
            "hi",
            "thanks, looks good",
            "ok go ahead",
            "hello there, how are you doing today my friend",
            "what does /goal do?",
            "why is the build slow?",
            "the tests",
        ] {
            assert_eq!(
                operate_goal_from_prompt(chat),
                None,
                "must stay chat: {chat}"
            );
        }
        let long = "fix ".repeat(400);
        let objective = operate_goal_from_prompt(&long).expect("bounded").objective;
        assert!(objective.chars().count() <= OPERATE_GOAL_MAX_OBJECTIVE_CHARS + 1);
        assert!(objective.ends_with('…'));
    }

    #[tokio::test]
    async fn update_goal_rejects_objective_knob_instead_of_ignoring_it() {
        // #5123-class: `objective` used to return a success receipt with no
        // behavior. It is immutable after create_goal; the knob is gone from
        // the schema and supplying it fails fast with the corrective path.
        let state = new_shared_goal_state();
        let ctx = ToolContext::new(".");
        let create = CreateGoalTool::new(state.clone());
        create
            .execute(json!({"objective": "ship the runtime slice"}), &ctx)
            .await
            .expect("create goal");

        let update = UpdateGoalTool::new(state.clone());
        let schema = update.input_schema();
        assert!(
            schema["properties"].get("objective").is_none(),
            "ignored objective knob must not be advertised: {schema}"
        );

        let err = update
            .execute(
                json!({"status": "blocked", "blocker": "x", "objective": "different goal"}),
                &ctx,
            )
            .await
            .expect_err("objective must not be silently ignored");
        let message = format!("{err}");
        assert!(message.contains("immutable"), "{message}");
        assert!(message.contains("create_goal"), "{message}");
        // The rejected call must not have mutated goal state.
        assert!(state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn create_get_and_complete_goal() {
        let state = new_shared_goal_state();
        let ctx = ToolContext::new(".");

        let create = CreateGoalTool::new(state.clone());
        let created = create
            .execute(
                json!({
                    "objective": "ship the runtime slice",
                    "token_budget": 1200
                }),
                &ctx,
            )
            .await
            .expect("create goal");
        assert!(created.success);
        let created_json: Value = serde_json::from_str(&created.content).expect("created json");
        assert_eq!(
            created_json.get("status").and_then(Value::as_str),
            Some("active")
        );

        let get = GetGoalTool::new(state.clone());
        let current = get.execute(json!({}), &ctx).await.expect("get goal");
        assert!(current.content.contains("ship the runtime slice"));
        let current_json: Value = serde_json::from_str(&current.content).expect("current json");
        assert_eq!(
            current_json.get("token_budget").and_then(Value::as_u64),
            Some(1200)
        );

        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "focused tests passed",
                    "verification": {
                        "status": "passed",
                        "check": "cargo test -p codewhale-tui goal_loop",
                        "summary": "focused tests passed"
                    }
                }),
                &ctx,
            )
            .await
            .expect("complete goal");
        let completed_json: Value =
            serde_json::from_str(&completed.content).expect("completed json");
        assert_eq!(
            completed_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert!(completed.content.contains("focused tests passed"));
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[test]
    fn unfinished_goal_replacement_fails_closed_without_mutating_state() {
        for status in [GoalStatus::Active, GoalStatus::Paused, GoalStatus::Blocked] {
            let mut state = GoalState::default();
            state.sync_from_host_status(
                Some("preserve the current objective"),
                Some(1_200),
                status,
            );
            state.record_usage(300, 12);
            state.record_continuation();
            let before = state.snapshot();

            let error = state
                .create("replace it silently".to_string(), Some(99))
                .expect_err("unfinished goal replacement must fail");

            assert!(
                error.contains("unfinished goal"),
                "status {status:?}: {error}"
            );
            assert_eq!(
                state.snapshot(),
                before,
                "status {status:?} must preserve the entire goal snapshot"
            );
        }
    }

    #[test]
    fn same_objective_goal_host_resume_clears_terminal_payloads_and_preserves_progress() {
        let mut blocked = GoalState::default();
        blocked
            .create("resume the release goal".to_string(), Some(4_000))
            .expect("create blocked fixture");
        blocked.record_usage(750, 44);
        blocked.record_continuation();
        blocked
            .mark_blocked("provider failed".to_string())
            .expect("block goal");

        blocked.sync_from_host_status(
            Some("resume the release goal"),
            Some(4_000),
            GoalStatus::Active,
        );

        let resumed = blocked.snapshot();
        assert_eq!(resumed.status, "active");
        assert_eq!(resumed.tokens_used, 750);
        assert_eq!(resumed.time_used_seconds, 44);
        assert_eq!(resumed.continuation_count, 1);
        assert_eq!(resumed.evidence, None);
        assert_eq!(resumed.blocker, None);
        assert_eq!(resumed.completion_verification, None);
        let prompt = render_continuation_prompt(&resumed, resumed.continuation_count);
        assert!(prompt.contains("\"blocker\": null"), "{prompt}");

        let mut completed = GoalState::default();
        completed
            .create("resume verified work".to_string(), None)
            .expect("create completed fixture");
        completed
            .mark_complete(
                "focused tests passed".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "cargo test".to_string(),
                    summary: "goal tests passed".to_string(),
                    ..Default::default()
                },
            )
            .expect("complete goal");

        completed.sync_from_host_status(Some("resume verified work"), None, GoalStatus::Active);
        let resumed = completed.snapshot();
        assert_eq!(resumed.status, "active");
        assert_eq!(resumed.evidence, None);
        assert_eq!(resumed.blocker, None);
        assert_eq!(resumed.completion_verification, None);
    }

    #[test]
    fn completed_goal_can_be_replaced_with_fresh_accounting() {
        let mut state = GoalState::default();
        state
            .create("finish the first objective".to_string(), Some(1_200))
            .expect("create first goal");
        state.record_usage(300, 12);
        state.record_continuation();
        state
            .mark_complete(
                "focused tests passed".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "cargo test".to_string(),
                    summary: "goal tests passed".to_string(),
                    ..Default::default()
                },
            )
            .expect("complete first goal");

        state
            .create("start the next objective".to_string(), Some(2_400))
            .expect("completed goal may be replaced");

        let snapshot = state.snapshot();
        assert_eq!(
            snapshot.objective.as_deref(),
            Some("start the next objective")
        );
        assert_eq!(snapshot.status, "active");
        assert_eq!(snapshot.token_budget, Some(2_400));
        assert_eq!(snapshot.tokens_used, 0);
        assert_eq!(snapshot.time_used_seconds, 0);
        assert_eq!(snapshot.continuation_count, 0);
        assert_eq!(snapshot.evidence, None);
        assert_eq!(snapshot.blocker, None);
        assert_eq!(snapshot.completion_verification, None);
    }

    #[tokio::test]
    async fn subagent_context_cannot_mutate_parent_goal() {
        let state = new_shared_goal_state_from_host_status(
            Some("keep root lifecycle authority".to_string()),
            Some(1_200),
            GoalStatus::Active,
        );
        let before = state.lock().expect("goal lock").snapshot();
        let child_context = ToolContext::new(".").with_owner_agent("agent_child", "child verifier");

        let create_error = CreateGoalTool::new(state.clone())
            .execute(
                json!({"objective": "replace the parent goal"}),
                &child_context,
            )
            .await
            .expect_err("child create_goal must fail");
        assert!(create_error.to_string().contains("root-agent only"));

        let update_error = UpdateGoalTool::new(state.clone())
            .execute(
                json!({"status": "blocked", "blocker": "child decided to stop"}),
                &child_context,
            )
            .await
            .expect_err("child update_goal must fail");
        assert!(update_error.to_string().contains("root-agent only"));

        assert_eq!(
            state.lock().expect("goal lock").snapshot(),
            before,
            "rejected child mutations must leave the parent goal unchanged"
        );
    }

    #[tokio::test]
    async fn update_goal_requires_completion_evidence() {
        let state = new_shared_goal_state_from_host_status(
            Some("prove completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "complete"}), &ToolContext::new("."))
            .await
            .expect_err("missing evidence should fail");

        assert!(err.to_string().contains("evidence is required"));
    }

    #[tokio::test]
    async fn update_goal_accepts_not_applicable_verification_for_non_verifiable_goals() {
        let state = new_shared_goal_state_from_host_status(
            Some("write the release notes".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        let completed = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "release notes drafted and reviewed in thread",
                    "verification": {
                        "status": "not_applicable",
                        "check": "no automated verifier applies",
                        "summary": "writing task completed with evidence in thread"
                    }
                }),
                &ToolContext::new("."),
            )
            .await
            .expect("non-verifiable goal should complete");

        let completed_json: Value =
            serde_json::from_str(&completed.content).expect("completed json");
        assert_eq!(
            completed_json.get("status").and_then(Value::as_str),
            Some("complete")
        );
        assert_eq!(
            completed_json
                .get("completion_verification")
                .and_then(|verification| verification.get("status"))
                .and_then(Value::as_str),
            Some("not_applicable")
        );
        assert!(!state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn update_goal_requires_passed_verification_to_complete() {
        let state = new_shared_goal_state_from_host_status(
            Some("prove completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        let err = update
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "all checks look good"
                }),
                &ToolContext::new("."),
            )
            .await
            .expect_err("missing verifier gate should fail");

        assert!(err.to_string().contains("verification is required"));
        assert!(state.lock().expect("goal lock").is_active());
    }

    #[tokio::test]
    async fn advisory_review_is_append_only_and_fail_open() {
        let state = new_shared_goal_state_from_host_status(
            Some("keep the judged contract authoritative".to_string()),
            None,
            GoalStatus::Active,
        );
        let update = UpdateGoalTool::new(state.clone());
        update
            .execute(
                json!({
                    "status": "advisory",
                    "advisory": "Consider a narrower compatibility test."
                }),
                &ToolContext::new("."),
            )
            .await
            .expect("advisory note");
        let result = state.lock().expect("goal lock").snapshot();

        assert_eq!(result.status, "active");
        assert_eq!(result.advisories.len(), 1);
        assert_eq!(
            result.advisories[0].summary,
            "Consider a narrower compatibility test."
        );
        assert!(result.completion_verification.is_none());
    }

    #[tokio::test]
    async fn advisory_verification_cannot_complete_goal() {
        let state = new_shared_goal_state_from_host_status(
            Some("require a critical judge".to_string()),
            None,
            GoalStatus::Active,
        );
        let err = UpdateGoalTool::new(state.clone())
            .execute(
                json!({
                    "status": "complete",
                    "evidence": "an advisor liked it",
                    "verification": {
                        "status": "passed",
                        "check": "advisory review",
                        "summary": "looks reasonable",
                        "role": "advisory"
                    }
                }),
                &ToolContext::new("."),
            )
            .await
            .expect_err("advisory completion must fail closed");

        assert!(err.to_string().contains("advisory review cannot complete"));
        assert!(state.lock().expect("goal lock").is_active());
    }

    #[test]
    fn judged_completion_contract_is_fingerprinted_and_immutable() {
        let mut state = GoalState::default();
        state
            .create("seal the release candidate".to_string(), None)
            .expect("create goal");
        state
            .mark_complete(
                "locked tests passed".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "cargo test --locked".to_string(),
                    summary: "all required tests passed".to_string(),
                    ..Default::default()
                },
            )
            .expect("seal judged contract");
        let sealed = state.snapshot();
        let fingerprint = &sealed
            .completion_verification
            .as_ref()
            .expect("completion contract")
            .contract_fingerprint;
        assert_eq!(fingerprint.len(), 64);

        let err = state
            .mark_complete(
                "replace the evidence".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "different check".to_string(),
                    summary: "different result".to_string(),
                    ..Default::default()
                },
            )
            .expect_err("sealed contract must be immutable");
        assert!(err.contains("already sealed"));
        assert_eq!(state.snapshot(), sealed);
    }

    fn not_achieved_review(role: GoalReviewRole, gaps: &[&str]) -> GoalProgressVerification {
        GoalProgressVerification {
            status: "not_achieved".to_string(),
            check: "critical verifier".to_string(),
            summary: "remaining work found".to_string(),
            role,
            gaps: gaps.iter().map(|gap| (*gap).to_string()).collect(),
        }
    }

    #[test]
    fn equivalent_gap_sets_have_one_stable_fingerprint() {
        let first = gap_fingerprint(&[
            "  Add   a regression test ".to_string(),
            "Fix provider copy".to_string(),
        ]);
        let reordered = gap_fingerprint(&[
            "fix PROVIDER copy".to_string(),
            "add a regression test".to_string(),
            "Add a regression test".to_string(),
        ]);
        assert_eq!(first, reordered);
        assert_eq!(first.expect("fingerprint").len(), 64);
    }

    #[test]
    fn changed_gaps_reset_stall_counter_and_advice_never_advances_it() {
        let mut state = GoalState::default();
        state
            .create("keep making measurable progress".to_string(), None)
            .expect("create goal");
        state
            .record_not_achieved(not_achieved_review(
                GoalReviewRole::Critical,
                &["first gap"],
            ))
            .expect("first critical review");
        state
            .record_not_achieved(not_achieved_review(
                GoalReviewRole::Critical,
                &["first gap"],
            ))
            .expect("repeat critical review");
        assert_eq!(state.snapshot().repeated_gap_count, 2);

        state
            .record_not_achieved(not_achieved_review(
                GoalReviewRole::Advisory,
                &["advisor-only concern"],
            ))
            .expect("advisory review is fail-open");
        let after_advice = state.snapshot();
        assert_eq!(after_advice.repeated_gap_count, 2);
        assert_eq!(after_advice.advisories.len(), 1);
        assert_eq!(after_advice.status, "active");

        state
            .record_not_achieved(not_achieved_review(
                GoalReviewRole::Critical,
                &["a different remaining gap"],
            ))
            .expect("changed critical review");
        let progressed = state.snapshot();
        assert_eq!(progressed.repeated_gap_count, 1);
        assert_eq!(progressed.status, "active");
    }

    #[tokio::test]
    async fn update_goal_rejects_model_resume() {
        let state = new_shared_goal_state_from_host_status(
            Some("pause remains host controlled".to_string()),
            None,
            GoalStatus::Paused,
        );
        let update = UpdateGoalTool::new(state);
        let err = update
            .execute(json!({"status": "active"}), &ToolContext::new("."))
            .await
            .expect_err("model resume should fail");

        assert!(err.to_string().contains("complete or blocked"));
    }

    #[test]
    fn paused_host_goal_is_not_active() {
        let state = new_shared_goal_state_from_host_status(
            Some("wait for user".to_string()),
            Some(42),
            GoalStatus::Paused,
        );
        let snapshot = state.lock().expect("goal lock").snapshot();

        assert_eq!(snapshot.status, "paused");
        assert_eq!(snapshot.token_budget, Some(42));
        assert_eq!(snapshot.pause_reason, Some(GoalPauseReason::User));
        assert!(!snapshot.is_active());
    }

    #[test]
    fn goal_state_projects_usage_and_continuations() {
        let state = new_shared_goal_state_from_host_status(
            Some("persist accounting".to_string()),
            Some(1_000),
            GoalStatus::Active,
        );
        {
            let mut goal = state.lock().expect("goal lock");
            goal.record_usage(300, 12);
            goal.record_continuation();
        }

        let snapshot = state.lock().expect("goal lock").snapshot();
        assert_eq!(snapshot.tokens_used, 300);
        assert_eq!(snapshot.time_used_seconds, 12);
        assert_eq!(snapshot.continuation_count, 1);
    }

    #[test]
    fn completed_goal_snapshot_freezes_elapsed() {
        // Regression: a completed goal's snapshot elapsed_seconds must not keep
        // growing. Before the fix, snapshot() always used started_at.elapsed(),
        // so a finished goal's elapsed kept ticking in the sidebar/tool output.
        let state = new_shared_goal_state_from_host_status(
            Some("freeze on completion".to_string()),
            None,
            GoalStatus::Active,
        );
        let first = {
            let mut goal = state.lock().expect("goal lock");
            goal.mark_complete(
                "evidence".to_string(),
                GoalCompletionVerification {
                    status: "passed".to_string(),
                    check: "cargo test".to_string(),
                    summary: "ok".to_string(),
                    ..Default::default()
                },
            )
            .expect("mark complete");
            goal.snapshot()
        };
        let elapsed_at_completion = first.elapsed_seconds.expect("elapsed present");

        // Sleep past a whole-second boundary. Under the old (buggy) code,
        // snapshot() returned started_at.elapsed().as_secs(), so this would
        // tick up by at least one second and the assertion below would fail.
        // With the freeze, the completed snapshot stays at the captured value.
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let second = state.lock().expect("goal lock").snapshot();
        assert_eq!(second.status, "complete");
        assert_eq!(
            second.elapsed_seconds,
            Some(elapsed_at_completion),
            "completed goal elapsed must be frozen, not keep ticking"
        );
    }

    #[test]
    fn protocol_thread_goal_converts_to_runtime_snapshot() {
        let snapshot = GoalSnapshot::from_thread_goal(&codewhale_protocol::ThreadGoal {
            thread_id: "thread-1".to_string(),
            goal_id: "goal-1".to_string(),
            objective: "Bridge the goal models".to_string(),
            status: codewhale_protocol::ThreadGoalStatus::Active,
            token_budget: Some(2_000),
            tokens_used: 750,
            time_used_seconds: 44,
            continuation_count: 3,
            created_at: 1,
            updated_at: 2,
        });

        assert_eq!(
            snapshot.objective.as_deref(),
            Some("Bridge the goal models")
        );
        assert_eq!(snapshot.status, "active");
        assert_eq!(snapshot.token_budget, Some(2_000));
        assert_eq!(snapshot.tokens_used, 750);
        assert_eq!(snapshot.time_used_seconds, 44);
        assert_eq!(snapshot.continuation_count, 3);
    }

    #[test]
    fn protocol_limit_statuses_keep_distinct_pause_reasons() {
        for (status, reason) in [
            (
                codewhale_protocol::ThreadGoalStatus::UsageLimited,
                GoalPauseReason::UsageLimit,
            ),
            (
                codewhale_protocol::ThreadGoalStatus::BudgetLimited,
                GoalPauseReason::BudgetLimit,
            ),
        ] {
            let (projected, projected_reason) = thread_goal_status_projection(status);
            assert_eq!(projected, GoalStatus::Paused);
            assert_eq!(projected_reason, Some(reason));
        }
    }

    #[test]
    fn continuation_prompt_includes_bound_and_goal_state() {
        let snapshot = GoalSnapshot {
            objective: Some("finish issue 2199".to_string()),
            status: "active".to_string(),
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            continuation_count: 0,
            elapsed_seconds: Some(5),
            evidence: None,
            blocker: None,
            pause_reason: None,
            completion_verification: None,
            ..Default::default()
        };

        let prompt = render_continuation_prompt(&snapshot, 2);
        assert!(prompt.contains("Goal Continuation"));
        assert!(prompt.contains("finish issue 2199"));
        assert!(prompt.contains("Continuation pass #2"));
    }

    #[test]
    fn update_goal_contract_treats_required_user_input_as_blocking() {
        let update = UpdateGoalTool::new(new_shared_goal_state());
        assert!(update.description().contains("requires user input"));
    }

    #[test]
    fn from_persisted_keeps_counters_that_sync_from_host_status_resets() {
        // Rehydration treats the durable record as history: the counters
        // survive, evidence starts empty, and only a terminal status carries
        // a finish time.
        let restored = GoalState::from_persisted(
            "ship the goal loop",
            Some(50_000),
            GoalStatus::Active,
            None,
            1_234,
            56,
            3,
        );
        assert_eq!(restored.objective.as_deref(), Some("ship the goal loop"));
        assert_eq!(restored.token_budget, Some(50_000));
        assert_eq!(restored.status, Some(GoalStatus::Active));
        assert_eq!(restored.tokens_used, 1_234);
        assert_eq!(restored.time_used_seconds, 56);
        assert_eq!(restored.continuation_count, 3);
        assert!(restored.started_at.is_some());
        assert!(restored.finished_at.is_none());
        assert!(restored.evidence.is_none());
        assert!(restored.blocker.is_none());
        assert!(restored.advisories.is_empty());

        let blocked = GoalState::from_persisted(
            "ship the goal loop",
            None,
            GoalStatus::Blocked,
            None,
            0,
            0,
            0,
        );
        assert!(blocked.finished_at.is_some());

        // The same objective through the host-status path keeps the counters
        // (it is not a re-declaration)…
        let mut state = restored;
        state.sync_from_host_status(Some("ship the goal loop"), Some(50_000), GoalStatus::Active);
        assert_eq!(state.tokens_used, 1_234);
        assert_eq!(state.continuation_count, 3);

        // …while a changed objective resets them — the contrast that makes
        // `from_persisted` necessary for rehydration.
        state.sync_from_host_status(Some("a different objective"), None, GoalStatus::Active);
        assert_eq!(state.tokens_used, 0);
        assert_eq!(state.time_used_seconds, 0);
        assert_eq!(state.continuation_count, 0);
    }
}
