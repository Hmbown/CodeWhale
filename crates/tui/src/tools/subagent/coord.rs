//! Narrow model-facing agent coordination tools.
//!
//! Keeps `agent` as the creation surface. These five tools wrap existing
//! SubAgentManager / mailbox / checkpoint machinery without restoring the
//! retired lifecycle theater (`agent_open` / `agent_eval` / …).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    COMPLETED_AGENT_RETENTION, SharedSubAgentManager, SubAgentRuntime, SubAgentStatus,
    parse_agent_ref, subagent_session_projection, subagent_status_name,
    wait_for_subagents_from_input,
};
use crate::tools::registry::ToolRegistryBuilder;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

const COORD_WAIT_DEFAULT_TIMEOUT_SECS: u64 = 300;
const COORD_WAIT_MIN_TIMEOUT_SECS: u64 = 1;
const COORD_WAIT_MAX_TIMEOUT_SECS: u64 = 1800;
const COORD_WAIT_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const RECENT_PROGRESS_LIMIT: usize = 8;
pub(super) const COORDINATION_RECORD_LIMIT: usize = 128;
const COORDINATION_INSPECT_LIMIT: usize = 24;
pub(super) const COORDINATION_PROJECTION_DECISION_LIMIT: usize = 8;
pub(super) const COORDINATION_PROJECTION_BYTE_LIMIT: usize = 4096;

// ── agents/list ──────────────────────────────────────────────────────────

pub struct AgentsListTool {
    manager: SharedSubAgentManager,
}

impl AgentsListTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentsListTool {
    fn name(&self) -> &'static str {
        "agents/list"
    }

    fn description(&self) -> &'static str {
        "List child agents: ids, parent hierarchy, state, bounded recent progress, and token budget. Read-only coordination view — does not spawn or wake workers."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "include_archived": {
                    "type": "boolean",
                    "description": "Include prior-session / archived agents. Default false."
                },
                "agent_id": {
                    "type": "string",
                    "description": "Optional single agent id or session name to inspect."
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn is_read_only_for(&self, _input: &Value) -> bool {
        true
    }

    fn supports_parallel_for(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let include_archived = input
            .get("include_archived")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let agent_ref = parse_agent_ref(&input);

        let mut manager = self.manager.write().await;
        manager.cleanup(COMPLETED_AGENT_RETENTION);
        let summaries = if let Some(agent_ref) = agent_ref {
            let summary = manager
                .coordination_summary_for(&agent_ref, RECENT_PROGRESS_LIMIT)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            vec![summary]
        } else {
            manager.list_coordination_summaries(include_archived, RECENT_PROGRESS_LIMIT)
        };
        drop(manager);

        let payload = json!({
            "action": "list",
            "count": summaries.len(),
            "agents": summaries,
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": "list",
            "count": summaries.len(),
        }));
        Ok(tool_result)
    }
}

// ── agents/message ───────────────────────────────────────────────────────

pub struct AgentsMessageTool {
    manager: SharedSubAgentManager,
    caller_agent_id: Option<String>,
}

impl AgentsMessageTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self {
            manager,
            caller_agent_id: None,
        }
    }

    #[must_use]
    pub fn with_caller(mut self, caller_agent_id: impl Into<String>) -> Self {
        self.caller_agent_id = Some(caller_agent_id.into());
        self
    }
}

#[async_trait]
impl ToolSpec for AgentsMessageTool {
    fn name(&self) -> &'static str {
        "agents/message"
    }

    fn description(&self) -> &'static str {
        "Queue a parent message onto a child agent without waking it. The child receives the message on the next followup or natural resume. Use agents/followup when you also need to resume an idle or interrupted child."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Target child agent id or session name."
                },
                "message": {
                    "type": "string",
                    "description": "Message text to queue."
                }
            },
            "required": ["agent_id", "message"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_ref =
            parse_agent_ref(&input).ok_or_else(|| ToolError::missing_field("agent_id"))?;
        let message = input
            .get("message")
            .or_else(|| input.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::missing_field("message"))?
            .to_string();

        let receipt = {
            let mut manager = self.manager.write().await;
            manager
                .ensure_caller_controls_descendant(
                    &agent_ref,
                    self.caller_agent_id.as_deref(),
                    "agents/message",
                )
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            manager
                .queue_parent_message(&agent_ref, message, false)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?
        };

        let payload = json!({
            "action": "message",
            "agent_id": receipt.agent_id,
            "queued": true,
            "woke": false,
            "queue_depth": receipt.queue_depth,
            "status": receipt.status,
            "note": "Message queued without waking the child.",
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": "message",
            "agent_id": receipt.agent_id,
            "woke": false,
            "queue_depth": receipt.queue_depth,
        }));
        Ok(tool_result)
    }
}

// ── agents/followup ──────────────────────────────────────────────────────

pub struct AgentsFollowupTool {
    manager: SharedSubAgentManager,
    caller_agent_id: Option<String>,
}

impl AgentsFollowupTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self {
            manager,
            caller_agent_id: None,
        }
    }

    #[must_use]
    pub fn with_caller(mut self, caller_agent_id: impl Into<String>) -> Self {
        self.caller_agent_id = Some(caller_agent_id.into());
        self
    }
}

#[async_trait]
impl ToolSpec for AgentsFollowupTool {
    fn name(&self) -> &'static str {
        "agents/followup"
    }

    fn description(&self) -> &'static str {
        "Queue a message and attempt to resume an idle or interrupted child. Running children receive the message on their next step; interrupted_continuable children keep a checkpoint and return the continuation_handle — live in-place resume is not automated yet (re-dispatch via agent)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Target child agent id or session name."
                },
                "message": {
                    "type": "string",
                    "description": "Follow-up message text."
                }
            },
            "required": ["agent_id", "message"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_ref =
            parse_agent_ref(&input).ok_or_else(|| ToolError::missing_field("agent_id"))?;
        let message = input
            .get("message")
            .or_else(|| input.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::missing_field("message"))?
            .to_string();

        let receipt = {
            let mut manager = self.manager.write().await;
            manager
                .ensure_caller_controls_descendant(
                    &agent_ref,
                    self.caller_agent_id.as_deref(),
                    "agents/followup",
                )
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            manager
                .followup_child(&agent_ref, message)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?
        };

        let payload = json!({
            "action": "followup",
            "agent_id": receipt.agent_id,
            "queued": true,
            "woke": receipt.woke,
            "queue_depth": receipt.queue_depth,
            "status": receipt.status,
            "continued_from_checkpoint": receipt.continued_from_checkpoint,
            "continuation_handle": receipt.continuation_handle,
            "note": receipt.note,
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": "followup",
            "agent_id": receipt.agent_id,
            "woke": receipt.woke,
            "continued_from_checkpoint": receipt.continued_from_checkpoint,
            "continuation_handle": receipt.continuation_handle,
        }));
        Ok(tool_result)
    }
}

// ── agents/interrupt ─────────────────────────────────────────────────────

pub struct AgentsInterruptTool {
    manager: SharedSubAgentManager,
    /// Optional caller identity for fail-closed self-interrupt checks.
    caller_agent_id: Option<String>,
}

impl AgentsInterruptTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self {
            manager,
            caller_agent_id: None,
        }
    }

    #[must_use]
    #[allow(dead_code)] // arms self-interrupt fail-closed when child registries thread caller (P1.2)
    pub fn with_caller(mut self, caller_agent_id: impl Into<String>) -> Self {
        self.caller_agent_id = Some(caller_agent_id.into());
        self
    }
}

#[async_trait]
impl ToolSpec for AgentsInterruptTool {
    fn name(&self) -> &'static str {
        "agents/interrupt"
    }

    fn description(&self) -> &'static str {
        "Interrupt a running child agent, preserve its checkpoint, and return the prior state. Fails closed on root or self targets. Prefer this over cancel when you may resume later."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Child agent id or session name to interrupt."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional interrupt reason recorded on the checkpoint."
                }
            },
            "required": ["agent_id"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let agent_ref =
            parse_agent_ref(&input).ok_or_else(|| ToolError::missing_field("agent_id"))?;
        let reason = input
            .get("reason")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("interrupted by parent via agents/interrupt")
            .to_string();

        let (prior, snapshot) = {
            let mut manager = self.manager.write().await;
            manager
                .interrupt_child(&agent_ref, self.caller_agent_id.as_deref(), reason)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?
        };

        let worker_record = {
            let manager = self.manager.read().await;
            manager.get_worker_record(&snapshot.agent_id)
        };
        let projection = subagent_session_projection(snapshot, false, context, worker_record).await;
        let payload = json!({
            "action": "interrupt",
            "agent_id": projection.agent_id,
            "prior_status": subagent_status_name(&prior.status),
            "prior_steps_taken": prior.steps_taken,
            "status": projection.status,
            "checkpoint_preserved": projection.checkpoint.is_some(),
            "continuable": projection.continuable,
            "projection": projection,
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({
            "action": "interrupt",
            "agent_id": payload["agent_id"],
            "checkpoint_preserved": payload["checkpoint_preserved"],
        }));
        Ok(tool_result)
    }
}

// ── agents/wait ──────────────────────────────────────────────────────────

pub struct AgentsWaitTool {
    manager: SharedSubAgentManager,
}

impl AgentsWaitTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolSpec for AgentsWaitTool {
    fn name(&self) -> &'static str {
        "agents/wait"
    }

    fn description(&self) -> &'static str {
        "Block until a child shows activity, settles (completion/failure/interrupt), or the timeout elapses. Prefer one wait over polling agents/list. until=completion (default) waits for settle; until=activity returns on progress or settle."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "Optional specific child. When omitted, waits for the next watched child event."
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1800,
                    "description": "Maximum seconds to block. Default 300."
                },
                "until": {
                    "type": "string",
                    "enum": ["completion", "activity"],
                    "description": "completion (default): return when a child leaves running. activity: also return when recent progress changes."
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn is_read_only_for(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let until = input
            .get("until")
            .and_then(Value::as_str)
            .unwrap_or("completion")
            .trim()
            .to_ascii_lowercase();

        if until == "completion" || until.is_empty() {
            let mut wait_input = input.clone();
            if wait_input.get("action").is_none() {
                wait_input["action"] = json!("wait");
            }
            return wait_for_subagents_from_input(&wait_input, Arc::clone(&self.manager), context)
                .await;
        }

        if until != "activity" {
            return Err(ToolError::invalid_input(format!(
                "Invalid until '{until}'. Use completion or activity."
            )));
        }

        wait_for_activity(&input, Arc::clone(&self.manager), context).await
    }
}

