//! Low-level tool execution helpers for the engine turn loop.
//!
//! This module keeps the mechanics of MCP dispatch, execution locking, and
//! parallel-tool fanout out of `engine.rs`; the turn loop still owns planning,
//! approval, and how tool results are written back into session state.

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use super::*;

const TOOL_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

fn inherited_interactive_shell_refusal(tool_name: &str, interactive: bool) -> Option<ToolError> {
    if !interactive || !matches!(tool_name, "bash" | "Bash" | "exec_shell") {
        return None;
    }
    crate::tools::shell::inherited_interactive_terminal_refusal()
        .map(|message| ToolError::execution_failed(message.to_string()))
}

/// Emits delayed, best-effort liveness pulses for one running tool.
///
/// Keep the ticker in its own task instead of embedding `tokio::time::Interval`
/// in the already-large engine turn future. Besides keeping the turn future
/// compact, this leaves pre-execution MCP discovery and approval scheduling
/// untouched. Dropping the guard cancels and aborts the ticker synchronously.
struct ToolHeartbeatGuard {
    cancel: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ToolHeartbeatGuard {
    fn start(tx_event: mpsc::Sender<Event>, interval: Duration) -> Self {
        let cancel = tokio_util::sync::CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Tokio intervals tick immediately once. Consume that tick so fast
            // tools do not produce a pulse and the first heartbeat is delayed.
            ticker.tick().await;

            loop {
                tokio::select! {
                    biased;

                    () = task_cancel.cancelled() => break,
                    _ = ticker.tick() => {
                        match tx_event.try_send(Event::ToolCallHeartbeat) {
                            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {}
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                        }
                    }
                }
            }
        });
        Self { cancel, task }
    }
}

