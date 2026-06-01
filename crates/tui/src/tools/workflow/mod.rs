//! WhaleFlow TUI integration — AgentSpawner implementation and tool registration.
//!
//! Implements [`codewhale_whaleflow::AgentSpawner`] using CodeWhale's existing
//! [`SubAgentManager`](crate::tools::subagent::SubAgentManager) /
//! [`SubAgentRuntime`](crate::tools::subagent::SubAgentRuntime) infrastructure,
//! enabling whaleFlow's declarative scheduler to fan out sub-agents with
//! optional git-worktree isolation.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use codewhale_whaleflow::{AgentResult, AgentSpawner, SpawnError, WorktreeManager};
use serde_json::Value;

use crate::tools::spec::{ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec};
use crate::tools::subagent::{
    SharedSubAgentManager, SubAgentRuntime, SubAgentStatus, SubAgentType,
};

/// Implements [`AgentSpawner`] using CodeWhale's `SubAgentManager`.
///
/// Each call to [`spawn`](AgentSpawner::spawn) fans out a background sub-agent
/// via [`SubAgentManager::spawn_background`], then polls
/// [`SubAgentManager::get_result`] until the agent reaches a terminal state.
/// When a `cwd` is supplied (worktree isolation), the worktree is created
/// before spawn, and its changes are extracted and applied back to the
/// main workspace on success.
pub struct WhaleFlowSpawner {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    workspace: PathBuf,
}

impl WhaleFlowSpawner {
    /// Create a new spawner.
    ///
    /// The `runtime` is used as the template for each child sub-agent; the
    /// child runtime is derived via [`SubAgentRuntime::background_runtime`]
    /// so children are detached from the parent turn's cancellation token.
    #[must_use]
    pub fn new(
        manager: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        workspace: PathBuf,
    ) -> Self {
        Self {
            manager,
            runtime,
            workspace,
        }
    }
}

#[async_trait]
impl AgentSpawner for WhaleFlowSpawner {
    async fn spawn(
        &self,
        task_id: String,
        prompt: String,
        agent_type: Option<String>,
        cwd: Option<PathBuf>,
        timeout_secs: Option<u64>,
        max_steps: Option<u32>,
    ) -> Result<AgentResult, SpawnError> {
        // Build the future that does the real work — we'll wrap it in a
        // timeout below.
        let task_id_inner = task_id.clone();
        let work = async {
            let task_id = task_id_inner;
            // For worktree isolation: create the worktree if cwd is set
            // (the scheduler pre-computes the path based on isolation mode).
            // `WorktreeManager::create` is idempotent — no-op if the worktree
            // already exists (e.g. reused across parallel phases).
            // Git operations are CPU-bound; run them on the blocking pool.
            let workspace = self.workspace.clone();
            let tid = task_id.clone();
            let actual_cwd = if cwd.is_some() {
                let wp = tokio::task::spawn_blocking(move || {
                    WorktreeManager::create(&tid, &workspace)
                })
                .await
                .map_err(|e| SpawnError::Internal(format!("spawn_blocking join: {e}")))??;
                Some(wp)
            } else {
                None
            };

            // Determine agent type. Default to General (full tool access).
            // Warn on unknown agent_type strings so typos don't silently
            // default to a full-access agent.
            let subagent_type = match agent_type.as_deref() {
                Some(s) => match SubAgentType::from_str(s) {
                    Some(t) => t,
                    None => {
                        tracing::warn!(
                            task_id = %task_id,
                            raw_type = %s,
                            "unknown agent_type, defaulting to General"
                        );
                        SubAgentType::default()
                    }
                },
                None => SubAgentType::default(),
            };

            // Derive a detached child runtime so the sub-agent outlives the
            // scheduler's turn token.
            let mut child_runtime = self.runtime.background_runtime();
            if let Some(ref cwd_path) = actual_cwd {
                child_runtime.context.workspace = cwd_path.clone();
            }

            // Spawn via the shared sub-agent manager.
            let spawn_result = {
                let mut mgr = self.manager.write().await;
                let opts = crate::tools::subagent::SubAgentSpawnOptions {
                    max_steps,
                    ..Default::default()
                };
                mgr.spawn_background_with_assignment_options(
                    Arc::clone(&self.manager),
                    child_runtime,
                    subagent_type,
                    prompt.clone(),
                    crate::tools::subagent::SubAgentAssignment {
                        objective: prompt.clone(),
                        role: None,
                    },
                    None, // full tool access
                    opts,
                )
                .map_err(|e| SpawnError::SpawnFailed(format!("{e}")))?
            };

            let agent_id = spawn_result.agent_id.clone();

            tracing::debug!(
                agent_id = %agent_id,
                task_id = %task_id,
                "WhaleFlow spawned sub-agent"
            );

            // Poll for completion. The sub-agent manager updates the snapshot
            // in-place when the background task finishes.
            loop {
                let snapshot = {
                    let mgr = self.manager.read().await;
                    mgr.get_result(&agent_id)
                        .map_err(|e| SpawnError::Internal(format!("{e}")))?
                };

                match snapshot.status {
                    SubAgentStatus::Running => {
                        // Still running — back off before next poll.
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    }
                    SubAgentStatus::Completed => {
                        let summary = snapshot.result.clone().unwrap_or_default();
                        let elapsed_ms = Some(snapshot.duration_ms);

                        // Clean up worktree if we created one: extract the
                        // diff patch, apply it to the main workspace, then
                        // remove the worktree. Best-effort — we already have
                        // the agent result, so worktree cleanup failures are
                        // logged but don't fail the task.
                        let mut files_touched: Vec<String> = Vec::new();
                        if cwd.is_some() {
                            let ws = self.workspace.clone();
                            let tid = task_id.clone();
                            let patch_result = tokio::task::spawn_blocking(move || {
                                WorktreeManager::extract_changes(&tid, &ws)
                            })
                            .await
                            .map_err(|e| {
                                SpawnError::Internal(format!("spawn_blocking join: {e}"))
                            });
                            match patch_result {
                                Ok(Ok(patch)) => {
                                    if !patch.trim().is_empty() {
                                        // Parse changed file paths from the diff.
                                        files_touched = patch
                                            .lines()
                                            .filter(|l| l.starts_with("+++ b/"))
                                            .filter_map(|l| l.strip_prefix("+++ b/"))
                                            .map(|s| s.to_string())
                                            .collect();
                                        let ws = self.workspace.clone();
                                        let p = patch;
                                        if let Err(e) = tokio::task::spawn_blocking(
                                            move || WorktreeManager::apply_patch(&ws, &p),
                                        )
                                        .await
                                        .map_err(|e| {
                                            SpawnError::Internal(format!(
                                                "spawn_blocking join: {e}"
                                            ))
                                        }) {
                                            tracing::warn!(
                                                task_id = %task_id,
                                                error = %e,
                                                "Failed to apply worktree patch"
                                            );
                                        }
                                    }
                                }
                                Ok(Err(e)) => {
                                    tracing::warn!(
                                        task_id = %task_id,
                                        error = %e,
                                        "Failed to extract worktree changes"
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        task_id = %task_id,
                                        error = %e,
                                        "spawn_blocking failed during worktree extraction"
                                    );
                                }
                            }
                            let ws = self.workspace.clone();
                            let tid = task_id.clone();
                            if let Err(e) = tokio::task::spawn_blocking(move || {
                                WorktreeManager::remove(&tid, &ws)
                            })
                            .await
                            .map_err(|e| {
                                SpawnError::Internal(format!("spawn_blocking join: {e}"))
                            }) {
                                tracing::warn!(
                                    task_id = %task_id,
                                    error = %e,
                                    "Failed to remove worktree"
                                );
                            }
                        }

                        return Ok(AgentResult {
                            task_id,
                            success: true,
                            summary,
                            files_touched,
                            raw_output: snapshot.result,
                            tokens_used: None,
                            cost_usd: None,
                            elapsed_ms,
                            last_checkpoint: None,
                        });
                    }
                    SubAgentStatus::Failed(err) | SubAgentStatus::Interrupted(err) => {
                        let ws = self.workspace.clone();
                        let tid = task_id.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            WorktreeManager::remove(&tid, &ws)
                        })
                        .await;
                        return Err(SpawnError::SpawnFailed(err));
                    }
                    SubAgentStatus::Cancelled => {
                        let ws = self.workspace.clone();
                        let tid = task_id.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            WorktreeManager::remove(&tid, &ws)
                        })
                        .await;
                        return Err(SpawnError::Cancelled(
                            "agent cancelled".to_string(),
                        ));
                    }
                }
            }
        };

        // Wrap the entire spawn+ poll in a timeout when `timeout_secs` is set.
        if let Some(secs) = timeout_secs {
            match tokio::time::timeout(std::time::Duration::from_secs(secs), work).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    tracing::warn!(
                        task_id = %task_id,
                        timeout_secs = secs,
                        "WhaleFlow sub-agent timed out"
                    );
                    Err(SpawnError::Timeout(format!(
                        "task '{}' timed out after {}s",
                        task_id, secs
                    )))
                }
            }
        } else {
            work.await
        }
    }
}