async fn wait_for_activity(
    input: &Value,
    manager: SharedSubAgentManager,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let timeout_secs = input
        .get("timeout_secs")
        .or_else(|| input.get("timeout"))
        .and_then(Value::as_u64)
        .unwrap_or(COORD_WAIT_DEFAULT_TIMEOUT_SECS)
        .clamp(COORD_WAIT_MIN_TIMEOUT_SECS, COORD_WAIT_MAX_TIMEOUT_SECS);
    let timeout = Duration::from_secs(timeout_secs);
    let agent_ref = parse_agent_ref(input);

    let (watched, baseline): (Vec<String>, Vec<(String, u64)>) = {
        let manager = manager.read().await;
        if let Some(agent_ref) = &agent_ref {
            let snap = manager
                .get_result_by_ref(agent_ref)
                .map_err(|err| ToolError::invalid_input(err.to_string()))?;
            let fp = manager.activity_fingerprint(&snap.agent_id).unwrap_or(0);
            if snap.status != SubAgentStatus::Running {
                let payload = json!({
                    "action": "wait",
                    "until": "activity",
                    "reason": "already_settled",
                    "timed_out": false,
                    "agent_id": snap.agent_id,
                    "status": subagent_status_name(&snap.status),
                });
                let mut tool_result = ToolResult::json(&payload)
                    .map_err(|err| ToolError::execution_failed(err.to_string()))?;
                tool_result.metadata = Some(json!({ "action": "wait", "timed_out": false }));
                return Ok(tool_result);
            }
            (vec![snap.agent_id.clone()], vec![(snap.agent_id, fp)])
        } else {
            let running = manager
                .list_filtered(false)
                .into_iter()
                .filter(|s| s.status == SubAgentStatus::Running)
                .map(|s| s.agent_id)
                .collect::<Vec<_>>();
            let baseline = running
                .iter()
                .map(|id| {
                    let fp = manager.activity_fingerprint(id).unwrap_or(0);
                    (id.clone(), fp)
                })
                .collect();
            (running, baseline)
        }
    };

    if watched.is_empty() {
        let payload = json!({
            "action": "wait",
            "until": "activity",
            "note": "No running sub-agents; nothing to wait for.",
            "timed_out": false,
        });
        let mut tool_result = ToolResult::json(&payload)
            .map_err(|err| ToolError::execution_failed(err.to_string()))?;
        tool_result.metadata = Some(json!({ "action": "wait", "timed_out": false }));
        return Ok(tool_result);
    }

    let started = Instant::now();
    let cancelled = async {
        match &context.cancel_token {
            Some(token) => token.cancelled().await,
            None => std::future::pending().await,
        }
    };
    tokio::pin!(cancelled);

    loop {
        let outcome = {
            let manager = manager.read().await;
            let mut settled = Vec::new();
            let mut activity = Vec::new();
            for (id, base_fp) in &baseline {
                if let Ok(snap) = manager.get_result_by_ref(id) {
                    if snap.status != SubAgentStatus::Running {
                        settled.push(snap);
                        continue;
                    }
                    let fp = manager.activity_fingerprint(id).unwrap_or(0);
                    if fp != *base_fp {
                        activity.push(json!({
                            "agent_id": id,
                            "status": "running",
                            "activity_fingerprint": fp,
                        }));
                    }
                }
            }
            (settled, activity, manager.running_count())
        };

        if !outcome.0.is_empty() || !outcome.1.is_empty() {
            let payload = json!({
                "action": "wait",
                "until": "activity",
                "settled": outcome.0.iter().map(|s| json!({
                    "agent_id": s.agent_id,
                    "status": subagent_status_name(&s.status),
                })).collect::<Vec<_>>(),
                "activity": outcome.1,
                "running": outcome.2,
                "elapsed_ms": started.elapsed().as_millis(),
                "timed_out": false,
            });
            let mut tool_result = ToolResult::json(&payload)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            tool_result.metadata = Some(json!({
                "action": "wait",
                "timed_out": false,
                "settled": outcome.0.len(),
                "activity": outcome.1.len(),
            }));
            return Ok(tool_result);
        }

        if started.elapsed() >= timeout {
            let payload = json!({
                "action": "wait",
                "until": "activity",
                "settled": [],
                "activity": [],
                "running": outcome.2,
                "elapsed_ms": started.elapsed().as_millis(),
                "timed_out": true,
                "note": "Timed out before child activity or completion.",
            });
            let mut tool_result = ToolResult::json(&payload)
                .map_err(|err| ToolError::execution_failed(err.to_string()))?;
            tool_result.metadata = Some(json!({ "action": "wait", "timed_out": true }));
            return Ok(tool_result);
        }

        tokio::select! {
            biased;
            () = &mut cancelled => {
                return Err(ToolError::cancelled(
                    "Wait interrupted by user cancellation before child activity.".to_string(),
                ));
            }
            () = tokio::time::sleep(COORD_WAIT_CHECK_INTERVAL) => {}
        }
    }
}

