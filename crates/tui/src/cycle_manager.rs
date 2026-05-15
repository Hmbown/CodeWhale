//! Checkpoint-restart cycle management for long-running sessions (issue #124).
//!
//! ## Why
//!
//! DeepSeek V4's empirical retrieval degradation begins around the 256K band
//! (paper Figure 9: 8K/0.90, 64K/0.87, 128K/0.85, 256K/0.76,
//! 512K/0.66, 1M/0.59). Lossy
//! summarization compaction creates a "Frankenstein" context — half verbatim,
//! half paraphrased — that the model cannot tell apart, so it treats the
//! summary as if it were verbatim and confabulates around the gaps.
//!
//! Checkpoint-restart fixes this by giving every cycle a *homogeneous* fresh
//! context: original system prompt, structured work state (checklist /
//! strategy / working set / sub-agent handles), and a model-curated free-form briefing of at
//! most ~3,000 tokens. The previous cycle is archived to disk in JSONL form
//! so a future `recall_archive` tool (issue #127) can search it on demand.
//!
//! ## Layers of carry-forward
//!
//! 1. **Auto-preserved** (deterministic, no agent judgment): the original
//!    system prompt, `SharedTodoList`, `SharedPlanState`, working-set paths,
//!    open sub-agent snapshots, mode / workspace / cwd, and the user's most
//!    recent unsent message.
//! 2. **Free-form briefing** (model-curated, wrapped as `<carry_forward>`):
//!    decisions made + why, constraints discovered, hypotheses being tested,
//!    approaches that failed, open questions. Tool output bytes, file
//!    contents, and step-by-step recaps explicitly do NOT belong here —
//!    they're either in the archive or recoverable from disk.
//!
//! ## Trigger
//!
//! - Token threshold: **768K** active input by default (~75% of the 1M window).
//!   This is a rare overflow safety net. The trigger is based on the next
//!   request's live input estimate, not lifetime summed API usage, with
//!   assistant-output and safety headroom considered against the model window.
//!   Optional soft seams at 192K/384K/576K are controlled by the opt-in layered
//!   context manager (#159).
//! - Phase guard: callers only invoke `should_advance_cycle` at clean turn
//!   boundaries (no in-flight tool, no streaming, no approval modal).
//! - Per-model defaults: `CycleConfig` can carry model-specific thresholds
//!   for `deepseek-v4-pro` and `deepseek-v4-flash`.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::client::DeepSeekClient;
use crate::llm_client::LlmClient;
use crate::models::{
    ContentBlock, Message, MessageRequest, SystemBlock, SystemPrompt, context_window_for_model,
};
use crate::tools::plan::{PlanSnapshot, SharedPlanState};
use crate::tools::subagent::{SharedSubAgentManager, SubAgentResult, SubAgentStatus};
use crate::tools::todo::{SharedTodoList, TodoListSnapshot};
use crate::working_set::WorkingSet;

/// JSONL header record emitted as the first line of an archived cycle file.
const CYCLE_ARCHIVE_SCHEMA_VERSION: u32 = 1;

/// Default token threshold at which a cycle boundary fires.
///
/// Bumped from 110K to 768K (~75% of 1M window). The layered context manager
/// (#159) can add opt-in soft seams at 192K/384K/576K; the hard cycle remains
/// a near-wall safety net.
pub const DEFAULT_CYCLE_THRESHOLD_TOKENS: usize = 768_000;

/// Default cap on the model-curated briefing block.
pub const DEFAULT_BRIEFING_MAX_TOKENS: usize = 3_000;

/// Conservative chars-per-token used to bound the briefing length to the
/// configured token cap. Matches `compaction::estimate_tokens` (~4 chars/token).
const APPROX_CHARS_PER_TOKEN: usize = 4;

/// Per-model cycle tuning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCycleConfig {
    /// Token threshold above which a cycle boundary fires.
    pub threshold_tokens: usize,
    /// Cap on the model-curated `<carry_forward>` briefing.
    pub briefing_max_tokens: usize,
}

impl Default for ModelCycleConfig {
    fn default() -> Self {
        Self {
            threshold_tokens: DEFAULT_CYCLE_THRESHOLD_TOKENS,
            briefing_max_tokens: DEFAULT_BRIEFING_MAX_TOKENS,
        }
    }
}