impl Drop for ToolHeartbeatGuard {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

/// RAII guard that pauses the TUI's terminal-state ownership for the duration
/// of an interactive tool, then restores it on drop.
///
/// Background: interactive tools (anything that needs the raw TTY — external
/// editor, `exec_shell` with stdin, etc.) need the TUI to leave alt-screen,
/// disable raw mode, and release mouse capture so the child sees a normal
/// terminal. The TUI listens for `Event::PauseEvents` / `Event::ResumeEvents`
/// and runs `pause_terminal` / `resume_terminal` in response.
///
/// Earlier code sent `PauseEvents` before tool execution and `ResumeEvents`
/// after. That worked on the happy path, but if the tool's future was dropped
/// — Ctrl+C cancellation, sub-agent abort, parent task cancelled while the
/// tool was awaiting — the second `await` never reached and `ResumeEvents`
/// was never sent. It also let interactive children start before the UI had
/// actually left alt-screen/raw mode. Both failures strand the TUI in a
/// regular shell scrollback: the parent shell scrollbar takes over, mouse
/// wheel scrolls the host terminal instead of the transcript, and the TUI
/// renders at the bottom of cooked-mode output.
///
/// `Drop` runs synchronously and can't await, so we first use `try_send` on a
/// **clone of the event channel** to push `ResumeEvents` non-blockingly. If the
/// channel is full we enqueue the resume on the active Tokio runtime instead of
/// dropping it; otherwise a burst of engine events can strand the UI in the
/// paused terminal state.
pub(super) struct InteractiveTerminalGuard {
    tx: Option<mpsc::Sender<Event>>,
}

impl InteractiveTerminalGuard {
    /// Send `PauseEvents` and arm the guard. If `interactive` is false the
    /// guard is a no-op — `Drop` will skip the resume.
    pub(super) async fn engage(
        tx: mpsc::Sender<Event>,
        interactive: bool,
    ) -> Result<Self, ToolError> {
        if !interactive {
            return Ok(Self { tx: None });
        }
        // Arm before the send/ack awaits. Cancellation can drop this future
        // at either await point; the guard must still queue ResumeEvents if a
        // PauseEvents raced into the UI first.
        let guard = Self {
            tx: Some(tx.clone()),
        };
        let ack = Arc::new(tokio::sync::Notify::new());
        match tx
            .send(Event::PauseEvents {
                ack: Some(ack.clone()),
            })
            .await
        {
            Ok(()) => {
                if tokio::time::timeout(Duration::from_millis(750), ack.notified())
                    .await
                    .is_err()
                {
                    // `guard` drops on return and queues the matching resume.
                    return Err(ToolError::execution_failed(
                        "Terminal handoff was not acknowledged; interactive tool was not launched.",
                    ));
                }
            }
            Err(err) => {
                tracing::debug!(
                    target: "engine.tool_execution",
                    ?err,
                    "InteractiveTerminalGuard: event channel closed before PauseEvents"
                );
                return Err(ToolError::execution_failed(
                    "Terminal handoff channel closed; interactive tool was not launched.",
                ));
            }
        }
        Ok(guard)
    }
}

impl Drop for InteractiveTerminalGuard {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            match tx.try_send(Event::ResumeEvents) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(event)) => {
                    match tokio::runtime::Handle::try_current() {
                        Ok(handle) => {
                            handle.spawn(async move {
                                if let Err(err) = tx.send(event).await {
                                    tracing::warn!(
                                        target: "engine.tool_execution",
                                        ?err,
                                        "InteractiveTerminalGuard: async send(ResumeEvents) failed; \
                                         terminal may stay in paused state until the next \
                                         pause/resume cycle"
                                    );
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!(
                                target: "engine.tool_execution",
                                ?err,
                                "InteractiveTerminalGuard: event channel full and no Tokio runtime \
                                 available to queue ResumeEvents; terminal may stay paused until \
                                 the next pause/resume cycle"
                            );
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    tracing::debug!(
                        target: "engine.tool_execution",
                        "InteractiveTerminalGuard: event channel closed before ResumeEvents"
                    );
                }
            }
        }
    }
}

pub(crate) fn emit_tool_audit(event: serde_json::Value) {
    let Some(path) = std::env::var_os("CODEWHALE_TOOL_AUDIT_LOG")
        .or_else(|| std::env::var_os("DEEPSEEK_TOOL_AUDIT_LOG"))
    else {
        return;
    };
    emit_tool_audit_to_path(&PathBuf::from(path), event);
}

fn emit_tool_audit_to_path(path: &Path, event: serde_json::Value) {
    let line = match serde_json::to_string(&event) {
        Ok(line) => line,
        Err(e) => {
            tracing::error!("Failed to serialize tool audit event: {e}");
            return;
        }
    };
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::error!(
            "Failed to create audit log directory {}: {e}",
            parent.display()
        );
        return;
    }
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{line}") {
                tracing::error!("Failed to write to audit log {}: {e}", path.display());
            }
        }
        Err(e) => {
            tracing::error!("Failed to open audit log {}: {e}", path.display());
        }
    }
}