/// Register the narrow coordination tools alongside `agent`.
pub fn register_coordination_tools(
    builder: ToolRegistryBuilder,
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
) -> ToolRegistryBuilder {
    // `runtime.parent_agent_id` is the identity of the agent this registry is
    // being built FOR: `runtime_for_nested_agent_tools` stamps the child's own
    // id there before `new_with_owner` registers tools, so anything that agent
    // spawns records it as parent. Thread that identity through every mutating
    // hierarchy tool: a child may control only its own descendants, while the
    // root registry (`None`) may control any child (TUI-DOG-017).
    let caller = runtime.parent_agent_id.clone();
    let message = caller.as_deref().map_or_else(
        || AgentsMessageTool::new(Arc::clone(&manager)),
        |caller| AgentsMessageTool::new(Arc::clone(&manager)).with_caller(caller),
    );
    let followup = caller.as_deref().map_or_else(
        || AgentsFollowupTool::new(Arc::clone(&manager)),
        |caller| AgentsFollowupTool::new(Arc::clone(&manager)).with_caller(caller),
    );
    let interrupt = caller.as_deref().map_or_else(
        || AgentsInterruptTool::new(Arc::clone(&manager)),
        |caller| AgentsInterruptTool::new(Arc::clone(&manager)).with_caller(caller),
    );
    let coordinate = AgentsCoordinateTool::new(Arc::clone(&manager), caller);
    builder
        .with_tool(Arc::new(AgentsListTool::new(Arc::clone(&manager))))
        .with_tool(Arc::new(message))
        .with_tool(Arc::new(followup))
        .with_tool(Arc::new(interrupt))
        .with_tool(Arc::new(coordinate))
        .with_tool(Arc::new(AgentsWaitTool::new(manager)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolContext;
    use tempfile::tempdir;

    async fn manager_with_running_child(
        workspace: &std::path::Path,
    ) -> (SharedSubAgentManager, String) {
        let manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(workspace.to_path_buf(), 4),
        ));
        let agent_id = {
            let mut guard = manager.write().await;
            guard.insert_test_running_agent("coord_child", workspace)
        };
        (manager, agent_id)
    }

    async fn manager_with_agent_hierarchy(
        workspace: &std::path::Path,
    ) -> (SharedSubAgentManager, String, String, String) {
        let manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(workspace.to_path_buf(), 8),
        ));
        let (parent, child, sibling) = {
            let mut guard = manager.write().await;
            let parent = guard.insert_test_running_agent("hierarchy_parent", workspace);
            let child = guard.insert_test_running_agent("hierarchy_child", workspace);
            let sibling = guard.insert_test_running_agent("hierarchy_sibling", workspace);
            for (agent_id, parent_id) in [
                (&parent, "root"),
                (&child, parent.as_str()),
                (&sibling, "root"),
            ] {
                let record = guard
                    .worker_records
                    .get_mut(agent_id)
                    .expect("hierarchy worker record");
                record.parent_run_id = Some(parent_id.to_string());
                record.spec.parent_run_id = Some(parent_id.to_string());
            }
            (parent, child, sibling)
        };
        (manager, parent, child, sibling)
    }

    #[tokio::test]
    async fn message_queues_without_waking() {
        let tmp = tempdir().unwrap();
        let (manager, agent_id) = manager_with_running_child(tmp.path()).await;
        let tool = AgentsMessageTool::new(Arc::clone(&manager));
        let result = tool
            .execute(
                json!({ "agent_id": agent_id, "message": "hold this" }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect("message ok");
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["woke"], json!(false));
        assert_eq!(body["queued"], json!(true));
        assert_eq!(body["queue_depth"], json!(1));

        let guard = manager.read().await;
        let depth = guard.queued_mail_depth(&agent_id).unwrap();
        assert_eq!(depth, 1);
        assert!(!guard.child_was_woken(&agent_id));
    }

    #[tokio::test]
    async fn hierarchy_mutations_allow_own_descendants_and_deny_siblings_or_ancestors() {
        let tmp = tempdir().unwrap();
        let (manager, parent, child, sibling) = manager_with_agent_hierarchy(tmp.path()).await;
        let context = ToolContext::new(tmp.path());

        AgentsMessageTool::new(Arc::clone(&manager))
            .with_caller(parent.clone())
            .execute(
                json!({ "agent_id": child, "message": "bounded parent note" }),
                &context,
            )
            .await
            .expect("parent may message its own child");
        AgentsFollowupTool::new(Arc::clone(&manager))
            .with_caller(parent.clone())
            .execute(
                json!({ "agent_id": child, "message": "resume own child" }),
                &context,
            )
            .await
            .expect("parent may follow up its own child");

        let sibling_message = AgentsMessageTool::new(Arc::clone(&manager))
            .with_caller(parent.clone())
            .execute(
                json!({ "agent_id": sibling, "message": "cross branch" }),
                &context,
            )
            .await
            .expect_err("sibling message must fail closed")
            .to_string();
        assert!(
            sibling_message.contains("own descendants"),
            "{sibling_message}"
        );

        let ancestor_followup = AgentsFollowupTool::new(Arc::clone(&manager))
            .with_caller(child.clone())
            .execute(
                json!({ "agent_id": parent, "message": "wake ancestor" }),
                &context,
            )
            .await
            .expect_err("ancestor followup must fail closed")
            .to_string();
        assert!(
            ancestor_followup.contains("own descendants"),
            "{ancestor_followup}"
        );

        let sibling_interrupt = AgentsInterruptTool::new(Arc::clone(&manager))
            .with_caller(parent.clone())
            .execute(json!({ "agent_id": sibling }), &context)
            .await
            .expect_err("sibling interrupt must fail closed")
            .to_string();
        assert!(
            sibling_interrupt.contains("own descendants"),
            "{sibling_interrupt}"
        );

        let interrupted = AgentsInterruptTool::new(Arc::clone(&manager))
            .with_caller(parent)
            .execute(json!({ "agent_id": child }), &context)
            .await
            .expect("parent may interrupt its own child");
        let body: Value = serde_json::from_str(&interrupted.content).unwrap();
        assert_eq!(body["status"], json!("interrupted"));
    }

    #[tokio::test]
    async fn coordinate_inspect_is_side_effect_free_and_mutations_are_synchronously_durable() {
        let tmp = tempdir().unwrap();
        let blocked_state_path = tmp.path().join("blocked-state");
        std::fs::create_dir(&blocked_state_path).unwrap();
        let blocked_manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(tmp.path().to_path_buf(), 4)
                .with_state_path(blocked_state_path),
        ));
        let blocked_tool = AgentsCoordinateTool::new(Arc::clone(&blocked_manager), None);

        blocked_tool
            .execute(
                json!({ "action": "inspect" }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect("read-only inspect must not attempt persistence");
        let error = blocked_tool
            .execute(
                json!({
                    "action": "propose",
                    "decision_id": "durable-decision",
                    "subject": "durability",
                    "constraints": ["persist before acknowledgement"]
                }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect_err("mutation must fail when its receipt cannot persist")
            .to_string();
        assert!(error.contains("failed to persist"), "{error}");
        assert!(
            blocked_manager
                .read()
                .await
                .coordination
                .decisions
                .is_empty(),
            "failed persistence must roll the in-memory decision back"
        );

        let durable_workspace = tempdir().unwrap();
        let state_path = durable_workspace.path().join("subagents.v1.json");
        let manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(durable_workspace.path().to_path_buf(), 4)
                .with_state_path(state_path.clone()),
        ));
        AgentsCoordinateTool::new(Arc::clone(&manager), None)
            .execute(
                json!({
                    "action": "propose",
                    "decision_id": "durable-decision",
                    "subject": "durability",
                    "constraints": ["persist before acknowledgement"]
                }),
                &ToolContext::new(durable_workspace.path()),
            )
            .await
            .expect("durable mutation");
        let mut replayed =
            super::super::SubAgentManager::new(durable_workspace.path().to_path_buf(), 4)
                .with_state_path(state_path);
        replayed.load_state().expect("reload durable action");
        assert_eq!(replayed.coordination.decisions.len(), 1);
        assert_eq!(
            replayed.coordination.decisions[0].decision_id,
            "durable-decision"
        );
    }

    #[tokio::test]
    async fn rejected_claim_contention_is_persisted_before_returning_the_error() {
        let tmp = tempdir().unwrap();
        let state_path = tmp.path().join("subagents.v1.json");
        let manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(tmp.path().to_path_buf(), 4)
                .with_state_path(state_path.clone()),
        ));
        let (claimant, owner) = {
            let mut guard = manager.write().await;
            let claimant = guard.insert_test_running_agent("claimant", tmp.path());
            let owner = guard.insert_test_running_agent("owner", tmp.path());
            let active = [claimant.clone(), owner.clone()]
                .into_iter()
                .collect::<BTreeSet<_>>();
            for claim in [
                WriteScopeClaim {
                    owner: claimant.clone(),
                    roots: vec!["src/claimant".into()],
                    exact_files: Vec::new(),
                    contracts: Vec::new(),
                },
                WriteScopeClaim {
                    owner: owner.clone(),
                    roots: vec!["src/shared".into()],
                    exact_files: Vec::new(),
                    contracts: Vec::new(),
                },
            ] {
                guard
                    .coordination
                    .register_claim(claim, false, |candidate| active.contains(candidate))
                    .expect("initial non-overlapping claim");
            }
            (claimant, owner)
        };

        let error = AgentsCoordinateTool::new(Arc::clone(&manager), Some(claimant.clone()))
            .execute(
                json!({ "action": "claim", "roots": ["src/shared/nested"] }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect_err("overlap must block")
            .to_string();
        assert!(
            error.contains(&owner) && error.contains("contention"),
            "{error}"
        );

        let mut replayed = super::super::SubAgentManager::new(tmp.path().to_path_buf(), 4)
            .with_state_path(state_path);
        replayed.load_state().expect("reload contention receipt");
        assert_eq!(replayed.coordination.contentions.len(), 1);
        assert_eq!(replayed.coordination.contentions[0].claimant, claimant);
        assert_eq!(
            replayed.coordination.contentions[0].conflicting_owner,
            owner
        );
    }

    #[tokio::test]
    async fn interrupt_fails_closed_on_self() {
        let tmp = tempdir().unwrap();
        let (manager, agent_id) = manager_with_running_child(tmp.path()).await;
        let tool = AgentsInterruptTool::new(Arc::clone(&manager)).with_caller(agent_id.clone());
        let err = tool
            .execute(
                json!({ "agent_id": agent_id }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect_err("self interrupt must fail");
        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("self") || msg.contains("own"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn interrupt_fails_closed_on_missing_target() {
        let tmp = tempdir().unwrap();
        let manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(tmp.path().to_path_buf(), 2),
        ));
        let tool = AgentsInterruptTool::new(manager);
        let err = tool
            .execute(
                json!({ "agent_id": "agent_missing" }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect_err("missing target");
        assert!(err.to_string().contains("not found") || err.to_string().contains("Agent"));
    }

    #[tokio::test]
    async fn wait_times_out_when_child_stays_running() {
        let tmp = tempdir().unwrap();
        let (manager, agent_id) = manager_with_running_child(tmp.path()).await;
        let tool = AgentsWaitTool::new(manager);
        let result = tool
            .execute(
                json!({
                    "agent_id": agent_id,
                    "timeout_secs": 1,
                    "until": "activity"
                }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect("wait returns");
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["timed_out"], json!(true));
    }

    #[tokio::test]
    async fn list_resolves_target_and_reports_queue() {
        let tmp = tempdir().unwrap();
        let (manager, agent_id) = manager_with_running_child(tmp.path()).await;
        {
            let mut guard = manager.write().await;
            guard
                .queue_parent_message(&agent_id, "note".into(), false)
                .unwrap();
        }
        let tool = AgentsListTool::new(manager);
        let result = tool
            .execute(
                json!({ "agent_id": agent_id }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect("list ok");
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["count"], json!(1));
        assert_eq!(body["agents"][0]["agent_id"], json!(agent_id));
        assert!(body["agents"][0]["queued_mail"].as_u64().unwrap_or(0) >= 1);
    }

    #[tokio::test]
    async fn followup_interrupted_continuable_queues_honestly_without_auto_resume() {
        let tmp = tempdir().unwrap();
        let manager = Arc::new(tokio::sync::RwLock::new(
            super::super::SubAgentManager::new(tmp.path().to_path_buf(), 4),
        ));
        let (agent_id, handle) = {
            let mut guard = manager.write().await;
            guard.insert_test_interrupted_continuable_agent(
                "paused_child",
                tmp.path(),
                vec![crate::models::Message {
                    role: "user".to_string(),
                    content: vec![crate::models::ContentBlock::Text {
                        text: "prior work".to_string(),
                        cache_control: None,
                    }],
                }],
            )
        };
        let tool = AgentsFollowupTool::new(Arc::clone(&manager));
        let result = tool
            .execute(
                json!({ "agent_id": agent_id, "message": "please continue" }),
                &ToolContext::new(tmp.path()),
            )
            .await
            .expect("followup ok");
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["queued"], json!(true));
        assert_eq!(body["woke"], json!(false));
        assert_eq!(body["continued_from_checkpoint"], json!(false));
        assert_eq!(body["continuation_handle"], json!(handle));
        let note = body["note"].as_str().unwrap_or_default();
        assert!(
            note.contains("not automated") && note.contains(&handle),
            "note must fail honestly with the continuation handle: {note}"
        );

        let guard = manager.read().await;
        assert_eq!(guard.queued_mail_depth(&agent_id).unwrap(), 1);
        assert!(!guard.child_was_woken(&agent_id));
    }
}

/// Coordination records for delegated Work (#4647).
///
/// Decision records, write-scope claims, and contention detection for parallel
/// agent work. Parallel work may proceed only when scopes and contracts do not
/// collide silently.
/// Status of a coordination decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    Proposed,
    Accepted,
    Superseded,
}

/// Serialized coordination state schema. Increment only with an explicit
/// migration; restart/replay must never infer a newer contract from old data.
pub const COORDINATION_SCHEMA_VERSION: u32 = 1;

const MAX_RECONCILIATION_RETRIES: u32 = 3;

const fn coordination_schema_version() -> u32 {
    COORDINATION_SCHEMA_VERSION
}

/// A bounded coordination decision record (#4647).
///
/// Persisted with stable subject, concise constraints, one active owner,
/// applicability scope, evidence handles, and sequence/version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecisionRecord {
    pub decision_id: String,
    pub subject: String,
    pub status: DecisionStatus,
    pub owner: String,
    pub scope: Vec<String>,
    pub constraints: Vec<String>,
    pub evidence_handles: Vec<String>,
    pub version: u32,
    pub sequence: u64,
}

/// A write-scope claim for a write-capable child (#4647).
///
/// Declares expected repo-relative paths/trees and named contracts.
/// This is coordination metadata, not another approval system.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteScopeClaim {
    pub owner: String,
    pub roots: Vec<String>,
    pub exact_files: Vec<String>,
    pub contracts: Vec<String>,
}

impl WriteScopeClaim {
    /// Check whether this claim overlaps with another. A claim overlaps when
    /// either normalized tree contains the other or exact files collide.
    #[must_use]
    pub fn overlaps(&self, other: &WriteScopeClaim) -> bool {
        for root_a in &self.roots {
            for root_b in &other.roots {
                if paths_overlap_by_containment(root_a, root_b)
                    || paths_overlap_by_containment(root_b, root_a)
                {
                    return true;
                }
            }
        }
        for file_a in &self.exact_files {
            if other
                .exact_files
                .iter()
                .any(|file| paths_overlap_equal(file, file_a))
                || other
                    .roots
                    .iter()
                    .any(|root| paths_overlap_by_containment(root, file_a))
            {
                return true;
            }
        }
        for file_b in &other.exact_files {
            if self
                .roots
                .iter()
                .any(|root| paths_overlap_by_containment(root, file_b))
            {
                return true;
            }
        }
        if self
            .contracts
            .iter()
            .any(|contract| other.contracts.iter().any(|other| other == contract))
        {
            return true;
        }
        false
    }

    #[must_use]
    pub fn contains_path(&self, path: &str) -> bool {
        self.exact_files.iter().any(|file| file == path)
            || self.roots.iter().any(|root| path_contains(root, path))
    }
}

fn path_contains(root: &str, candidate: &str) -> bool {
    let root = root.trim_end_matches('/');
    let candidate = candidate.trim_end_matches('/');
    root == "."
        || root == candidate
        || candidate
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn paths_overlap_equal(left: &str, right: &str) -> bool {
    left == right || left.to_lowercase() == right.to_lowercase()
}

fn paths_overlap_by_containment(root: &str, candidate: &str) -> bool {
    path_contains(root, candidate) || path_contains(&root.to_lowercase(), &candidate.to_lowercase())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedWriteClaim {
    pub claim: WriteScopeClaim,
    pub sequence: u64,
    #[serde(default)]
    pub isolated_worktree: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReconciliationReceipt {
    pub reconciliation_id: String,
    pub subject: String,
    pub owner: String,
    pub input_decisions: Vec<String>,
    pub outcome: String,
    pub evidence_handles: Vec<String>,
    /// Preserved candidate branches, patches, or artifact handles. A fan-in
    /// receipt is not valid if either conflicting candidate was discarded.
    #[serde(default)]
    pub candidate_handles: Vec<String>,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub retry_limit: u32,
    #[serde(default)]
    pub reviewer_evidence_handles: Vec<String>,
    #[serde(default)]
    pub verifier_evidence_handles: Vec<String>,
    #[serde(default)]
    pub verification_outcome: String,
    pub sequence: u64,
}

/// Durable receipt for the minimal accepted-decision context projected into a
/// child. It records counts and stable ids, never the child's transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextProjectionReceipt {
    pub child_id: String,
    pub decision_ids: Vec<String>,
    pub projected_bytes: usize,
    /// Repeated constraint facts elided across otherwise distinct decisions.
    /// Decision records themselves are never collapsed by this count.
    pub deduplicated: usize,
    /// Relevant unique decisions omitted solely because the hard count or
    /// byte bound was reached. This must not be conflated with deduplication.
    #[serde(default)]
    pub omitted: usize,
    pub sequence: u64,
}

/// Durable non-secret receipt emitted when two active shared-workspace claims
/// collide. Rejected scope expansion remains visible after restart.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteContentionReceipt {
    pub claimant: String,
    pub conflicting_owner: String,
    pub roots: Vec<String>,
    pub exact_files: Vec<String>,
    pub contracts: Vec<String>,
    pub disposition: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoordinationHotPath {
    pub path: String,
    pub active_claims: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoordinationDetailMetrics {
    pub hottest_paths: Vec<CoordinationHotPath>,
    pub package_or_module_growth: Option<Value>,
    pub route_or_cost: Option<Value>,
    pub note: String,
}

/// One bounded typed projection shared by headless inspection and the TUI.
/// It contains durable coordination facts only, never raw reasoning or a
/// delegated transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoordinationDetailProjection {
    pub schema_version: u32,
    pub sequence: u64,
    pub decisions: Vec<DecisionRecord>,
    pub write_claims: Vec<PersistedWriteClaim>,
    pub reconciliations: Vec<ReconciliationReceipt>,
    pub context_projections: Vec<ContextProjectionReceipt>,
    pub contentions: Vec<WriteContentionReceipt>,
    pub metrics: CoordinationDetailMetrics,
    pub bounded: bool,
    pub limit: usize,
}

/// Durable, bounded coordination state owned by `SubAgentManager`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationLedger {
    #[serde(default = "coordination_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub decisions: Vec<DecisionRecord>,
    #[serde(default)]
    pub write_claims: Vec<PersistedWriteClaim>,
    #[serde(default)]
    pub reconciliations: Vec<ReconciliationReceipt>,
    #[serde(default)]
    pub projections: Vec<ContextProjectionReceipt>,
    #[serde(default)]
    pub contentions: Vec<WriteContentionReceipt>,
}

impl Default for CoordinationLedger {
    fn default() -> Self {
        Self {
            schema_version: COORDINATION_SCHEMA_VERSION,
            sequence: 0,
            decisions: Vec::new(),
            write_claims: Vec::new(),
            reconciliations: Vec::new(),
            projections: Vec::new(),
            contentions: Vec::new(),
        }
    }
}

impl CoordinationLedger {
    fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    pub fn record_decision(
        &mut self,
        mut decision: DecisionRecord,
    ) -> Result<DecisionRecord, String> {
        self.validate_schema()?;
        decision.decision_id = decision.decision_id.trim().to_string();
        if !decision.decision_id.is_empty() {
            decision.decision_id = bounded_coordination_atom("decision id", &decision.decision_id)?;
        }
        decision.subject = bounded_coordination_atom("decision subject", &decision.subject)?;
        decision.owner = bounded_coordination_atom("decision owner", &decision.owner)?;
        decision.scope = normalize_coordination_values("decision scope", &decision.scope, 24)?;
        decision.constraints =
            normalize_coordination_values("decision constraints", &decision.constraints, 24)?;
        decision.evidence_handles = normalize_coordination_values(
            "decision evidence handles",
            &decision.evidence_handles,
            24,
        )?;
        reject_sensitive_coordination_values(&decision.constraints)?;
        reject_sensitive_coordination_values(&decision.evidence_handles)?;
        if decision.subject.trim().is_empty() || decision.owner.trim().is_empty() {
            return Err("decision subject and owner are required".to_string());
        }
        if !decision.decision_id.trim().is_empty()
            && self
                .decisions
                .iter()
                .any(|existing| existing.decision_id == decision.decision_id)
        {
            return Err(format!(
                "decision id '{}' already exists",
                decision.decision_id
            ));
        }
        if decision.status == DecisionStatus::Accepted
            && let Some(existing) = self.decisions.iter().find(|existing| {
                existing.subject == decision.subject
                    && existing.status == DecisionStatus::Accepted
                    && existing.decision_id != decision.decision_id
            })
        {
            return Err(format!(
                "subject '{}' already has accepted decision '{}' owned by '{}'; preserve both candidates and use neutral reconciliation",
                decision.subject, existing.decision_id, existing.owner
            ));
        }
        let next_version = self
            .decisions
            .iter()
            .filter(|existing| existing.subject == decision.subject)
            .map(|existing| existing.version)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        decision.version = decision.version.max(next_version);
        decision.sequence = self.next_sequence();
        if decision.decision_id.trim().is_empty() {
            decision.decision_id = format!("decision_{}", decision.sequence);
        }
        self.decisions.push(decision.clone());
        if self.decisions.len() > COORDINATION_RECORD_LIMIT {
            let referenced = self
                .reconciliations
                .iter()
                .flat_map(|receipt| receipt.input_decisions.iter())
                .cloned()
                .collect::<BTreeSet<_>>();
            if let Some(index) = self.decisions.iter().position(|existing| {
                existing.status != DecisionStatus::Accepted
                    && !referenced.contains(&existing.decision_id)
            }) {
                self.decisions.remove(index);
            } else {
                self.decisions.pop();
                return Err(
                    "coordination decision capacity is occupied by accepted or reconciled records"
                        .to_string(),
                );
            }
        }
        Ok(decision)
    }

    pub fn update_decision_status(
        &mut self,
        decision_id: &str,
        status: DecisionStatus,
        owner: &str,
        expected_version: u32,
    ) -> Result<DecisionRecord, String> {
        self.validate_schema()?;
        let Some(index) = self
            .decisions
            .iter()
            .position(|decision| decision.decision_id == decision_id)
        else {
            return Err(format!("decision '{decision_id}' not found"));
        };
        if self.decisions[index].owner != owner {
            return Err(format!(
                "decision '{decision_id}' is owned by '{}'; caller '{owner}' cannot change it",
                self.decisions[index].owner
            ));
        }
        if self.decisions[index].version != expected_version {
            return Err(format!(
                "decision '{decision_id}' version changed: expected {expected_version}, current {}",
                self.decisions[index].version
            ));
        }
        let subject = self.decisions[index].subject.clone();
        if status == DecisionStatus::Accepted {
            if let Some(existing) =
                self.decisions
                    .iter()
                    .enumerate()
                    .find_map(|(other_index, existing)| {
                        (other_index != index
                            && existing.subject == subject
                            && existing.status == DecisionStatus::Accepted)
                            .then_some(existing)
                    })
            {
                return Err(format!(
                    "subject '{subject}' already has accepted decision '{}' owned by '{}'; preserve both candidates and use neutral reconciliation",
                    existing.decision_id, existing.owner
                ));
            }
        }
        let sequence = self.next_sequence();
        let decision = &mut self.decisions[index];
        decision.status = status;
        decision.version = decision.version.saturating_add(1);
        decision.sequence = sequence;
        Ok(decision.clone())
    }

    pub fn register_claim<F>(
        &mut self,
        mut claim: WriteScopeClaim,
        isolated_worktree: bool,
        mut owner_is_active: F,
    ) -> Result<PersistedWriteClaim, String>
    where
        F: FnMut(&str) -> bool,
    {
        self.validate_schema()?;
        claim.owner = bounded_coordination_atom("write claim owner", &claim.owner)?;
        claim.roots = normalize_claim_paths(&claim.roots)?;
        claim.exact_files = normalize_claim_paths(&claim.exact_files)?;
        claim.contracts = normalize_claim_strings(&claim.contracts, 16, 128, "contracts")?;
        if claim.roots.is_empty() && claim.exact_files.is_empty() && claim.contracts.is_empty() {
            return Err(
                "write claim requires an owner and at least one root, file, or contract"
                    .to_string(),
            );
        }
        let replacing_existing_owner = self
            .write_claims
            .iter()
            .any(|existing| existing.claim.owner == claim.owner);
        if !replacing_existing_owner && self.write_claims.len() >= COORDINATION_RECORD_LIMIT {
            let mut inactive = Vec::new();
            for existing in &self.write_claims {
                if !owner_is_active(&existing.claim.owner) {
                    inactive.push((existing.sequence, existing.claim.owner.clone()));
                }
            }
            inactive.sort_by_key(|(sequence, _)| *sequence);
            for (_, owner) in inactive {
                if self.write_claims.len() < COORDINATION_RECORD_LIMIT {
                    break;
                }
                self.write_claims
                    .retain(|existing| existing.claim.owner != owner);
            }
            if self.write_claims.len() >= COORDINATION_RECORD_LIMIT {
                return Err(format!(
                    "write-claim capacity is {COORDINATION_RECORD_LIMIT} active owners; complete, serialize, or isolate existing work before admitting another writer"
                ));
            }
        }
        if !isolated_worktree {
            if let Some(existing) = self
                .write_claims
                .iter()
                .find(|existing| {
                    !existing.isolated_worktree
                        && existing.claim.owner != claim.owner
                        && owner_is_active(&existing.claim.owner)
                        && existing.claim.overlaps(&claim)
                })
                .cloned()
            {
                let receipt = WriteContentionReceipt {
                    claimant: claim.owner.clone(),
                    conflicting_owner: existing.claim.owner.clone(),
                    roots: claim.roots.clone(),
                    exact_files: claim.exact_files.clone(),
                    contracts: claim.contracts.clone(),
                    disposition: "blocked_pending_isolation_or_serialization".to_string(),
                    sequence: self.next_sequence(),
                };
                self.contentions.push(receipt);
                trim_front(&mut self.contentions, COORDINATION_RECORD_LIMIT);
                return Err(format!(
                    "write-scope contention with {} (roots: {:?}, files: {:?}, contracts: {:?}); serialize the work, narrow the claim, or use worktree isolation",
                    existing.claim.owner,
                    existing.claim.roots,
                    existing.claim.exact_files,
                    existing.claim.contracts
                ));
            }
        }
        self.write_claims
            .retain(|existing| existing.claim.owner != claim.owner);
        let record = PersistedWriteClaim {
            claim,
            sequence: self.next_sequence(),
            isolated_worktree,
        };
        self.write_claims.push(record.clone());
        Ok(record)
    }

    pub fn reconcile(
        &mut self,
        subject: String,
        owner: String,
        input_decisions: Vec<String>,
        outcome: String,
        evidence_handles: Vec<String>,
        candidate_handles: Vec<String>,
        retry_count: u32,
        retry_limit: u32,
        reviewer_evidence_handles: Vec<String>,
        verifier_evidence_handles: Vec<String>,
        verification_outcome: String,
    ) -> Result<ReconciliationReceipt, String> {
        self.validate_schema()?;
        let subject = bounded_coordination_atom("reconciliation subject", &subject)?;
        let owner = bounded_coordination_atom("reconciliation owner", &owner)?;
        let outcome = bounded_coordination_atom("reconciliation outcome", &outcome)?;
        let verification_outcome = bounded_coordination_atom(
            "reconciliation verification outcome",
            &verification_outcome,
        )?;
        if input_decisions.len() < 2 {
            return Err("neutral fan-in requires at least two input decisions".to_string());
        }
        if input_decisions.iter().collect::<BTreeSet<_>>().len() != input_decisions.len() {
            return Err("neutral fan-in decision ids must be distinct".to_string());
        }
        if candidate_handles.len() < 2
            || candidate_handles
                .iter()
                .any(|handle| handle.trim().is_empty())
        {
            return Err(
                "neutral fan-in must preserve at least two candidate branch, patch, or artifact handles"
                    .to_string(),
            );
        }
        if candidate_handles.iter().collect::<BTreeSet<_>>().len() != candidate_handles.len() {
            return Err("neutral fan-in candidate handles must be distinct".to_string());
        }
        let input_decisions =
            normalize_coordination_values("input decision ids", &input_decisions, 24)?;
        let evidence_handles = normalize_coordination_values(
            "reconciliation evidence handles",
            &evidence_handles,
            24,
        )?;
        let candidate_handles =
            normalize_coordination_values("candidate handles", &candidate_handles, 24)?;
        if input_decisions.len() < 2 {
            return Err(
                "neutral fan-in requires at least two distinct normalized input decisions"
                    .to_string(),
            );
        }
        if candidate_handles.len() < 2 {
            return Err(
                "neutral fan-in must preserve at least two distinct normalized candidate handles"
                    .to_string(),
            );
        }
        let reviewer_evidence_handles = normalize_coordination_values(
            "Reviewer evidence handles",
            &reviewer_evidence_handles,
            24,
        )?;
        let verifier_evidence_handles = normalize_coordination_values(
            "Verifier evidence handles",
            &verifier_evidence_handles,
            24,
        )?;
        reject_sensitive_coordination_values(&evidence_handles)?;
        reject_sensitive_coordination_values(&candidate_handles)?;
        reject_sensitive_coordination_values(&reviewer_evidence_handles)?;
        reject_sensitive_coordination_values(&verifier_evidence_handles)?;
        if retry_limit == 0 || retry_limit > MAX_RECONCILIATION_RETRIES {
            return Err(format!(
                "reconciliation retry_limit must be between 1 and {MAX_RECONCILIATION_RETRIES}"
            ));
        }
        if retry_count > retry_limit {
            return Err("reconciliation retry_count exceeds retry_limit".to_string());
        }
        if reviewer_evidence_handles.is_empty() || verifier_evidence_handles.is_empty() {
            return Err(
                "neutral fan-in requires independent Reviewer and Verifier evidence handles"
                    .to_string(),
            );
        }
        if reviewer_evidence_handles.iter().any(|review| {
            verifier_evidence_handles
                .iter()
                .any(|verify| verify == review)
        }) {
            return Err("Reviewer and Verifier evidence handles must be independent".to_string());
        }
        if !matches!(
            verification_outcome.as_str(),
            "verified" | "failed" | "blocked"
        ) {
            return Err(
                "neutral fan-in verification_outcome must be verified, failed, or blocked"
                    .to_string(),
            );
        }
        if input_decisions.iter().any(|id| {
            !self
                .decisions
                .iter()
                .any(|decision| &decision.decision_id == id)
        }) {
            return Err("reconciliation references an unknown decision".to_string());
        }
        let inputs = input_decisions
            .iter()
            .filter_map(|id| {
                self.decisions
                    .iter()
                    .find(|decision| &decision.decision_id == id)
            })
            .collect::<Vec<_>>();
        if inputs.iter().any(|decision| decision.subject != subject) {
            return Err("reconciliation inputs must share the requested subject".to_string());
        }
        if inputs.iter().any(|decision| decision.owner == owner) {
            return Err(
                "neutral fan-in owner must differ from every input decision owner".to_string(),
            );
        }
        let sequence = self.next_sequence();
        let receipt = ReconciliationReceipt {
            reconciliation_id: format!("reconcile_{sequence}"),
            subject,
            owner,
            input_decisions,
            outcome,
            evidence_handles,
            candidate_handles,
            retry_count,
            retry_limit,
            reviewer_evidence_handles,
            verifier_evidence_handles,
            verification_outcome,
            sequence,
        };
        self.reconciliations.push(receipt.clone());
        trim_front(&mut self.reconciliations, COORDINATION_RECORD_LIMIT);
        Ok(receipt)
    }

    pub fn project_relevant_decisions(
        &mut self,
        child_id: &str,
        claim: Option<&WriteScopeClaim>,
        capabilities: &[String],
    ) -> (String, ContextProjectionReceipt) {
        const HEADER: &str = "Accepted coordination decisions relevant to this child (bounded):\n";
        let mut seen_constraint_facts = BTreeSet::new();
        let mut decision_ids = Vec::new();
        let mut lines = Vec::new();
        let mut projected_bytes = 0usize;
        let mut deduplicated = 0usize;
        let mut omitted = 0usize;
        for decision in self
            .decisions
            .iter()
            .rev()
            .filter(|decision| decision.status == DecisionStatus::Accepted)
            .filter(|decision| decision_is_relevant(decision, claim, capabilities))
        {
            if decision_ids.len() >= COORDINATION_PROJECTION_DECISION_LIMIT {
                omitted = omitted.saturating_add(1);
                continue;
            }
            let constraints = decision
                .constraints
                .iter()
                .filter_map(|value| {
                    let value = bounded_utf8(value, 192);
                    if seen_constraint_facts.insert(value.clone()) {
                        Some(value)
                    } else {
                        deduplicated = deduplicated.saturating_add(1);
                        None
                    }
                })
                .take(8)
                .collect::<Vec<_>>()
                .join("; ");
            let mut line = format!(
                "- {} v{} [{}] owner={}",
                decision.subject, decision.version, decision.decision_id, decision.owner,
            );
            if !constraints.is_empty() {
                line.push_str(": ");
                line.push_str(&constraints);
            }
            let line = bounded_utf8(&line, 512);
            let added_bytes = line.len().saturating_add(1);
            if HEADER
                .len()
                .saturating_add(projected_bytes)
                .saturating_add(added_bytes)
                > COORDINATION_PROJECTION_BYTE_LIMIT
            {
                omitted = omitted.saturating_add(1);
                continue;
            }
            projected_bytes = projected_bytes.saturating_add(added_bytes);
            decision_ids.push(decision.decision_id.clone());
            lines.push(line);
        }
        let projection = if lines.is_empty() {
            String::new()
        } else {
            format!("{HEADER}{}", lines.join("\n"))
        };
        let receipt = ContextProjectionReceipt {
            child_id: child_id.to_string(),
            decision_ids,
            projected_bytes: projection.len(),
            deduplicated,
            omitted,
            sequence: self.next_sequence(),
        };
        self.projections.push(receipt.clone());
        trim_front(&mut self.projections, COORDINATION_RECORD_LIMIT);
        (projection, receipt)
    }

    pub(super) fn validate_replay(&mut self) -> Result<(), String> {
        self.validate_schema()?;
        if self.decisions.len() > COORDINATION_RECORD_LIMIT
            || self.write_claims.len() > COORDINATION_RECORD_LIMIT
            || self.reconciliations.len() > COORDINATION_RECORD_LIMIT
            || self.projections.len() > COORDINATION_RECORD_LIMIT
            || self.contentions.len() > COORDINATION_RECORD_LIMIT
        {
            return Err("coordination record count exceeds the durable bound".to_string());
        }

        let mut sequences = BTreeSet::new();
        let mut max_sequence = 0_u64;
        let mut decision_ids = BTreeSet::new();
        let mut accepted_subjects = BTreeSet::new();
        for decision in &self.decisions {
            bounded_coordination_atom("decision id", &decision.decision_id)?;
            bounded_coordination_atom("decision subject", &decision.subject)?;
            bounded_coordination_atom("decision owner", &decision.owner)?;
            if decision.version == 0 {
                return Err(format!(
                    "decision '{}' has zero version",
                    decision.decision_id
                ));
            }
            validate_sequence(
                decision.sequence,
                "decision",
                &mut sequences,
                &mut max_sequence,
            )?;
            if !decision_ids.insert(decision.decision_id.clone()) {
                return Err(format!("duplicate decision id '{}'", decision.decision_id));
            }
            if decision.status == DecisionStatus::Accepted
                && !accepted_subjects.insert(decision.subject.clone())
            {
                return Err(format!(
                    "multiple accepted decisions own subject '{}'",
                    decision.subject
                ));
            }
            validate_normalized_coordination_values("decision scope", &decision.scope, 24)?;
            validate_normalized_coordination_values(
                "decision constraints",
                &decision.constraints,
                24,
            )?;
            validate_normalized_coordination_values(
                "decision evidence handles",
                &decision.evidence_handles,
                24,
            )?;
            reject_sensitive_coordination_values(&decision.constraints)?;
            reject_sensitive_coordination_values(&decision.evidence_handles)?;
        }

        let mut claim_owners = BTreeSet::new();
        for claim in &self.write_claims {
            validate_sequence(
                claim.sequence,
                "write claim",
                &mut sequences,
                &mut max_sequence,
            )?;
            bounded_coordination_atom("write claim owner", &claim.claim.owner)?;
            if !claim_owners.insert(claim.claim.owner.clone()) {
                return Err(format!(
                    "duplicate write claim owner '{}'",
                    claim.claim.owner
                ));
            }
            let roots = normalize_claim_paths(&claim.claim.roots)?;
            let exact_files = normalize_claim_paths(&claim.claim.exact_files)?;
            let contracts = normalize_claim_strings(&claim.claim.contracts, 16, 128, "contracts")?;
            if roots != claim.claim.roots
                || exact_files != claim.claim.exact_files
                || contracts != claim.claim.contracts
                || (roots.is_empty() && exact_files.is_empty() && contracts.is_empty())
            {
                return Err(format!(
                    "write claim for '{}' is not normalized and bounded",
                    claim.claim.owner
                ));
            }
        }

        for receipt in &self.reconciliations {
            validate_sequence(
                receipt.sequence,
                "reconciliation",
                &mut sequences,
                &mut max_sequence,
            )?;
            validate_reconciliation_receipt(receipt, &self.decisions)?;
        }
        for projection in &self.projections {
            validate_sequence(
                projection.sequence,
                "context projection",
                &mut sequences,
                &mut max_sequence,
            )?;
            bounded_coordination_atom("projection child", &projection.child_id)?;
            if projection.decision_ids.len() > COORDINATION_PROJECTION_DECISION_LIMIT
                || projection.projected_bytes > COORDINATION_PROJECTION_BYTE_LIMIT
                || projection
                    .decision_ids
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != projection.decision_ids.len()
            {
                return Err(format!(
                    "context projection for '{}' exceeds its bounds or duplicates decisions",
                    projection.child_id
                ));
            }
        }
        for contention in &self.contentions {
            validate_sequence(
                contention.sequence,
                "contention",
                &mut sequences,
                &mut max_sequence,
            )?;
            bounded_coordination_atom("contention claimant", &contention.claimant)?;
            bounded_coordination_atom(
                "contention conflicting owner",
                &contention.conflicting_owner,
            )?;
            bounded_coordination_atom("contention disposition", &contention.disposition)?;
            if normalize_claim_paths(&contention.roots)? != contention.roots
                || normalize_claim_paths(&contention.exact_files)? != contention.exact_files
                || normalize_claim_strings(&contention.contracts, 16, 128, "contracts")?
                    != contention.contracts
            {
                return Err("contention receipt paths/contracts are not normalized".to_string());
            }
        }
        if self.sequence < max_sequence {
            return Err(format!(
                "coordination sequence {} is behind record sequence {max_sequence}",
                self.sequence
            ));
        }
        Ok(())
    }

    fn validate_schema(&self) -> Result<(), String> {
        if self.schema_version != COORDINATION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported coordination schema {}; expected {}",
                self.schema_version, COORDINATION_SCHEMA_VERSION
            ));
        }
        Ok(())
    }
}