/// Top-level cycle configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CycleConfig {
    /// Whether checkpoint-restart cycles are enabled. Defaults to true.
    pub enabled: bool,
    /// Default token threshold; per-model overrides take precedence when present.
    pub threshold_tokens: usize,
    /// Default briefing cap; per-model overrides take precedence when present.
    pub briefing_max_tokens: usize,
    /// Per-model overrides keyed by model identifier (e.g. `deepseek-v4-pro`).
    pub per_model: HashMap<String, ModelCycleConfig>,
}

impl Default for CycleConfig {
    fn default() -> Self {
        let mut per_model: HashMap<String, ModelCycleConfig> = HashMap::new();
        per_model.insert("deepseek-v4-pro".to_string(), ModelCycleConfig::default());
        per_model.insert("deepseek-v4-flash".to_string(), ModelCycleConfig::default());
        Self {
            enabled: true,
            threshold_tokens: DEFAULT_CYCLE_THRESHOLD_TOKENS,
            briefing_max_tokens: DEFAULT_BRIEFING_MAX_TOKENS,
            per_model,
        }
    }
}

impl CycleConfig {
    /// Resolve the threshold for a given model (per-model override > default).
    #[must_use]
    pub fn threshold_for(&self, model: &str) -> usize {
        self.per_model
            .get(model)
            .map(|m| m.threshold_tokens)
            .unwrap_or(self.threshold_tokens)
    }

    /// Resolve the briefing-token cap for a given model.
    #[must_use]
    pub fn briefing_max_for(&self, model: &str) -> usize {
        self.per_model
            .get(model)
            .map(|m| m.briefing_max_tokens)
            .unwrap_or(self.briefing_max_tokens)
    }
}

/// Snapshot of a model-curated briefing produced at cycle handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleBriefing {
    /// 1-based cycle number this briefing closes (i.e. the cycle being archived).
    pub cycle: u32,
    /// UTC timestamp when the briefing turn completed.
    pub timestamp: DateTime<Utc>,
    /// Extracted contents of the `<carry_forward>` block.
    pub briefing_text: String,
    /// Approximate token count of `briefing_text`.
    pub token_estimate: usize,
}

/// Decide whether a cycle boundary should fire.
///
/// `active_input_tokens` is the estimated token count of the next request's
/// current input, including previous assistant/tool output that is now part of
/// the transcript. `reserved_response_headroom_tokens` is the max output budget
/// plus any provider safety headroom reserved for that next request. Lifetime
/// API usage is intentionally not used here because it repeatedly counts the
/// same stable prefix across requests.
///
/// `in_flight` is true when a tool is mid-execution, stream is open, or an
/// approval modal is pending — in those cases the caller must wait until the
/// next clean boundary.
#[must_use]
pub fn should_advance_cycle(
    active_input_tokens: u64,
    reserved_response_headroom_tokens: u64,
    model: &str,
    cfg: &CycleConfig,
    in_flight: bool,
) -> bool {
    if !cfg.enabled || in_flight {
        return false;
    }
    let threshold = cfg.threshold_for(model) as u64;
    if threshold == 0 {
        return false;
    }
    let trigger_floor = context_window_for_model(model)
        .map(|window| u64::from(window).saturating_sub(reserved_response_headroom_tokens))
        .map_or(threshold, |window_floor| threshold.min(window_floor));
    active_input_tokens >= trigger_floor
}

/// Roll-up of state that survives a cycle boundary deterministically.
///
/// Construction is cheap — borrow the live state, snapshot it once, render it
/// into a system block. The snapshot decouples rendering from any mutex held
/// by the engine.
#[derive(Debug, Clone, Default)]
pub struct StructuredState {
    pub mode_label: String,
    pub workspace: PathBuf,
    pub cwd: Option<PathBuf>,
    pub working_set_summary: Option<String>,
    pub todo_snapshot: Option<TodoListSnapshot>,
    pub plan_snapshot: Option<PlanSnapshot>,
    pub subagent_snapshots: Vec<SubAgentResult>,
}