// ---------------------------------------------------------------------------
// workflow_run tool
// ---------------------------------------------------------------------------

/// The `workflow_run` tool — exposed to DeepSeek so it can orchestrate
/// multi-agent workflows via WhaleFlow's declarative scheduler.
pub struct WorkflowRunTool {
    spawner: Arc<WhaleFlowSpawner>,
}

impl WorkflowRunTool {
    /// Create a new `workflow_run` tool backed by the given spawner.
    #[must_use]
    pub fn new(spawner: Arc<WhaleFlowSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl ToolSpec for WorkflowRunTool {
    fn name(&self) -> &'static str {
        "workflow_run"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Run a declarative multi-agent workflow. Provide a JSON config with a goal and phases, ",
            "each containing tasks with prompts, dependencies, and optional isolation. ",
            "The scheduler will fan out sub-agents, pipe results between dependent tasks, ",
            "and return a structured result summarizing every agent's output."
        )
    }

    fn input_schema(&self) -> Value {
        serde_json::from_str(codewhale_whaleflow::tool::WORKFLOW_RUN_SCHEMA)
            .unwrap_or_else(|_| serde_json::json!({}))
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // workflow_run orchestrates sub-agents that may write files, so it
        // is NOT read-only even though the tool itself doesn't write directly.
        vec![]
    }

    fn supports_parallel(&self) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Extract the `config` sub-object and serialize it as the
        // WorkflowConfig JSON that the whaleflow scheduler expects.
        let config = input
            .get("config")
            .cloned()
            .ok_or_else(|| ToolError::missing_field("config"))?;

        let config_json =
            serde_json::to_string(&config).map_err(|e| {
                ToolError::invalid_input(format!("failed to serialize config: {e}"))
            })?;

        let spawner: Arc<dyn AgentSpawner> = self.spawner.clone();

        match codewhale_whaleflow::tool::execute_workflow(&config_json, spawner).await {
            Ok(result_json) => Ok(ToolResult::success(result_json)),
            Err(err) => Ok(ToolResult::error(err)),
        }
    }
}