fn normalize_claim_paths(paths: &[String]) -> Result<Vec<String>, String> {
    if paths.len() > 32 {
        return Err("write claim paths accept at most 32 entries".to_string());
    }
    let mut normalized = Vec::new();
    for path in paths {
        let path = super::normalize_claim_path(path)?;
        if !normalized.contains(&path) {
            normalized.push(path);
        }
    }
    Ok(normalized)
}

fn normalize_claim_strings(
    values: &[String],
    count_limit: usize,
    char_limit: usize,
    field: &str,
) -> Result<Vec<String>, String> {
    if values.len() > count_limit {
        return Err(format!(
            "write claim {field} accepts at most {count_limit} entries"
        ));
    }
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty()
            || value.chars().count() > char_limit
            || value.chars().any(char::is_control)
        {
            return Err(format!(
                "write claim {field} entries must be 1..={char_limit} characters"
            ));
        }
        if !normalized.iter().any(|existing| existing == value) {
            normalized.push(value.to_string());
        }
    }
    Ok(normalized)
}

fn bounded_coordination_atom(field: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 512
        || value.chars().any(|ch| matches!(ch, '\r' | '\n'))
    {
        return Err(format!(
            "{field} must be one non-empty line of at most 512 characters"
        ));
    }
    Ok(value.to_string())
}