impl Engine {
    pub(super) async fn execute_mcp_tool_with_pool(
        pool: Arc<AsyncMutex<McpPool>>,
        tx_event: &mpsc::Sender<Event>,
        name: &str,
        input: serde_json::Value,
    ) -> Result<RichToolResult, ToolError> {
        // A synthetic `mcp_<server>_authenticate` call runs the shared OAuth
        // login flow with the pool lock released during the browser wait, so
        // parallel MCP tools and the `/mcp` manager keep working while the
        // user signs in. On success it changes the callable MCP surface (the
        // server's real tools replace the synthetic one); flag that so the
        // turn loop merges the refreshed catalog before the next model
        // request instead of leaving the model with names it cannot legally
        // call yet.
        let auth_target = pool.lock().await.authenticate_tool_target(name);
        if let Some(server) = auth_target {
            let result = crate::mcp::authenticate_tool_via_pool(&pool, &server, |url| {
                // The model cannot relay the URL until the call returns, and
                // the call returns only after the sign-in completes — so the
                // user must see it now. This status is the only copy of the
                // URL: `try_send` drops it on a full channel, so the send
                // waits for room on its own task instead of failing the
                // login.
                let server = server.clone();
                let url = url.to_string();
                let tx = tx_event.clone();
                tokio::spawn(async move {
                    let _ = tx
                        .send(Event::status(format!(
                            "◆ auth required: sign in to MCP server '{server}' in your browser — {url}"
                        )))
                        .await;
                });
            })
            .await
            .map_err(|e| ToolError::execution_failed(format!("MCP tool failed: {e}")))?;
            let mut rich = crate::tools::registry::mcp_result_to_bounded_rich_tool_result(result);
            if rich.result.success {
                rich.result.metadata = Some(serde_json::json!({ "mcp_catalog_changed": true }));
            }
            return Ok(rich);
        }
        let needs_auth_generation_before = pool.lock().await.needs_auth_generation();
        let result = pool.lock().await.call_tool(name, input).await;
        match result {
            Ok(result) => {
                Ok(crate::tools::registry::mcp_result_to_bounded_rich_tool_result(result))
            }
            Err(error) => {
                // A credential the server stopped accepting mid-session
                // flips it into the typed needs-auth state and drops its
                // connection — the callable surface changed. For THAT
                // transition only, return the failure as a result (not an
                // Err) carrying `mcp_catalog_changed`, so the turn loop
                // replaces the pool's catalog slice: dead tools leave, the
                // synthetic login tool arrives, and the error's own hint
                // stays model-readable. Every other failure keeps the Err
                // contract.
                let auth_surface_changed = {
                    let pool = pool.lock().await;
                    pool.needs_auth_generation() != needs_auth_generation_before
                };
                if !auth_surface_changed {
                    return Err(ToolError::execution_failed(format!(
                        "MCP tool failed: {error}"
                    )));
                }
                let tool_result =
                    crate::tools::spec::ToolResult::error(format!("MCP tool failed: {error}"))
                        .with_metadata(serde_json::json!({ "mcp_catalog_changed": true }));
                Ok(RichToolResult::plain(tool_result))
            }
        }
    }