impl StructuredState {
    /// Capture the current state. All locks are held only for the duration of
    /// the snapshot.
    pub async fn capture(
        mode_label: impl Into<String>,
        workspace: PathBuf,
        cwd: Option<PathBuf>,
        working_set: &WorkingSet,
        todos: &SharedTodoList,
        plan_state: &SharedPlanState,
        subagents: Option<&SharedSubAgentManager>,
    ) -> Self {
        let working_set_summary = working_set.summary_block(&workspace);

        let todo_snapshot = {
            let guard = todos.lock().await;
            let snap = guard.snapshot();
            if snap.items.is_empty() {
                None
            } else {
                Some(snap)
            }
        };

        let plan_snapshot = {
            let guard = plan_state.lock().await;
            if guard.is_empty() {
                None
            } else {
                Some(guard.snapshot())
            }
        };

        let subagent_snapshots = if let Some(handle) = subagents {
            let guard = handle.read().await;
            guard
                .list()
                .into_iter()
                .filter(|s| matches!(s.status, SubAgentStatus::Running))
                .collect()
        } else {
            Vec::new()
        };

        Self {
            mode_label: mode_label.into(),
            workspace,
            cwd,
            working_set_summary,
            todo_snapshot,
            plan_snapshot,
            subagent_snapshots,
        }
    }

    /// Render the structured state as a single system block. Returns `None`
    /// when there is nothing meaningful to carry forward (rare in practice —
    /// at least the workspace and mode are always present).
    #[must_use]
    pub fn to_system_block(&self) -> Option<String> {
        let mut out = String::new();
        out.push_str("## Cycle State (Auto-Preserved)\n\n");
        out.push_str(&format!("- Mode: `{}`\n", self.mode_label));
        out.push_str(&format!("- Workspace: `{}`\n", self.workspace.display()));
        if let Some(cwd) = self.cwd.as_ref() {
            out.push_str(&format!("- Cwd: `{}`\n", cwd.display()));
        }

        if self.todo_snapshot.is_some() || self.plan_snapshot.is_some() {
            out.push_str("\n### Work\n");
        }

        if let Some(todos) = self.todo_snapshot.as_ref() {
            out.push_str(&format!(
                "\nChecklist ({}% complete)\n",
                todos.completion_pct
            ));
            for item in &todos.items {
                let marker = match item.status {
                    crate::tools::todo::TodoStatus::Pending => "[ ]",
                    crate::tools::todo::TodoStatus::InProgress => "[~]",
                    crate::tools::todo::TodoStatus::Completed => "[x]",
                };
                out.push_str(&format!("- {marker} {}\n", item.content));
            }
        }

        if let Some(plan) = self.plan_snapshot.as_ref() {
            out.push_str("\nStrategy\n");
            if let Some(explanation) = plan.explanation.as_ref() {
                out.push_str(&format!("{explanation}\n\n"));
            }
            for item in &plan.items {
                let marker = match item.status {
                    crate::tools::plan::StepStatus::Pending => "[ ]",
                    crate::tools::plan::StepStatus::InProgress => "[~]",
                    crate::tools::plan::StepStatus::Completed => "[x]",
                };
                out.push_str(&format!("- {marker} {}\n", item.step));
            }
        }

        if !self.subagent_snapshots.is_empty() {
            out.push_str("\n### Open Sub-Agents\n");
            for s in &self.subagent_snapshots {
                let role = s.assignment.role.as_deref().unwrap_or("—");
                let goal = if s.assignment.objective.is_empty() {
                    "(no objective set)"
                } else {
                    s.assignment.objective.as_str()
                };
                out.push_str(&format!("- `{}` (role: {}) — {}\n", s.agent_id, role, goal));
            }
        }

        if let Some(working_set) = self.working_set_summary.as_deref() {
            out.push('\n');
            out.push_str(working_set);
            out.push('\n');
        }

        Some(out)
    }
}

/// Build the prompt the model uses to produce its `<carry_forward>` briefing.
pub const CYCLE_HANDOFF_TEMPLATE: &str = include_str!("prompts/cycle_handoff.md");