fn normalize_coordination_values(
    field: &str,
    values: &[String],
    limit: usize,
) -> Result<Vec<String>, String> {
    if values.len() > limit {
        return Err(format!("{field} accepts at most {limit} entries"));
    }
    let mut normalized = Vec::new();
    for value in values {
        let value = bounded_coordination_atom(field, value)?;
        if !normalized.contains(&value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

fn validate_normalized_coordination_values(
    field: &str,
    values: &[String],
    limit: usize,
) -> Result<(), String> {
    if normalize_coordination_values(field, values, limit)? != values {
        return Err(format!("{field} is not trimmed and deduplicated"));
    }
    Ok(())
}

fn reject_sensitive_coordination_values(values: &[String]) -> Result<(), String> {
    const SENSITIVE_MARKERS: &[&str] = &[
        "secret",
        "password",
        "api_key",
        "api-key",
        "authorization:",
        "bearer ",
        "token=",
        "sk-",
        "ghp_",
        "xoxb-",
        "<thinking",
        "chain of thought",
        "raw reasoning",
    ];
    for value in values {
        let lower = value.to_ascii_lowercase();
        if let Some(marker) = SENSITIVE_MARKERS
            .iter()
            .find(|marker| lower.contains(**marker))
        {
            return Err(format!(
                "coordination metadata rejected sensitive or raw-reasoning marker '{marker}'"
            ));
        }
    }
    Ok(())
}

fn validate_sequence(
    sequence: u64,
    kind: &str,
    sequences: &mut BTreeSet<u64>,
    max_sequence: &mut u64,
) -> Result<(), String> {
    if sequence == 0 || !sequences.insert(sequence) {
        return Err(format!(
            "{kind} has a zero or duplicate sequence {sequence}"
        ));
    }
    *max_sequence = (*max_sequence).max(sequence);
    Ok(())
}

fn validate_reconciliation_receipt(
    receipt: &ReconciliationReceipt,
    decisions: &[DecisionRecord],
) -> Result<(), String> {
    bounded_coordination_atom("reconciliation id", &receipt.reconciliation_id)?;
    bounded_coordination_atom("reconciliation subject", &receipt.subject)?;
    bounded_coordination_atom("reconciliation owner", &receipt.owner)?;
    bounded_coordination_atom("reconciliation outcome", &receipt.outcome)?;
    if receipt.input_decisions.len() < 2
        || receipt
            .input_decisions
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != receipt.input_decisions.len()
    {
        return Err("reconciliation requires at least two distinct decision ids".to_string());
    }
    let inputs = receipt
        .input_decisions
        .iter()
        .map(|id| {
            decisions
                .iter()
                .find(|decision| &decision.decision_id == id)
                .ok_or_else(|| format!("reconciliation references unknown decision '{id}'"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if inputs
        .iter()
        .any(|decision| decision.subject != receipt.subject)
    {
        return Err("reconciliation inputs must share the requested subject".to_string());
    }
    if inputs
        .iter()
        .any(|decision| decision.owner == receipt.owner)
    {
        return Err("neutral fan-in owner must differ from every candidate owner".to_string());
    }
    if receipt.candidate_handles.len() < 2
        || receipt
            .candidate_handles
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != receipt.candidate_handles.len()
    {
        return Err("reconciliation requires at least two distinct candidate handles".to_string());
    }
    validate_normalized_coordination_values("candidate handles", &receipt.candidate_handles, 24)?;
    validate_normalized_coordination_values(
        "reconciliation evidence handles",
        &receipt.evidence_handles,
        24,
    )?;
    validate_normalized_coordination_values(
        "Reviewer evidence handles",
        &receipt.reviewer_evidence_handles,
        24,
    )?;
    validate_normalized_coordination_values(
        "Verifier evidence handles",
        &receipt.verifier_evidence_handles,
        24,
    )?;
    reject_sensitive_coordination_values(&receipt.candidate_handles)?;
    reject_sensitive_coordination_values(&receipt.evidence_handles)?;
    reject_sensitive_coordination_values(&receipt.reviewer_evidence_handles)?;
    reject_sensitive_coordination_values(&receipt.verifier_evidence_handles)?;
    if receipt.retry_limit == 0
        || receipt.retry_limit > MAX_RECONCILIATION_RETRIES
        || receipt.retry_count > receipt.retry_limit
    {
        return Err("reconciliation retry count/limit is invalid".to_string());
    }
    if receipt.reviewer_evidence_handles.is_empty()
        || receipt.verifier_evidence_handles.is_empty()
        || receipt.reviewer_evidence_handles.iter().any(|review| {
            receipt
                .verifier_evidence_handles
                .iter()
                .any(|verify| verify == review)
        })
    {
        return Err("Reviewer and Verifier evidence must be present and independent".to_string());
    }
    if !matches!(
        receipt.verification_outcome.as_str(),
        "verified" | "failed" | "blocked"
    ) {
        return Err("reconciliation verification outcome is invalid".to_string());
    }
    Ok(())
}

fn decision_is_relevant(
    decision: &DecisionRecord,
    claim: Option<&WriteScopeClaim>,
    capabilities: &[String],
) -> bool {
    if decision.scope.is_empty() {
        return true;
    }
    decision.scope.iter().any(|raw| {
        let value = raw.trim();
        let (kind, value) = value
            .split_once(':')
            .map_or(("", value), |(kind, value)| (kind.trim(), value.trim()));
        match kind {
            "capability" => capabilities.iter().any(|capability| capability == value),
            "contract" => {
                claim.is_some_and(|claim| claim.contracts.iter().any(|contract| contract == value))
            }
            "path" => claim.is_some_and(|claim| claim_reaches_path(claim, value)),
            _ => {
                capabilities.iter().any(|capability| capability == value)
                    || claim.is_some_and(|claim| {
                        claim.contracts.iter().any(|contract| contract == value)
                            || claim_reaches_path(claim, value)
                    })
            }
        }
    })
}

fn claim_reaches_path(claim: &WriteScopeClaim, path: &str) -> bool {
    let Ok(path) = super::normalize_claim_path(path) else {
        return false;
    };
    claim.contains_path(&path)
        || claim.roots.iter().any(|root| path_contains(&path, root))
        || claim
            .exact_files
            .iter()
            .any(|file| path_contains(&path, file))
}

fn bounded_utf8(value: &str, byte_limit: usize) -> String {
    if value.len() <= byte_limit {
        return value.to_string();
    }
    let mut end = byte_limit;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

fn trim_front<T>(records: &mut Vec<T>, limit: usize) {
    if records.len() > limit {
        records.drain(..records.len() - limit);
    }
}

pub struct AgentsCoordinateTool {
    manager: SharedSubAgentManager,
    caller: Option<String>,
}

impl AgentsCoordinateTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, caller: Option<String>) -> Self {
        Self { manager, caller }
    }
}

#[async_trait]
impl ToolSpec for AgentsCoordinateTool {
    fn name(&self) -> &'static str {
        "agents/coordinate"
    }

    fn description(&self) -> &'static str {
        "Record or inspect bounded coordination state: propose/accept/supersede decisions, expand the caller's write claim before mutation, or reconcile multiple decision records into one neutral fan-in receipt."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["inspect", "propose", "accept", "supersede", "claim", "reconcile"] },
                "decision_id": { "type": "string" },
                "subject": { "type": "string" },
                "expected_version": { "type": "integer", "minimum": 1 },
                "scope": { "type": "array", "items": { "type": "string" } },
                "constraints": { "type": "array", "items": { "type": "string" } },
                "evidence_handles": { "type": "array", "items": { "type": "string" } },
                "roots": { "type": "array", "items": { "type": "string" } },
                "exact_files": { "type": "array", "items": { "type": "string" } },
                "contracts": { "type": "array", "items": { "type": "string" } },
                "input_decisions": { "type": "array", "items": { "type": "string" } },
                "outcome": { "type": "string" },
                "candidate_handles": { "type": "array", "items": { "type": "string" } },
                "retry_count": { "type": "integer", "minimum": 0, "maximum": 3 },
                "retry_limit": { "type": "integer", "minimum": 1, "maximum": 3 },
                "reviewer_evidence_handles": { "type": "array", "items": { "type": "string" } },
                "verifier_evidence_handles": { "type": "array", "items": { "type": "string" } },
                "verification_outcome": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 24 }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }
    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }
    fn is_read_only_for(&self, input: &Value) -> bool {
        input.get("action").and_then(Value::as_str) == Some("inspect")
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("inspect");
        let bounded_text = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_str)
                .map(|value| value.chars().take(512).collect::<String>())
        };
        // Tool authority is the runtime caller identity. Root cannot supply an
        // arbitrary child owner and mutate that child's decisions/claim.
        let owner = self.caller.clone().unwrap_or_else(|| "root".to_string());
        let strings = |key: &str| {
            input
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .take(24)
                        .filter_map(Value::as_str)
                        .map(|value| value.chars().take(512).collect::<String>())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        if action == "inspect" {
            let manager = self.manager.read().await;
            let value = manager.inspect_coordination(
                bounded_text("subject").as_deref(),
                input
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(COORDINATION_INSPECT_LIMIT as u64) as usize,
            );
            return ToolResult::json(&value)
                .map_err(|e| ToolError::execution_failed(e.to_string()));
        }
        if !matches!(
            action,
            "propose" | "accept" | "supersede" | "claim" | "reconcile"
        ) {
            return Err(ToolError::invalid_input(format!(
                "unknown coordination action '{action}'"
            )));
        }

        let mut manager = self.manager.write().await;
        let coordination_before = manager.coordination.clone();
        let mutation = match action {
            "propose" => manager
                .record_coordination_decision(DecisionRecord {
                    decision_id: bounded_text("decision_id").unwrap_or_default(),
                    subject: bounded_text("subject").unwrap_or_default(),
                    status: DecisionStatus::Proposed,
                    owner,
                    scope: strings("scope"),
                    constraints: strings("constraints"),
                    evidence_handles: strings("evidence_handles"),
                    version: 1,
                    sequence: 0,
                })
                .map_err(ToolError::invalid_input)
                .and_then(|record| {
                    serde_json::to_value(record)
                        .map_err(|e| ToolError::execution_failed(e.to_string()))
                }),
            "accept" | "supersede" => input
                .get("expected_version")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    ToolError::invalid_input(
                        "accept/supersede requires expected_version".to_string(),
                    )
                })
                .and_then(|expected_version| {
                    manager
                        .update_coordination_decision(
                            &bounded_text("decision_id").unwrap_or_default(),
                            if action == "accept" {
                                DecisionStatus::Accepted
                            } else {
                                DecisionStatus::Superseded
                            },
                            &owner,
                            expected_version,
                        )
                        .map_err(ToolError::invalid_input)
                })
                .and_then(|record| {
                    serde_json::to_value(record)
                        .map_err(|e| ToolError::execution_failed(e.to_string()))
                }),
            "claim" => manager
                .expand_write_claim(
                    &owner,
                    strings("roots"),
                    strings("exact_files"),
                    strings("contracts"),
                )
                .map_err(ToolError::invalid_input)
                .and_then(|claim| {
                    serde_json::to_value(claim)
                        .map_err(|e| ToolError::execution_failed(e.to_string()))
                }),
            "reconcile" => manager
                .reconcile_coordination(
                    bounded_text("subject").unwrap_or_default(),
                    owner,
                    strings("input_decisions"),
                    bounded_text("outcome").unwrap_or_default(),
                    strings("evidence_handles"),
                    strings("candidate_handles"),
                    input
                        .get("retry_count")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or_default(),
                    input
                        .get("retry_limit")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or(MAX_RECONCILIATION_RETRIES),
                    strings("reviewer_evidence_handles"),
                    strings("verifier_evidence_handles"),
                    bounded_text("verification_outcome").unwrap_or_default(),
                )
                .map_err(ToolError::invalid_input)
                .and_then(|receipt| {
                    serde_json::to_value(receipt)
                        .map_err(|e| ToolError::execution_failed(e.to_string()))
                }),
            _ => unreachable!("coordination action validated above"),
        };
        if let Err(error) = manager.persist_state_synchronously() {
            manager.coordination = coordination_before;
            return Err(ToolError::execution_failed(format!(
                "failed to persist coordination action '{action}': {error}"
            )));
        }
        let value = mutation?;
        ToolResult::json(&value).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[cfg(test)]