    pub(super) async fn execute_parallel_tool(
        &mut self,
        input: serde_json::Value,
        tool_registry: Option<&crate::tools::ToolRegistry>,
        tool_exec_lock: Arc<RwLock<()>>,
        context_override: Option<crate::tools::ToolContext>,
    ) -> Result<RichToolResult, ToolError> {
        let calls = parse_parallel_tool_calls(&input)?;
        let mcp_pool = if calls.iter().any(|(tool, _)| McpPool::is_mcp_tool(tool)) {
            Some(self.ensure_mcp_pool().await?)
        } else {
            None
        };
        let Some(registry) = tool_registry else {
            return Err(ToolError::not_available(
                "tool registry unavailable for multi_tool_use.parallel",
            ));
        };

        let result_count = calls.len();
        let mut tasks = FuturesUnordered::new();
        let shell_permits = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_SHELL_EXEC));
        for (index, (tool_name, tool_input)) in calls.into_iter().enumerate() {
            if tool_name == MULTI_TOOL_PARALLEL_NAME {
                return Err(ToolError::invalid_input(
                    "multi_tool_use.parallel cannot call itself",
                ));
            }
            if McpPool::is_mcp_tool(&tool_name) {
                if !mcp_tool_is_parallel_safe(&tool_name) {
                    return Err(ToolError::invalid_input(format!(
                        "Tool '{tool_name}' is an MCP tool and cannot run in parallel. \
                         Allowed MCP tools: list_mcp_resources, list_mcp_resource_templates, \
                         mcp_read_resource, read_mcp_resource, mcp_get_prompt."
                    )));
                }
            } else {
                let Some(spec) = registry.get(&tool_name) else {
                    return Err(ToolError::not_available(format!(
                        "tool '{tool_name}' is not registered"
                    )));
                };
                if !spec.is_read_only_for(&tool_input) {
                    return Err(ToolError::invalid_input(format!(
                        "Tool '{tool_name}' is not read-only and cannot run in parallel"
                    )));
                }
                if spec.approval_requirement_for(&tool_input) != ApprovalRequirement::Auto {
                    return Err(ToolError::invalid_input(format!(
                        "Tool '{tool_name}' requires approval and cannot run in parallel"
                    )));
                }
                if !spec.supports_parallel_for(&tool_input) {
                    return Err(ToolError::invalid_input(format!(
                        "Tool '{tool_name}' does not support parallel execution"
                    )));
                }
            }

            let registry_ref = registry;
            let lock = tool_exec_lock.clone();
            let tx_event = self.tx_event.clone();
            let mcp_pool = mcp_pool.clone();
            let shell_permits = shell_permits.clone();
            let workspace = self.session.workspace.clone();
            let context_override = context_override.clone();
            let cancel_token = self.cancel_token.clone();
            tasks.push(async move {
                let _shell_permit = if matches!(tool_name.as_str(), "bash" | "Bash" | "exec_shell")
                {
                    shell_permits.acquire_owned().await.ok()
                } else {
                    None
                };
                let result = Engine::execute_tool_with_lock(
                    lock,
                    true,
                    false,
                    tx_event,
                    Some(cancel_token),
                    tool_name.clone(),
                    tool_input.clone(),
                    workspace,
                    Some(registry_ref),
                    mcp_pool,
                    context_override,
                )
                .await;
                (index, tool_name, result)
            });
        }

        let mut results: Vec<Option<ParallelToolResultEntry>> = Vec::with_capacity(result_count);
        results.resize_with(result_count, || None);
        let mut content_blocks = Vec::new();
        while let Some((index, tool_name, result)) = tasks.next().await {
            let entry = match result {
                Ok(output) => {
                    let RichToolResult {
                        result: output,
                        content_blocks: output_blocks,
                    } = output;
                    content_blocks.extend(output_blocks);
                    let mut error = None;
                    if !output.success {
                        error = Some(output.content.clone());
                    }
                    ParallelToolResultEntry {
                        tool_name,
                        success: output.success,
                        content: output.content,
                        error,
                    }
                }
                Err(err) => {
                    let message = format!("{err}");
                    ParallelToolResultEntry {
                        tool_name,
                        success: false,
                        content: format!("Error: {message}"),
                        error: Some(message),
                    }
                }
            };
            results[index] = Some(entry);
        }
        let results = results.into_iter().flatten().collect();

        let result = ToolResult::json(&ParallelToolResult { results })
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        Ok(crate::image_attach::bound_rich_tool_result(
            RichToolResult::with_content_blocks(result, content_blocks),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute_tool_with_lock(
        lock: Arc<RwLock<()>>,
        supports_parallel: bool,
        interactive: bool,
        tx_event: mpsc::Sender<Event>,
        cancel_token: Option<CancellationToken>,
        tool_name: String,
        tool_input: serde_json::Value,
        workspace: PathBuf,
        registry: Option<&crate::tools::ToolRegistry>,
        mcp_pool: Option<Arc<AsyncMutex<McpPool>>>,
        context_override: Option<crate::tools::ToolContext>,
    ) -> Result<RichToolResult, ToolError> {
        if cancel_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ToolError::permission_denied(
                "Turn stopped by user. Tool call blocked.",
            ));
        }
        // Unix inherited-terminal shell calls are impossible without a full
        // POSIX job-control lease. Refuse them before the terminal guard so a
        // known-invalid call cannot flash host scrollback or drain input.
        if let Some(error) = inherited_interactive_shell_refusal(&tool_name, interactive) {
            return Err(error);
        }
        // This guard starts before lock acquisition, so contention as well as
        // registry/MCP/interpreter execution remains visibly live.
        let _heartbeat = ToolHeartbeatGuard::start(tx_event.clone(), TOOL_HEARTBEAT_INTERVAL);
        let started_at = std::time::Instant::now();
        let dispatch = if McpPool::is_mcp_tool(&tool_name) {
            "mcp"
        } else if matches!(
            tool_name.as_str(),
            CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME
        ) {
            "interpreter"
        } else if registry.is_some() {
            "registry"
        } else {
            "missing"
        };
        let input_bytes = serde_json::to_string(&tool_input)
            .map(|s| s.len())
            .unwrap_or(0);
        tracing::debug!(
            target: "engine.tool_execution",
            tool = %tool_name,
            dispatch,
            interactive,
            supports_parallel,
            input_bytes,
            "tool.exec.start",
        );

        let _guard = if supports_parallel {
            ToolExecGuard::Read(lock.read().await)
        } else {
            ToolExecGuard::Write(lock.write().await)
        };

        // RAII pause/resume: ensures `Event::ResumeEvents` always fires on
        // drop, even if the tool future is cancelled mid-await. See
        // `InteractiveTerminalGuard` doc-comment for the regression this
        // closes (parent terminal scrollback hijacking the TUI after a
        // cancelled interactive tool).
        let _terminal = InteractiveTerminalGuard::engage(tx_event.clone(), interactive).await?;

        if cancel_token
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ToolError::permission_denied(
                "Turn stopped by user. Tool call blocked.",
            ));
        }

        let tool_authority = context_override
            .as_ref()
            .and_then(|context| context.tool_authority.as_ref())
            .or_else(|| registry.and_then(|registry| registry.context().tool_authority.as_ref()));
        if let Some(authority) = tool_authority {
            if McpPool::is_mcp_tool(&tool_name)
                && !super::dispatch::mcp_tool_is_read_only(&tool_name)
            {
                return Err(ToolError::permission_denied(format!(
                    "worker '{}' cannot run mutating MCP tool {tool_name}: it has no authorized file target",
                    authority.owner
                )));
            }
            if matches!(
                tool_name.as_str(),
                CODE_EXECUTION_TOOL_NAME | JS_EXECUTION_TOOL_NAME
            ) {
                return Err(ToolError::permission_denied(format!(
                    "worker '{}' cannot run {tool_name}: arbitrary code execution is outside its machine-readable authority envelope",
                    authority.owner
                )));
            }
        }

        let outcome: Result<RichToolResult, ToolError> = if McpPool::is_mcp_tool(&tool_name) {
            if let Some(pool) = mcp_pool {
                Engine::execute_mcp_tool_with_pool(pool, &tx_event, &tool_name, tool_input).await
            } else {
                Err(ToolError::not_available(format!(
                    "tool '{tool_name}' is not registered"
                )))
            }
        } else if tool_name == CODE_EXECUTION_TOOL_NAME {
            execute_code_execution_tool(&tool_input, &workspace)
                .await
                .map(RichToolResult::plain)
        } else if tool_name == JS_EXECUTION_TOOL_NAME {
            execute_js_execution_tool(&tool_input, &workspace)
                .await
                .map(RichToolResult::plain)
        } else if let Some(registry) = registry {
            registry
                .execute_rich_full_with_context(&tool_name, tool_input, context_override.as_ref())
                .await
        } else {
            Err(ToolError::not_available(format!(
                "tool '{tool_name}' is not registered"
            )))
        };

        let duration_ms = started_at.elapsed().as_millis() as u64;
        // The surface-agnostic choke point for every tool call, so this one
        // bump covers exec and the CLI as well as the TUI. `memory_search` is
        // counted here for the same reason — one site, not one per tool.
        let telemetry = codewhale_telemetry::session_counters();
        telemetry.bump(codewhale_telemetry::Counter::ToolCalls);
        if tool_name == "memory_search" {
            telemetry.bump(codewhale_telemetry::Counter::MemorySearch);
        }
        match &outcome {
            Ok(result) => {
                tracing::debug!(
                    target: "engine.tool_execution",
                    tool = %tool_name,
                    dispatch,
                    duration_ms,
                    success = result.result.success,
                    output_bytes = result.result.content.len(),
                    "tool.exec.end",
                );
            }
            Err(err) => {
                let kind = match err {
                    ToolError::InvalidInput { .. } => "invalid_input",
                    ToolError::MissingField { .. } => "missing_field",
                    ToolError::PathEscape { .. } => "path_escape",
                    ToolError::ExecutionFailed { .. } => "execution_failed",
                    ToolError::Timeout { .. } => "timeout",
                    ToolError::Cancelled { .. } => "cancelled",
                    ToolError::NotAvailable { .. } => "not_available",
                    ToolError::PermissionDenied { .. } => "permission_denied",
                };
                // The discriminant and nothing else. `ToolError::PathEscape`'s
                // `Display` *is* an absolute path, and several sibling
                // variants render a literal source fragment the model emitted.
                match err {
                    ToolError::PermissionDenied { .. } => {
                        telemetry.bump_error(codewhale_telemetry::ErrorCounter::ToolDeniedByPolicy)
                    }
                    ToolError::Timeout { .. } => {
                        telemetry.bump_error(codewhale_telemetry::ErrorCounter::ToolTimeout);
                    }
                    _ => {}
                }
                tracing::warn!(
                    target: "engine.tool_execution",
                    tool = %tool_name,
                    dispatch,
                    duration_ms,
                    error_kind = kind,
                    error = %err,
                    "tool.exec.end",
                );
            }
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    const TEST_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(10);

    #[tokio::test]
    async fn tool_heartbeat_emits_for_slow_tool() {
        let (tx, mut rx) = mpsc::channel(4);
        let guard = ToolHeartbeatGuard::start(tx, TEST_HEARTBEAT_INTERVAL);

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("heartbeat before slow tool completes")
            .expect("event channel stays open");

        assert!(matches!(event, Event::ToolCallHeartbeat));
        drop(guard);
    }

    #[tokio::test]
    async fn tool_heartbeat_is_delayed_for_fast_tool() {
        let (tx, mut rx) = mpsc::channel(4);

        let guard = ToolHeartbeatGuard::start(tx, TEST_HEARTBEAT_INTERVAL);
        drop(guard);
        tokio::time::sleep(TEST_HEARTBEAT_INTERVAL * 2).await;

        assert!(rx.try_recv().is_err(), "fast tool emitted a heartbeat");
    }

    #[tokio::test]
    async fn tool_heartbeat_stops_after_tool_completes() {
        let (tx, mut rx) = mpsc::channel(8);
        let guard = ToolHeartbeatGuard::start(tx, TEST_HEARTBEAT_INTERVAL);

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("heartbeat before slow tool completes")
            .expect("event channel stays open");
        assert!(matches!(event, Event::ToolCallHeartbeat));

        drop(guard);
        tokio::task::yield_now().await;
        while rx.try_recv().is_ok() {}
        tokio::time::sleep(TEST_HEARTBEAT_INTERVAL * 2).await;
        assert!(
            rx.try_recv().is_err(),
            "heartbeat continued after tool completion"
        );
    }

    #[tokio::test]
    async fn full_event_channel_never_blocks_tool_heartbeat() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(Event::status("filler")).expect("fill channel");

        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let guard = ToolHeartbeatGuard::start(tx, TEST_HEARTBEAT_INTERVAL);
            tokio::time::sleep(TEST_HEARTBEAT_INTERVAL * 3).await;
            drop(guard);
            "done"
        })
        .await
        .expect("full event channel must not block tool completion");

        assert_eq!(result, "done");
        assert!(matches!(rx.recv().await, Some(Event::Status { .. })));
        assert!(rx.try_recv().is_err(), "heartbeat displaced queued event");
    }

    #[tokio::test]
    async fn terminal_guard_queues_resume_when_event_channel_is_full() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(Event::status("filler")).expect("fill channel");

        drop(InteractiveTerminalGuard { tx: Some(tx) });

        assert!(matches!(rx.recv().await, Some(Event::Status { .. })));
        let resumed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("queued resume event")
            .expect("event channel still open");
        assert!(matches!(resumed, Event::ResumeEvents));
    }

    #[tokio::test]
    async fn terminal_guard_waits_for_pause_ack_before_returning() {
        let (tx, mut rx) = mpsc::channel(4);
        let task = tokio::spawn(InteractiveTerminalGuard::engage(tx, true));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("pause event")
            .expect("event channel still open");
        let ack = match event {
            Event::PauseEvents { ack: Some(ack) } => ack,
            other => panic!("expected PauseEvents with ack, got {other:?}"),
        };

        tokio::task::yield_now().await;
        assert!(!task.is_finished(), "guard returned before pause ack");

        ack.notify_one();
        let guard = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("guard returned after ack")
            .expect("guard task joined")
            .expect("terminal handoff acknowledged");

        drop(guard);
        let resumed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("resume event")
            .expect("event channel still open");
        assert!(matches!(resumed, Event::ResumeEvents));
    }

    #[tokio::test]
    async fn terminal_guard_refuses_child_and_queues_resume_when_pause_is_not_acknowledged() {
        let (tx, mut rx) = mpsc::channel(4);
        let task = tokio::spawn(InteractiveTerminalGuard::engage(tx, true));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("pause event")
            .expect("event channel still open");
        let _unacknowledged_pause = match event {
            Event::PauseEvents { ack: Some(ack) } => ack,
            other => panic!("expected PauseEvents with ack, got {other:?}"),
        };

        let handoff = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("guard refused child after pause timeout")
            .expect("guard task joined");
        let err = match handoff {
            Ok(_) => panic!("unacknowledged terminal handoff must fail closed"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("was not acknowledged"),
            "unexpected handoff error: {err}"
        );

        let resumed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("queued resume event")
            .expect("event channel still open");
        assert!(matches!(resumed, Event::ResumeEvents));
    }

    #[tokio::test]
    async fn terminal_guard_cancellation_during_pause_ack_still_queues_resume() {
        let (tx, mut rx) = mpsc::channel(4);
        let task = tokio::spawn(InteractiveTerminalGuard::engage(tx, true));

        let event = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("pause event")
            .expect("event channel still open");
        let _unacknowledged_pause = match event {
            Event::PauseEvents { ack: Some(ack) } => ack,
            other => panic!("expected PauseEvents with ack, got {other:?}"),
        };

        task.abort();
        let cancelled = match task.await {
            Ok(_) => panic!("engage future should be cancelled"),
            Err(cancelled) => cancelled,
        };
        assert!(cancelled.is_cancelled());

        let resumed = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("queued resume event")
            .expect("event channel still open");
        assert!(matches!(resumed, Event::ResumeEvents));
    }

    #[cfg(unix)]
    #[test]
    fn inherited_interactive_shell_is_refused_before_terminal_handoff() {
        for tool_name in ["bash", "Bash", "exec_shell"] {
            let err = inherited_interactive_shell_refusal(tool_name, true)
                .expect("Unix inherited-interactive shell must fail preflight");
            assert!(
                err.to_string().contains("foreground TTY ownership"),
                "{tool_name}: {err}"
            );
        }
        assert!(inherited_interactive_shell_refusal("Bash", false).is_none());
        assert!(
            inherited_interactive_shell_refusal(REQUEST_USER_INPUT_NAME, true).is_none(),
            "user-input modal keeps its own terminal handoff"
        );
    }

    #[test]
    fn emit_tool_audit_to_path_writes_jsonl_lines() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("audit.log");
        let marker = path.display().to_string();

        emit_tool_audit_to_path(
            &path,
            json!({
                "event": "tool.spillover",
                "test_marker": marker,
                "tool_id": "call-abc",
                "tool_name": "exec_shell",
                "path": "/tmp/foo.txt",
            }),
        );
        emit_tool_audit_to_path(
            &path,
            json!({
                "event": "tool.result",
                "test_marker": marker,
                "tool_id": "call-xyz",
                "success": true,
            }),
        );

        let body = std::fs::read_to_string(&path).expect("audit log written");
        let entries: Vec<serde_json::Value> = body
            .lines()
            .map(|line| serde_json::from_str(line).expect("audit line is JSON"))
            .filter(|entry: &serde_json::Value| {
                entry.get("test_marker").and_then(|v| v.as_str()) == Some(marker.as_str())
            })
            .collect();
        assert_eq!(entries.len(), 2, "two marked emits -> two lines");

        // Each line round-trips as JSON, has the expected event key.
        let first = &entries[0];
        assert_eq!(
            first.get("event").and_then(|v| v.as_str()),
            Some("tool.spillover")
        );
        assert_eq!(
            first.get("tool_id").and_then(|v| v.as_str()),
            Some("call-abc")
        );

        let second = &entries[1];
        assert_eq!(
            second.get("event").and_then(|v| v.as_str()),
            Some("tool.result")
        );
    }

    #[test]
    fn emit_tool_audit_creates_parent_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Path with a parent that doesn't exist yet — the writer
        // should create it.
        let nested = tmp.path().join("nested").join("dir").join("audit.log");
        emit_tool_audit_to_path(&nested, json!({"event": "test"}));
        assert!(nested.exists(), "writer should mkdir -p the parent chain");
    }
}