/// Run the briefing turn. The caller drives this just before swapping the
/// session message buffer. The returned text is the contents of the
/// `<carry_forward>` block — outer tags stripped, length-bounded to
/// `max_briefing_tokens` worth of characters as a defensive backstop in case
/// the model ignores the cap.
pub async fn produce_briefing(
    client: &DeepSeekClient,
    model: &str,
    conversation: &[Message],
    max_briefing_tokens: usize,
) -> Result<String> {
    if conversation.is_empty() {
        return Ok(String::new());
    }

    // Append a synthetic instruction asking for the carry_forward block. We
    // do not mutate the caller's conversation; this is a one-shot turn.
    let mut messages: Vec<Message> = conversation.to_vec();
    messages.push(Message {
        role: "user".to_string(),
        content: vec![ContentBlock::Text {
            text: format!(
                "[CYCLE BOUNDARY] {}\n\nProduce your `<carry_forward>` block now. \
                 Stay under {} tokens. Output only the block — no other text.",
                "The next turn starts in a fresh context.", max_briefing_tokens
            ),
            cache_control: None,
        }],
    });

    let request = MessageRequest {
        model: model.to_string(),
        messages,
        max_tokens: u32::try_from(max_briefing_tokens.saturating_mul(2))
            .unwrap_or(8_192)
            .max(1_024),
        system: Some(SystemPrompt::Blocks(vec![SystemBlock {
            block_type: "text".to_string(),
            text: CYCLE_HANDOFF_TEMPLATE.to_string(),
            cache_control: None,
        }])),
        tools: None,
        tool_choice: None,
        metadata: None,
        thinking: None,
        reasoning_effort: None,
        stream: Some(false),
        // Briefings benefit from low temperature — we want consistent state
        // capture, not stylistic variation.
        temperature: Some(0.2),
        top_p: None,
    };

    let response = client
        .create_message(request)
        .await
        .with_context(|| format!("Cycle briefing turn failed for model {model}"))?;
    // Cycle briefing calls are billed; route through the side-channel
    // (#526) so the footer total matches the DeepSeek website.
    crate::cost_status::report(&response.model, &response.usage);

    let raw = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let extracted = extract_carry_forward(&raw);
    let bounded = enforce_briefing_cap(&extracted, max_briefing_tokens);
    Ok(bounded)
}

/// Pull the contents of the first `<carry_forward>...</carry_forward>` block
/// out of the raw model response. If the tags are missing, return the trimmed
/// raw text — the caller would rather have *some* briefing than nothing.
#[must_use]
pub fn extract_carry_forward(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let open_tag = "<carry_forward>";
    let close_tag = "</carry_forward>";

    if let Some(start) = lower.find(open_tag) {
        let after = start + open_tag.len();
        let tail = &raw[after..];
        let tail_lower = &lower[after..];
        if let Some(end) = tail_lower.find(close_tag) {
            return tail[..end].trim().to_string();
        }
        // Open tag without close tag — take everything after, trimmed.
        return tail.trim().to_string();
    }
    raw.trim().to_string()
}

/// Defensive bound on briefing length. Calibrated at ~4 chars/token to match
/// the rest of the codebase's token estimator.
fn enforce_briefing_cap(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens.saturating_mul(APPROX_CHARS_PER_TOKEN);
    if max_chars == 0 {
        return String::new();
    }
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n\n[...briefing truncated to fit cap...]");
    out
}

/// Estimate briefing tokens — same method as `compaction::estimate_tokens`
/// for symmetry: ~4 chars per token.
#[must_use]
pub fn estimate_briefing_tokens(text: &str) -> usize {
    text.len().div_ceil(APPROX_CHARS_PER_TOKEN)
}

/// Header record written as the first line of an archived cycle JSONL file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CycleArchiveHeader {
    pub schema_version: u32,
    pub cycle: u32,
    pub session_id: String,
    pub model: String,
    pub started: DateTime<Utc>,
    pub ended: DateTime<Utc>,
    pub message_count: usize,
}

/// Resolve the on-disk archive directory: `~/.deepseek/sessions/<id>/cycles`.
fn archive_dir_for(session_id: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not resolve home directory for cycle archive")?;
    Ok(home
        .join(".deepseek")
        .join("sessions")
        .join(session_id)
        .join("cycles"))
}

/// Archive a cycle's messages to JSONL on disk and return the path written.
///
/// The first line is a `CycleArchiveHeader` JSON object; each subsequent
/// line is a single `Message` serialized as JSON.
pub fn archive_cycle(
    session_id: &str,
    cycle_n: u32,
    messages: &[Message],
    model: &str,
    started: DateTime<Utc>,
) -> Result<PathBuf> {
    let dir = archive_dir_for(session_id)?;
    std::fs::create_dir_all(&dir).with_context(|| {
        format!(
            "Failed to create cycle archive directory at {}",
            dir.display()
        )
    })?;

    let path = dir.join(format!("{cycle_n}.jsonl"));
    let header = CycleArchiveHeader {
        schema_version: CYCLE_ARCHIVE_SCHEMA_VERSION,
        cycle: cycle_n,
        session_id: session_id.to_string(),
        model: model.to_string(),
        started,
        ended: Utc::now(),
        message_count: messages.len(),
    };

    write_archive_file(&path, &header, messages)
        .with_context(|| format!("Failed to write cycle archive at {}", path.display()))?;

    Ok(path)
}