mod records_tests {
    use super::*;

    #[test]
    fn overlapping_roots_detected() {
        let a = WriteScopeClaim {
            owner: "agent-a".into(),
            roots: vec!["src/tui/".into()],
            exact_files: vec![],
            contracts: vec![],
        };
        let b = WriteScopeClaim {
            owner: "agent-b".into(),
            roots: vec!["src/tui/widgets/".into()],
            exact_files: vec![],
            contracts: vec![],
        };
        assert!(a.overlaps(&b));
    }

    #[test]
    fn disjoint_roots_no_overlap() {
        let a = WriteScopeClaim {
            owner: "agent-a".into(),
            roots: vec!["src/tui/".into()],
            exact_files: vec![],
            contracts: vec![],
        };
        let b = WriteScopeClaim {
            owner: "agent-b".into(),
            roots: vec!["src/core/".into()],
            exact_files: vec![],
            contracts: vec![],
        };
        assert!(!a.overlaps(&b));
    }

    #[test]
    fn exact_file_collision_detected() {
        let a = WriteScopeClaim {
            owner: "agent-a".into(),
            roots: vec![],
            exact_files: vec!["src/main.rs".into()],
            contracts: vec![],
        };
        let b = WriteScopeClaim {
            owner: "agent-b".into(),
            roots: vec![],
            exact_files: vec!["src/main.rs".into()],
            contracts: vec![],
        };
        assert!(a.overlaps(&b));
    }