fn write_archive_file(
    path: &Path,
    header: &CycleArchiveHeader,
    messages: &[Message],
) -> Result<()> {
    let tmp_path = path.with_extension("jsonl.tmp");
    {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        let mut buf = std::io::BufWriter::new(file);
        let header_line = serde_json::to_string(header)?;
        buf.write_all(header_line.as_bytes())?;
        buf.write_all(b"\n")?;
        for message in messages {
            let line = serde_json::to_string(message)?;
            buf.write_all(line.as_bytes())?;
            buf.write_all(b"\n")?;
        }
        // BufWriter flushes on drop, but we want any error surfaced now —
        // not silently into the void.
        buf.flush()?;
        // File handle drops with `buf`.
    }
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Open an archived cycle JSONL for streaming reads. Returns the parsed
/// header and an iterator over messages. Reserved for the future
/// `recall_archive` tool (#127).
#[allow(dead_code)]
pub fn open_archive(path: &Path) -> Result<(CycleArchiveHeader, ArchiveMessageReader)> {
    use std::io::{BufRead, BufReader};

    let file = File::open(path)
        .with_context(|| format!("Failed to open cycle archive at {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut header_line = String::new();
    reader.read_line(&mut header_line)?;
    let header: CycleArchiveHeader =
        serde_json::from_str(header_line.trim()).with_context(|| {
            format!(
                "Cycle archive at {} is missing a valid header",
                path.display()
            )
        })?;

    if header.schema_version > CYCLE_ARCHIVE_SCHEMA_VERSION {
        anyhow::bail!(
            "Cycle archive schema v{} at {} is newer than supported v{}",
            header.schema_version,
            path.display(),
            CYCLE_ARCHIVE_SCHEMA_VERSION
        );
    }

    Ok((header, ArchiveMessageReader { reader }))
}

/// Iterator yielding `Message`s from an opened archive file. Yields `None`
/// when the file is exhausted. Errors propagate through the `Result`.
#[allow(dead_code)]
#[derive(Debug)]
pub struct ArchiveMessageReader {
    reader: std::io::BufReader<File>,
}

#[allow(dead_code)]
impl Iterator for ArchiveMessageReader {
    type Item = Result<Message>;

    fn next(&mut self) -> Option<Self::Item> {
        use std::io::BufRead;

        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return self.next();
                }
                Some(
                    serde_json::from_str::<Message>(trimmed)
                        .map_err(|e| anyhow::anyhow!("Archive line parse failed: {e}")),
                )
            }
            Err(e) => Some(Err(anyhow::Error::new(e))),
        }
    }
}