    #[test]
    fn path_overlap_respects_component_boundaries_and_root_coverage() {
        let root = WriteScopeClaim {
            owner: "agent-a".into(),
            roots: vec!["src".into()],
            exact_files: vec![],
            contracts: vec![],
        };
        let sibling = WriteScopeClaim {
            owner: "agent-b".into(),
            roots: vec!["src2".into()],
            exact_files: vec![],
            contracts: vec![],
        };
        let child_file = WriteScopeClaim {
            owner: "agent-c".into(),
            roots: vec![],
            exact_files: vec!["src/lib.rs".into()],
            contracts: vec![],
        };
        assert!(!root.overlaps(&sibling));
        assert!(root.overlaps(&child_file));
    }

    #[test]
    fn active_shared_claims_contend_but_isolated_claims_do_not() {
        let mut ledger = CoordinationLedger::default();
        let first = WriteScopeClaim {
            owner: "agent-a".into(),
            roots: vec!["src".into()],
            exact_files: vec![],
            contracts: vec!["public-api".into()],
        };
        ledger.register_claim(first, false, |_| false).unwrap();
        let second = WriteScopeClaim {
            owner: "agent-b".into(),
            roots: vec!["docs".into()],
            exact_files: vec![],
            contracts: vec!["public-api".into()],
        };
        let err = ledger
            .register_claim(second.clone(), false, |owner| owner == "agent-a")
            .unwrap_err();
        assert!(
            err.contains("contention") && err.contains("agent-a"),
            "{err}"
        );
        assert_eq!(ledger.contentions.len(), 1);
        assert_eq!(ledger.contentions[0].claimant, "agent-b");
        assert_eq!(ledger.contentions[0].conflicting_owner, "agent-a");
        assert!(
            ledger
                .register_claim(second, true, |owner| owner == "agent-a")
                .is_ok()
        );
    }

    #[test]
    fn active_write_claims_are_never_evicted_by_receipt_retention() {
        let mut ledger = CoordinationLedger::default();
        for index in 0..COORDINATION_RECORD_LIMIT {
            ledger
                .register_claim(
                    WriteScopeClaim {
                        owner: format!("agent-{index:03}"),
                        roots: vec![format!("pkg-{index:03}")],
                        exact_files: vec![],
                        contracts: vec![],
                    },
                    false,
                    |_| true,
                )
                .unwrap();
        }
        let error = ledger
            .register_claim(
                WriteScopeClaim {
                    owner: "agent-over-cap".into(),
                    roots: vec!["new-package".into()],
                    exact_files: vec![],
                    contracts: vec![],
                },
                false,
                |_| true,
            )
            .expect_err("all-active capacity must fail before evicting ownership");
        assert!(error.contains("active owners"), "{error}");
        assert_eq!(ledger.write_claims.len(), COORDINATION_RECORD_LIMIT);
        assert!(
            ledger
                .write_claims
                .iter()
                .any(|record| record.claim.owner == "agent-000")
        );
    }

    #[test]
    fn accepted_decisions_require_owner_and_explicit_neutral_reconciliation() {
        let mut ledger = CoordinationLedger::default();
        let make = |id: &str, owner: &str, status| DecisionRecord {
            decision_id: id.into(),
            subject: "storage".into(),
            status,
            owner: owner.into(),
            scope: vec!["router".into()],
            constraints: vec![],
            evidence_handles: vec![format!("receipt:{id}")],
            version: 1,
            sequence: 0,
        };
        ledger
            .record_decision(make("a", "agent-a", DecisionStatus::Accepted))
            .unwrap();
        ledger
            .record_decision(make("b", "agent-b", DecisionStatus::Proposed))
            .unwrap();
        let owner_error = ledger
            .update_decision_status("b", DecisionStatus::Accepted, "root", 2)
            .unwrap_err();
        assert!(owner_error.contains("owned by 'agent-b'"), "{owner_error}");
        let stale = ledger
            .update_decision_status("b", DecisionStatus::Accepted, "agent-b", 1)
            .unwrap_err();
        assert!(stale.contains("expected 1, current 2"), "{stale}");
        let conflict = ledger
            .update_decision_status("b", DecisionStatus::Accepted, "agent-b", 2)
            .unwrap_err();
        assert!(conflict.contains("neutral reconciliation"), "{conflict}");
        ledger
            .update_decision_status("a", DecisionStatus::Superseded, "agent-a", 1)
            .unwrap();
        ledger
            .update_decision_status("b", DecisionStatus::Accepted, "agent-b", 2)
            .unwrap();
        let receipt = ledger
            .reconcile(
                "storage".into(),
                "root".into(),
                vec!["a".into(), "b".into()],
                "use bounded origin-session artifacts".into(),
                vec!["test:coord".into()],
                vec!["branch:agent-a".into(), "branch:agent-b".into()],
                1,
                3,
                vec!["review:independent".into()],
                vec!["verify:locked".into()],
                "verified".into(),
            )
            .unwrap();
        assert_eq!(receipt.input_decisions.len(), 2);
        assert!(receipt.sequence > ledger.decisions[1].sequence);
    }

    #[test]
    fn relevant_decision_projection_is_deduplicated_bounded_and_receipted() {
        let mut ledger = CoordinationLedger::default();
        for (id, subject, scope) in [
            ("file", "file-contract", "path:src"),
            ("docs", "docs-contract", "path:docs"),
            ("api", "api-contract", "contract:public-api"),
        ] {
            ledger
                .record_decision(DecisionRecord {
                    decision_id: id.into(),
                    subject: subject.into(),
                    status: DecisionStatus::Accepted,
                    owner: "planner".into(),
                    scope: vec![scope.into()],
                    constraints: vec!["bounded".into(), "bounded".into()],
                    evidence_handles: vec![format!("receipt:{id}")],
                    version: 1,
                    sequence: 0,
                })
                .unwrap();
        }
        let claim = WriteScopeClaim {
            owner: "worker".into(),
            roots: vec!["src/tui".into()],
            exact_files: vec![],
            contracts: vec!["public-api".into()],
        };
        let (projection, receipt) =
            ledger.project_relevant_decisions("worker", Some(&claim), &["File".into()]);
        assert!(projection.contains("file-contract"), "{projection}");
        assert!(projection.contains("api-contract"), "{projection}");
        assert!(!projection.contains("docs-contract"), "{projection}");
        assert!(projection.len() <= COORDINATION_PROJECTION_BYTE_LIMIT);
        assert_eq!(receipt.decision_ids, vec!["api", "file"]);
        assert_eq!(receipt.deduplicated, 1);
        assert_eq!(ledger.projections.last(), Some(&receipt));
    }

    #[test]
    fn projection_receipt_distinguishes_unique_omissions_from_deduplication() {
        let mut ledger = CoordinationLedger::default();
        for index in 0..(COORDINATION_PROJECTION_DECISION_LIMIT + 2) {
            ledger
                .record_decision(DecisionRecord {
                    decision_id: format!("decision-{index}"),
                    subject: format!("subject-{index}"),
                    status: DecisionStatus::Accepted,
                    owner: "planner".into(),
                    scope: vec!["path:src".into()],
                    constraints: vec![format!("constraint-{index}")],
                    evidence_handles: vec![format!("receipt:{index}")],
                    version: 1,
                    sequence: 0,
                })
                .unwrap();
        }
        let claim = WriteScopeClaim {
            owner: "worker".into(),
            roots: vec!["src".into()],
            exact_files: vec![],
            contracts: vec![],
        };

        let (projection, receipt) = ledger.project_relevant_decisions("worker", Some(&claim), &[]);

        assert!(projection.len() <= COORDINATION_PROJECTION_BYTE_LIMIT);
        assert_eq!(
            receipt.decision_ids.len(),
            COORDINATION_PROJECTION_DECISION_LIMIT
        );
        assert_eq!(receipt.deduplicated, 0);
        assert_eq!(receipt.omitted, 2);
    }

    #[test]
    fn coordination_schema_drift_fails_closed_before_mutation() {
        let mut ledger = CoordinationLedger {
            schema_version: COORDINATION_SCHEMA_VERSION + 1,
            ..CoordinationLedger::default()
        };
        let error = ledger
            .register_claim(
                WriteScopeClaim {
                    owner: "worker".into(),
                    roots: vec!["src".into()],
                    exact_files: vec![],
                    contracts: vec![],
                },
                false,
                |_| false,
            )
            .unwrap_err();
        assert!(error.contains("unsupported coordination schema"), "{error}");
        assert!(ledger.write_claims.is_empty());
    }
}