/// Compose the seed messages for the next cycle.
///
/// Layout (deterministic order):
///
/// 1. (system prompt is provided separately, not as a `Message`)
/// 2. Optional structured-state user message (todos / plan / working set /
///    sub-agents) — labeled with `[CYCLE STATE]` so the assistant can tell
///    it apart from a real user turn.
/// 3. The model-curated `<carry_forward>` briefing — labeled with `[CYCLE
///    BRIEFING]` so the assistant knows it was self-authored on the previous
///    cycle.
/// 4. Optional pending user message that hadn't been sent yet.
///
/// The original system prompt is composed by the engine and stays separate
/// from this list — the engine sets `session.system_prompt` directly.
#[must_use]
pub fn build_seed_messages(
    structured_state_block: Option<&str>,
    briefing: Option<&CycleBriefing>,
    pending_user_message: Option<&str>,
) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::new();

    if let Some(state) = structured_state_block
        && !state.trim().is_empty()
    {
        out.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "[CYCLE STATE — auto-preserved across the cycle boundary]\n\n{}",
                    state.trim()
                ),
                cache_control: None,
            }],
        });
        // A user message expects an assistant ack so the next real user
        // message lands on a clean alternation. We synthesize a one-line ack.
        out.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Acknowledged. State carried into the new cycle.".to_string(),
                cache_control: None,
            }],
        });
    }

    if let Some(brief) = briefing
        && !brief.briefing_text.trim().is_empty()
    {
        out.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: format!(
                    "[CYCLE BRIEFING — written by you on cycle {} at {}]\n\n<carry_forward>\n{}\n</carry_forward>",
                    brief.cycle,
                    brief.timestamp.to_rfc3339(),
                    brief.briefing_text.trim()
                ),
                cache_control: None,
            }],
        });
        out.push(Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: "Briefing absorbed. Continuing.".to_string(),
                cache_control: None,
            }],
        });
    }

    if let Some(pending) = pending_user_message
        && !pending.trim().is_empty()
    {
        out.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: pending.trim().to_string(),
                cache_control: None,
            }],
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ContentBlock, Message};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn user_msg(text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn asst_msg(text: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn cycle_config_default_includes_v4_overrides() {
        let cfg = CycleConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.per_model.contains_key("deepseek-v4-pro"));
        assert!(cfg.per_model.contains_key("deepseek-v4-flash"));
        assert_eq!(cfg.threshold_tokens, DEFAULT_CYCLE_THRESHOLD_TOKENS);
        assert_eq!(cfg.briefing_max_tokens, DEFAULT_BRIEFING_MAX_TOKENS);
    }

    #[test]
    fn threshold_for_falls_back_to_default() {
        let cfg = CycleConfig::default();
        assert_eq!(
            cfg.threshold_for("deepseek-v4-pro"),
            DEFAULT_CYCLE_THRESHOLD_TOKENS
        );
        assert_eq!(
            cfg.threshold_for("unknown-model"),
            DEFAULT_CYCLE_THRESHOLD_TOKENS
        );
    }

    #[test]
    fn threshold_for_uses_per_model_override() {
        let mut cfg = CycleConfig::default();
        cfg.per_model.insert(
            "deepseek-v4-pro".to_string(),
            ModelCycleConfig {
                threshold_tokens: 80_000,
                briefing_max_tokens: 2_000,
            },
        );
        assert_eq!(cfg.threshold_for("deepseek-v4-pro"), 80_000);
        assert_eq!(cfg.briefing_max_for("deepseek-v4-pro"), 2_000);
    }

    #[test]
    fn should_advance_below_threshold_returns_false() {
        let cfg = CycleConfig::default();
        assert!(!should_advance_cycle(
            50_000,
            0,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
    }

    #[test]
    fn should_advance_at_threshold_returns_true() {
        let cfg = CycleConfig::default();
        assert!(should_advance_cycle(
            DEFAULT_CYCLE_THRESHOLD_TOKENS as u64,
            0,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
    }

    #[test]
    fn should_advance_considers_output_plus_safety_headroom() {
        let cfg = CycleConfig::default();
        // Below the 768K active-input threshold, but too close to the 1M
        // model window once the next assistant response and safety headroom are
        // included.
        assert!(should_advance_cycle(
            737_000,
            263_168,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
    }

    #[test]
    fn should_not_count_lifetime_api_usage_as_active_context() {
        let cfg = CycleConfig::default();
        assert!(!should_advance_cycle(
            120_000,
            64_000,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
    }

    #[test]
    fn should_advance_v4_calibrates_threshold_against_output_reserve() {
        let cfg = CycleConfig::default();
        let reserve = 263_168;
        assert!(!should_advance_cycle(
            700_000,
            reserve,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
        assert!(should_advance_cycle(
            738_000,
            reserve,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
        assert!(should_advance_cycle(
            768_000,
            reserve,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
        assert!(should_advance_cycle(
            900_000,
            reserve,
            "deepseek-v4-pro",
            &cfg,
            false
        ));
    }

    #[test]
    fn in_flight_phase_guard_blocks_advance() {
        let cfg = CycleConfig::default();
        assert!(!should_advance_cycle(
            DEFAULT_CYCLE_THRESHOLD_TOKENS as u64 * 2,
            0,
            "deepseek-v4-pro",
            &cfg,
            true,
        ));
    }

    #[test]
    fn disabled_config_blocks_advance() {
        let cfg = CycleConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!should_advance_cycle(
            DEFAULT_CYCLE_THRESHOLD_TOKENS as u64 * 2,
            0,
            "deepseek-v4-pro",
            &cfg,
            false,
        ));
    }

    #[test]
    fn extract_carry_forward_pulls_block() {
        let raw = "Here is your handoff:\n<carry_forward>\nDecision A: chose X because Y.\n</carry_forward>\nDone.";
        assert_eq!(extract_carry_forward(raw), "Decision A: chose X because Y.");
    }

    #[test]
    fn extract_carry_forward_handles_missing_close_tag() {
        let raw = "<carry_forward>\nDecision A: chose X.";
        // Missing close tag → returns the tail, trimmed.
        assert_eq!(extract_carry_forward(raw), "Decision A: chose X.");
    }

    #[test]
    fn extract_carry_forward_no_tags_returns_trimmed_body() {
        let raw = "  Decision A: chose X.  ";
        assert_eq!(extract_carry_forward(raw), "Decision A: chose X.");
    }

    #[test]
    fn extract_carry_forward_case_insensitive() {
        let raw = "<CARRY_FORWARD>\nState here.\n</CARRY_FORWARD>";
        assert_eq!(extract_carry_forward(raw), "State here.");
    }

    #[test]
    fn enforce_briefing_cap_truncates_oversized_text() {
        let max_tokens = 10; // 10 * 4 = 40 chars
        let big = "x".repeat(200);
        let bounded = enforce_briefing_cap(&big, max_tokens);
        assert!(bounded.starts_with(&"x".repeat(40)));
        assert!(bounded.contains("[...briefing truncated"));
    }

    #[test]
    fn enforce_briefing_cap_passes_short_text_through() {
        let txt = "hello world";
        assert_eq!(enforce_briefing_cap(txt, 100), "hello world");
    }

    #[test]
    fn build_seed_messages_empty_when_all_inputs_empty() {
        let seeds = build_seed_messages(None, None, None);
        assert!(seeds.is_empty());
    }

    #[test]
    fn build_seed_messages_includes_state_briefing_and_pending() {
        let briefing = CycleBriefing {
            cycle: 1,
            timestamp: Utc::now(),
            briefing_text: "Decisions: chose A.".to_string(),
            token_estimate: 5,
        };

        let seeds = build_seed_messages(
            Some("## Cycle State\n- Mode: agent"),
            Some(&briefing),
            Some("Continue working on issue #124"),
        );

        // Expected layout: state user + ack assistant + briefing user + ack assistant + pending user.
        assert_eq!(seeds.len(), 5);
        assert_eq!(seeds[0].role, "user");
        assert_eq!(seeds[1].role, "assistant");
        assert_eq!(seeds[2].role, "user");
        assert_eq!(seeds[3].role, "assistant");
        assert_eq!(seeds[4].role, "user");

        if let ContentBlock::Text { text, .. } = &seeds[0].content[0] {
            assert!(text.contains("[CYCLE STATE"));
            assert!(text.contains("agent"));
        } else {
            panic!("expected text block");
        }
        if let ContentBlock::Text { text, .. } = &seeds[2].content[0] {
            assert!(text.contains("[CYCLE BRIEFING"));
            assert!(text.contains("<carry_forward>"));
            assert!(text.contains("Decisions: chose A."));
        } else {
            panic!("expected text block");
        }
        if let ContentBlock::Text { text, .. } = &seeds[4].content[0] {
            assert_eq!(text, "Continue working on issue #124");
        } else {
            panic!("expected text block");
        }
    }

    #[test]
    fn build_seed_messages_skips_blank_pending() {
        let seeds = build_seed_messages(Some("## State"), None, Some("   "));
        // State block + ack — no pending message.
        assert_eq!(seeds.len(), 2);
        assert_eq!(seeds[0].role, "user");
        assert_eq!(seeds[1].role, "assistant");
    }

    #[test]
    fn structured_state_to_system_block_renders_minimal() {
        let state = StructuredState {
            mode_label: "agent".to_string(),
            workspace: PathBuf::from("/tmp/ws"),
            cwd: None,
            working_set_summary: None,
            todo_snapshot: None,
            plan_snapshot: None,
            subagent_snapshots: Vec::new(),
        };
        let block = state.to_system_block().expect("renders");
        assert!(block.contains("Mode: `agent`"));
        assert!(block.contains("Workspace: `/tmp/ws`"));
    }

    #[test]
    fn structured_state_to_system_block_unifies_work_state() {
        let state = StructuredState {
            mode_label: "agent".to_string(),
            workspace: PathBuf::from("/tmp/ws"),
            cwd: None,
            working_set_summary: None,
            todo_snapshot: Some(TodoListSnapshot {
                items: vec![crate::tools::todo::TodoItem {
                    id: 1,
                    content: "Run focused tests".to_string(),
                    status: crate::tools::todo::TodoStatus::InProgress,
                }],
                completion_pct: 0,
                in_progress_id: Some(1),
            }),
            plan_snapshot: Some(PlanSnapshot {
                explanation: Some("Keep sidebar state unified".to_string()),
                items: vec![crate::tools::plan::PlanItemArg {
                    step: "Update prompts".to_string(),
                    status: crate::tools::plan::StepStatus::Pending,
                }],
            }),
            subagent_snapshots: Vec::new(),
        };

        let block = state.to_system_block().expect("renders");

        assert!(block.contains("### Work"));
        assert!(block.contains("Checklist (0% complete)"));
        assert!(block.contains("Strategy"));
        assert!(!block.contains("### Plan"));
        assert!(!block.contains("### Todos"));
    }

    #[test]
    fn archive_cycle_writes_jsonl_with_header_and_messages() {
        let dir = tempdir().expect("tempdir");
        let session_id = format!("test-session-{}", uuid::Uuid::new_v4());

        // Redirect dirs::home_dir() into our tempdir. On Unix that reads
        // HOME; on Windows it reads USERPROFILE — set both so the test is
        // platform-portable. SAFETY: cargo runs each test binary
        // single-threaded by default; we do not await across the env
        // mutation window.
        let original_home = std::env::var("HOME").ok();
        let original_userprofile = std::env::var("USERPROFILE").ok();
        unsafe {
            std::env::set_var("HOME", dir.path());
            std::env::set_var("USERPROFILE", dir.path());
        }

        let messages = vec![
            user_msg("hello"),
            asst_msg("hi"),
            user_msg("can you read Cargo.toml?"),
        ];

        let started = Utc::now();
        let path = archive_cycle(&session_id, 1, &messages, "deepseek-v4-pro", started)
            .expect("archive_cycle should succeed");

        assert!(path.exists(), "archive file should exist on disk");
        assert_eq!(path.file_name().and_then(|s| s.to_str()), Some("1.jsonl"));

        let contents = std::fs::read_to_string(&path).expect("read archive back");
        let mut lines = contents.lines();

        let header_line = lines.next().expect("header line present");
        let header: CycleArchiveHeader = serde_json::from_str(header_line).expect("header parses");
        assert_eq!(header.cycle, 1);
        assert_eq!(header.session_id, session_id);
        assert_eq!(header.model, "deepseek-v4-pro");
        assert_eq!(header.message_count, 3);
        assert_eq!(header.schema_version, CYCLE_ARCHIVE_SCHEMA_VERSION);

        for expected in &messages {
            let line = lines.next().expect("message line present");
            let parsed: Message = serde_json::from_str(line).expect("message parses");
            assert_eq!(&parsed, expected);
        }
        assert!(lines.next().is_none(), "no extra trailing lines");

        // Restore env so subsequent tests aren't surprised.
        unsafe {
            match original_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match original_userprofile {
                Some(value) => std::env::set_var("USERPROFILE", value),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }

    #[test]
    fn open_archive_rejects_newer_schema_version() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("999.jsonl");
        let header = CycleArchiveHeader {
            schema_version: CYCLE_ARCHIVE_SCHEMA_VERSION + 5,
            cycle: 999,
            session_id: "future-session".to_string(),
            model: "deepseek-v9".to_string(),
            started: Utc::now(),
            ended: Utc::now(),
            message_count: 0,
        };
        let mut payload = serde_json::to_string(&header).unwrap();
        payload.push('\n');
        std::fs::write(&path, payload).unwrap();

        let err = open_archive(&path).expect_err("must reject newer schema version");
        let msg = format!("{err:#}");
        assert!(msg.contains("newer than supported"), "got: {msg}");
    }

    /// Mock `produce_briefing`-style flow purely client-side: we feed a known
    /// raw string through `extract_carry_forward` + `enforce_briefing_cap`
    /// and assert the same result we'd produce after a real LLM call.
    /// Avoids spinning up a live mock server while still proving the
    /// extraction contract.
    #[test]
    fn briefing_extraction_pipeline_preserves_block() {
        let raw = "thinking: ok\n<carry_forward>\nDecision: pick lib A; constraint: no async.\n</carry_forward>\n";
        let extracted = extract_carry_forward(raw);
        let bounded = enforce_briefing_cap(&extracted, 50);
        assert_eq!(bounded, "Decision: pick lib A; constraint: no async.");
    }
}
