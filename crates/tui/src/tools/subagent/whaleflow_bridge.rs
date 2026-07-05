//! WhaleFlow dynamic-script driver: the tui half of the rquickjs bridge.
//!
//! `codewhale-whaleflow-js` owns the sandboxed VM and the `Send`-only
//! request types; this module owns everything that needs `SubAgentManager`
//! access (design §2.1's crate seam):
//!
//! - the long-lived driver pump that services `task()` spawns, `tools.*`
//!   calls, and `budget` queries for one script session (design §3.2),
//! - the mailbox-keyed completion pump that resolves `await task()` with the
//!   FULL result text from `manager.get_result` (design §3.1/§3.4),
//! - the cancel-cascade child derivation (`child_runtime()` + token mirror,
//!   design §3.4) and the budget reservation handoff (design §5.3),
//! - PTC posture parity: `tools.*` exposes exactly the calling context's
//!   tool surface (minus delegation), with the same approval semantics as
//!   model-issued calls — approval-gated calls round-trip a real
//!   `Event::ApprovalRequired` through the [`ToolApprovalBroker`] when the
//!   host is the root interactive turn, and throw an honest catchable error
//!   in child-hosted / headless runs,
//! - the model-facing [`WhaleFlowTool`] (M7): the `whaleflow` tool the
//!   orchestrator model calls to author-and-run a dynamic workflow script,
//!   registered on the agent tool surface next to `agent`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use codewhale_whaleflow_js::{
    BudgetQuery, BudgetSnapshot, HostChannels, SpawnRequest as JsSpawnRequest, TaskCompletion,
    ToolCallOutcome, ToolCallRequest, VmOptions,
};

use super::{
    Mailbox, MailboxMessage, MailboxReceiver, SharedSubAgentManager, SubAgentManager,
    SubAgentResult, SubAgentRuntime, SubAgentSpawnOptions, SubAgentStatus, SubAgentToolRegistry,
    SubAgentType, clamp_child_max_spawn_depth, configured_model_for_role_or_type,
    emit_parent_completion, parse_spawn_request, prepare_child_workspace,
    resolve_subagent_assignment_route,
};
use crate::core::events::Event;
use crate::tools::approval_broker::{
    BrokerVerdict, ToolApprovalBroker, registered_tool_approval_required,
};
use crate::tools::approval_cache::{build_approval_grouping_key, build_approval_key};
use crate::tools::plan::PlanState;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

/// Default spawn-time token reservation per inheriting `task()` call
/// (design §5.3's `default_task_estimate`). Debited against the shared pool
/// at spawn, released as the child's actual usage lands.
pub(crate) const DEFAULT_WHALEFLOW_TASK_RESERVE_TOKENS: u64 = 4_096;

/// Default wall-clock deadline for one script run, enforced by the VM
/// interrupt handler.
const DEFAULT_SCRIPT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// How long the completion pump will wait for the manager's terminal
/// `update_from_result` after the mailbox `Completed` wake fires. The
/// mailbox send happens *before* the manager write in `run_subagent_task`,
/// so a short reconciliation poll closes that race.
const RESULT_RECONCILE_POLL: Duration = Duration::from_millis(20);
const RESULT_RECONCILE_ATTEMPTS: u32 = 250;

/// Options for one WhaleFlow script session.
pub(crate) struct WhaleFlowScriptOptions {
    pub memory_limit_bytes: usize,
    pub max_stack_bytes: usize,
    pub timeout: Option<Duration>,
    /// Per-`task()` pool reservation for children that inherit the budget
    /// scope. `0` disables reservations.
    pub task_reserve_tokens: u64,
    /// Role posture used for the `tools.*` surface (which tools exist and
    /// which approval tier applies). Defaults to `General`.
    pub tool_role: SubAgentType,
    /// The CALLING context's explicit tool allowlist (`Some` for Custom /
    /// narrowed children, `None` for full inheritance). The script registry
    /// is built with exactly this filter so a restricted caller can never
    /// widen its tool surface by routing calls through a script.
    pub explicit_allowed_tools: Option<Vec<String>>,
}

impl Default for WhaleFlowScriptOptions {
    fn default() -> Self {
        Self {
            memory_limit_bytes: codewhale_whaleflow_js::DEFAULT_MEMORY_LIMIT_BYTES,
            max_stack_bytes: codewhale_whaleflow_js::DEFAULT_MAX_STACK_BYTES,
            timeout: Some(DEFAULT_SCRIPT_TIMEOUT),
            task_reserve_tokens: DEFAULT_WHALEFLOW_TASK_RESERVE_TOKENS,
            tool_role: SubAgentType::General,
            explicit_allowed_tools: None,
        }
    }
}

/// Run one WhaleFlow script against the live sub-agent engine.
///
/// `runtime` is the *script host's* runtime: when the script runs inside a
/// child agent, `runtime.parent_agent_id` must be that agent's id (so
/// spawned tasks parent correctly, the budget scope resolves to the active
/// pool, and `emit_parent_completion` reports upward) and `spawn_depth` its
/// depth. Children are derived via `child_runtime()` with the cancel-cascade
/// mirror (design §3.4): cancelling the script cancels its fan-out.
pub(crate) async fn run_whaleflow_script(
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    script: &str,
    options: WhaleFlowScriptOptions,
) -> Result<serde_json::Value, String> {
    let session_token = runtime.cancel_token.child_token();
    // Fresh session mailbox: the wake source for the completion pump.
    // `MailboxMessage::Completed` fires whenever `runtime.mailbox` is
    // `Some`, with no spawn_depth guard (design §3.4 decision #4).
    let (mailbox, mailbox_rx) = Mailbox::new(session_token.clone());
    let mut spawn_base = runtime.clone();
    spawn_base.cancel_token = session_token.clone();
    spawn_base.mailbox = Some(mailbox);

    let owner_agent_id = runtime
        .parent_agent_id
        .clone()
        .unwrap_or_else(|| "whaleflow_script".to_string());

    // PTC surface (design §8): the same registry/gate the sub-agent loop
    // uses, so posture + approval semantics stay mirrored, not re-derived.
    // Built from a runtime whose tool context observes the session token so
    // cancelling the script also interrupts in-flight tools.* calls.
    // The CALLER's role posture and explicit allowlist are threaded in so
    // the script surface is exactly the calling context's surface (minus
    // delegation) — never wider (posture-escalation fix).
    let mut tool_runtime = spawn_base.clone();
    tool_runtime.context.cancel_token = Some(session_token.clone());
    let tool_registry = Arc::new(SubAgentToolRegistry::new_with_owner(
        tool_runtime,
        options.tool_role.clone(),
        owner_agent_id.clone(),
        "whaleflow-script".to_string(),
        options.explicit_allowed_tools.clone(),
        runtime.todos.clone(),
        Arc::new(tokio::sync::Mutex::new(PlanState::default())),
    ));
    let tool_names = whaleflow_tool_names(&tool_registry);

    let budget_total = {
        let manager_guard = manager.read().await;
        budget_snapshot(&manager_guard, runtime.parent_agent_id.as_deref()).total
    };

    let (spawn_tx, spawn_rx) = mpsc::channel(256);
    let (tool_tx, tool_rx) = mpsc::channel(256);
    let (budget_tx, budget_rx) = mpsc::channel(64);
    let (log_tx, log_rx) = mpsc::unbounded_channel();

    let driver = WhaleFlowDriver {
        manager: Arc::clone(&manager),
        spawn_base,
        tool_registry,
        tool_names: tool_names.clone(),
        scope_parent: runtime.parent_agent_id.clone(),
        task_reserve_tokens: options.task_reserve_tokens,
        owner_agent_id: owner_agent_id.clone(),
        cancel: session_token.clone(),
    };
    let driver_handle = crate::utils::spawn_supervised(
        "whaleflow-driver",
        std::panic::Location::caller(),
        run_driver(driver, spawn_rx, tool_rx, budget_rx, log_rx, mailbox_rx),
    );

    let vm_options = VmOptions {
        memory_limit_bytes: options.memory_limit_bytes,
        max_stack_bytes: options.max_stack_bytes,
        timeout: options.timeout,
        cancel_token: session_token.clone(),
        budget_total,
        tool_names,
    };
    let channels = HostChannels {
        spawn_tx,
        tool_tx,
        budget_tx,
        log_tx,
    };
    let outcome =
        codewhale_whaleflow_js::run_script(script.to_string(), channels, vm_options).await;

    // Script finished (or failed): cancel the session so in-flight `task()`
    // children cascade-cancel (design §3.4) and the driver pump drains.
    session_token.cancel();
    let _ = driver_handle.await;

    // Stream event + upward completion report (design D / §3.1 sink #2).
    match &outcome {
        Ok(value) => {
            let rendered = render_script_result(value);
            emit_parent_completion(
                &runtime,
                &owner_agent_id,
                &format!("WhaleFlow script completed.\n{rendered}"),
            );
        }
        Err(error) => {
            if let Some(event_tx) = runtime.event_tx.as_ref() {
                let _ =
                    event_tx.try_send(Event::status(format!("WhaleFlow script failed: {error}")));
            }
            emit_parent_completion(
                &runtime,
                &owner_agent_id,
                &format!("WhaleFlow script failed: {error}"),
            );
        }
    }
    outcome
}

fn render_script_result(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The PTC tool surface: exactly the calling context's tool set minus the
/// delegation tools (`agent` / `whaleflow` — recursive dispatch goes through
/// `task()`, never `tools.*`). `tools_for_model` already applies the
/// allowlist, role posture (`runtime_profile` intersection — a Plan-mode
/// root or read-only child host still can't see write tools), and depth
/// gating, so a script never sees a tool its caller could not use.
/// Approval-gated tools stay VISIBLE here; per-call approval semantics are
/// enforced at call time (see `handle_tool_call`).
fn whaleflow_tool_names(registry: &SubAgentToolRegistry) -> Vec<String> {
    let mut names: Vec<String> = registry
        .tools_for_model(&registry.agent_type)
        .into_iter()
        .map(|tool| tool.name)
        .filter(|name| !super::is_delegation_tool(name))
        .collect();
    names.sort();
    names
}

/// Live budget snapshot for the script host's ACTIVE scope, built from the
/// existing scope machinery (`inherited_budget_scope` +
/// `aggregate_budget_spent`, design §5.1/§5.2). `remaining` subtracts
/// outstanding spawn reservations so `budget.remaining()` can gate fan-out
/// loops safely (design §5.3/§5.4).
fn budget_snapshot(manager: &SubAgentManager, parent_run_id: Option<&str>) -> BudgetSnapshot {
    if let Some((scope_id, limit)) = manager.inherited_budget_scope(parent_run_id) {
        let spent = manager.aggregate_budget_spent(&scope_id);
        let reserved = manager.reserved_budget_tokens(&scope_id);
        BudgetSnapshot {
            total: Some(limit),
            spent,
            remaining: Some(limit.saturating_sub(spent.saturating_add(reserved))),
        }
    } else if let Some(limit) = manager.default_token_budget {
        // Root-hosted scripts without a worker record: children each root
        // their own default pool, so report the configured default as the
        // per-task ceiling with nothing pooled yet.
        BudgetSnapshot {
            total: Some(limit),
            spent: 0,
            remaining: Some(limit),
        }
    } else {
        BudgetSnapshot::default()
    }
}

struct WhaleFlowDriver {
    manager: SharedSubAgentManager,
    /// Session runtime: mailbox wired, session cancel token. Children derive
    /// from this via `child_runtime()`.
    spawn_base: SubAgentRuntime,
    tool_registry: Arc<SubAgentToolRegistry>,
    /// Names actually exposed to the VM; defense-in-depth re-check on call.
    tool_names: Vec<String>,
    /// Scope anchor for budget queries (the script host's agent id).
    scope_parent: Option<String>,
    task_reserve_tokens: u64,
    owner_agent_id: String,
    cancel: CancellationToken,
}

/// The persistent multiplexer (design §3.2): one pump per script session,
/// resolving many outstanding `await task()` calls as their `agent_id`s
/// complete, while also servicing PTC and budget queries.
async fn run_driver(
    driver: WhaleFlowDriver,
    mut spawn_rx: mpsc::Receiver<JsSpawnRequest>,
    mut tool_rx: mpsc::Receiver<ToolCallRequest>,
    mut budget_rx: mpsc::Receiver<BudgetQuery>,
    mut log_rx: mpsc::UnboundedReceiver<String>,
    mut mailbox_rx: MailboxReceiver,
) {
    let mut pending: HashMap<String, tokio::sync::oneshot::Sender<Result<TaskCompletion, String>>> =
        HashMap::new();
    loop {
        tokio::select! {
            () = driver.cancel.cancelled() => break,
            Some(request) = spawn_rx.recv() => {
                match spawn_task(&driver, request.input).await {
                    Ok(snapshot) => {
                        pending.insert(snapshot.agent_id.clone(), request.reply);
                    }
                    Err(error) => {
                        // Explicit error signaling on the event stream for
                        // rejected task() calls (budget/admission/depth).
                        if let Some(event_tx) = driver.spawn_base.event_tx.as_ref() {
                            let _ = event_tx.try_send(Event::status(format!(
                                "WhaleFlow task() rejected: {error}"
                            )));
                        }
                        let _ = request.reply.send(Err(error));
                    }
                }
            }
            Some(envelope) = mailbox_rx.recv() => {
                handle_mailbox_message(&driver, &mut pending, envelope.message);
            }
            Some(request) = tool_rx.recv() => {
                handle_tool_call(&driver, request);
            }
            Some(query) = budget_rx.recv() => {
                let snapshot = {
                    let manager = driver.manager.read().await;
                    budget_snapshot(&manager, driver.scope_parent.as_deref())
                };
                let _ = query.reply.send(snapshot);
            }
            Some(message) = log_rx.recv() => {
                // Narrator line on the fan-out panel event stream.
                if let Some(event_tx) = driver.spawn_base.event_tx.as_ref() {
                    let _ = event_tx.try_send(Event::AgentProgress {
                        id: driver.owner_agent_id.clone(),
                        status: format!("[whaleflow] {message}"),
                        parent_run_id: driver.spawn_base.parent_agent_id.clone(),
                        spawn_depth: driver.spawn_base.spawn_depth,
                    });
                }
            }
        }
    }
    // Session over: any still-pending awaits resolve with an error so the
    // VM (if still alive) never hangs on a dropped oneshot.
    for (_, reply) in pending {
        let _ = reply.send(Err(
            "whaleflow: script session ended before the task completed".to_string(),
        ));
    }
}

/// Resolve completed/failed/cancelled children against the pending map.
/// Completed tasks read the FULL text from the manager (`get_result`), not
/// the truncated mailbox summary (design §3.1 sink #3).
fn handle_mailbox_message(
    driver: &WhaleFlowDriver,
    pending: &mut HashMap<String, tokio::sync::oneshot::Sender<Result<TaskCompletion, String>>>,
    message: MailboxMessage,
) {
    match message {
        MailboxMessage::Completed { agent_id, summary } => {
            if let Some(reply) = pending.remove(&agent_id) {
                let manager = Arc::clone(&driver.manager);
                tokio::spawn(async move {
                    let text = resolve_full_result_text(&manager, &agent_id, summary).await;
                    let _ = reply.send(Ok(TaskCompletion { agent_id, text }));
                });
            }
        }
        MailboxMessage::Failed { agent_id, error } => {
            if let Some(reply) = pending.remove(&agent_id) {
                let _ = reply.send(Err(error));
            }
        }
        MailboxMessage::Interrupted { agent_id, reason } => {
            if let Some(reply) = pending.remove(&agent_id) {
                let _ = reply.send(Err(format!("sub-agent interrupted: {reason}")));
            }
        }
        MailboxMessage::Cancelled { agent_id } => {
            if let Some(reply) = pending.remove(&agent_id) {
                let _ = reply.send(Err("sub-agent cancelled".to_string()));
            }
        }
        _ => {}
    }
}

/// The mailbox `Completed` send happens before the manager's terminal
/// `update_from_result` write in `run_subagent_task`; poll briefly until the
/// manager reconciles, then return the full untruncated result text.
async fn resolve_full_result_text(
    manager: &SharedSubAgentManager,
    agent_id: &str,
    fallback_summary: String,
) -> String {
    for _ in 0..RESULT_RECONCILE_ATTEMPTS {
        let snapshot = {
            let manager = manager.read().await;
            manager.get_result(agent_id).ok()
        };
        if let Some(snapshot) = snapshot
            && snapshot.status != SubAgentStatus::Running
        {
            return snapshot.result.unwrap_or_else(|| {
                if fallback_summary.is_empty() {
                    "Completed (no output)".to_string()
                } else {
                    fallback_summary.clone()
                }
            });
        }
        tokio::time::sleep(RESULT_RECONCILE_POLL).await;
    }
    if fallback_summary.is_empty() {
        "Completed (result unavailable)".to_string()
    } else {
        fallback_summary
    }
}

/// Spawn one `task()` child through the existing fire-and-forget engine.
///
/// Mirrors `spawn_subagent_from_input` with two deliberate differences
/// (design §3.4): children derive via `child_runtime()` — so `spawn_depth`
/// increments and the session cancel token cascades — and the token is
/// mirrored into `context.cancel_token` (which `child_runtime()`, unlike
/// `background_runtime()`, does not do on its own).
async fn spawn_task(driver: &WhaleFlowDriver, input: Value) -> Result<SubAgentResult, String> {
    let input = normalize_task_input(input);
    let mut spawn_request = parse_spawn_request(&input).map_err(|err| err.to_string())?;

    // task({ profile: "<id>" }): fleet-party profiles apply to script-driven
    // spawns exactly like agent-tool spawns. A degraded pin surfaces on the
    // event stream; unknown ids become a catchable JS exception.
    if spawn_request.profile.is_some() {
        let party = super::fleet_party::load_fleet_party(&driver.spawn_base.context.workspace);
        let application = super::fleet_party::apply_fleet_profile_to_spawn(
            &mut spawn_request,
            &party,
            &driver.spawn_base.model,
            driver.spawn_base.client.api_provider(),
        )
        .map_err(|err| err.to_string())?;
        if let Some(application) = application
            && let (Some(notice), Some(event_tx)) = (
                application.notice.as_deref(),
                driver.spawn_base.event_tx.as_ref(),
            )
        {
            let _ = event_tx.try_send(Event::status(format!(
                "Fleet profile '{}': {notice}",
                application.profile_id
            )));
        }
    }

    // The existing depth gate (`would_exceed_depth`, admission mirror of
    // mod.rs:3859): surfaced to JS as a thrown exception via the Err reply.
    if driver.spawn_base.would_exceed_depth() {
        return Err(format!(
            "Sub-agent depth limit reached (current depth {}, max {}). Increase via [subagents] max_depth in config.toml.",
            driver.spawn_base.spawn_depth, driver.spawn_base.max_spawn_depth
        ));
    }

    if let Some(remaining) = crate::retry_status::rate_limit_remaining() {
        let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
        return Err(format!(
            "Provider is rate-limiting; sub-agent spawning is paused for {seconds}s."
        ));
    }

    let child_workspace =
        prepare_child_workspace(&driver.spawn_base.context.workspace, &spawn_request)
            .map_err(|err| err.to_string())?;

    let mut child_runtime = driver.spawn_base.child_runtime();
    // Cancel-cascade mirror (design §3.4): cancel the script → cancel its
    // fan-out, including tools running inside the child.
    child_runtime.context.cancel_token = Some(child_runtime.cancel_token.clone());
    if let Some(max_depth) = spawn_request.max_depth {
        child_runtime.max_spawn_depth =
            clamp_child_max_spawn_depth(child_runtime.spawn_depth, max_depth);
    }
    if let Some(workspace) = child_workspace {
        child_runtime.context.workspace = workspace;
    }

    let configured_model = match spawn_request.model.clone() {
        Some(model) => Some(
            super::normalize_requested_subagent_model(
                &model,
                "model",
                driver.spawn_base.client.api_provider(),
            )
            .map_err(|err| err.to_string())?,
        ),
        None => configured_model_for_role_or_type(
            &driver.spawn_base,
            spawn_request.assignment.role.as_deref(),
            &spawn_request.agent_type,
        )
        .map_err(|err| err.to_string())?,
    };
    let route = resolve_subagent_assignment_route(
        &driver.spawn_base,
        configured_model,
        &spawn_request.prompt,
        &spawn_request.agent_type,
        spawn_request.model_strength.model_route(),
        spawn_request.thinking,
    )
    .await;
    child_runtime.model = route.model.clone();
    child_runtime.reasoning_effort = route.reasoning_effort.clone();
    child_runtime.reasoning_effort_auto = false;

    // Reservation (design §5.3): only inheriting children debit the shared
    // pool; an explicit tokenBudget forks an isolated scope.
    let budget_reserve = (spawn_request.token_budget.is_none() && driver.task_reserve_tokens > 0)
        .then_some(driver.task_reserve_tokens);

    let mut manager = driver.manager.write().await;
    manager
        .spawn_background_with_assignment_options(
            Arc::clone(&driver.manager),
            child_runtime,
            spawn_request.agent_type,
            spawn_request.prompt.clone(),
            spawn_request.assignment,
            spawn_request.allowed_tools,
            SubAgentSpawnOptions {
                name: spawn_request.session_name,
                model: Some(route.model),
                model_route: Some(route.model_route),
                // Whale-name nicknames are assigned by the manager
                // (`assign_unique_whale_name`) — never overwritten here.
                nickname: None,
                fork_context: spawn_request.fork_context,
                token_budget: spawn_request.token_budget,
                budget_reserve,
            },
        )
        .map_err(|err| err.to_string())
}

/// Map JS `task()` spellings onto `parse_spawn_request` field names
/// (design §3.3). `parse_spawn_request` already accepts the camelCase forms
/// of `tokenBudget`/`maxDepth`/`modelStrength`/`forkContext`.
fn normalize_task_input(mut input: Value) -> Value {
    if let Some(object) = input.as_object_mut() {
        for (from, to) in [
            ("description", "prompt"),
            ("subagentType", "type"),
            ("subagent_type", "type"),
            ("allowedTools", "allowed_tools"),
        ] {
            if !object.contains_key(to)
                && let Some(value) = object.remove(from)
            {
                object.insert(to.to_string(), value);
            }
        }
    }
    input
}

/// Execute one PTC call off the pump so a slow tool never blocks spawns or
/// completions. Posture parity with model-issued calls: availability is the
/// same allowlist + role-posture gate the sub-agent loop uses; the approval
/// requirement is decided by the SAME predicate the engine turn loop uses
/// (`registered_tool_approval_required`); approval-gated calls round-trip a
/// real `Event::ApprovalRequired` through the broker (root interactive host)
/// or throw an honest catchable error (child-hosted / headless). Executed
/// with `raw: true` so the script receives the full value instead of the
/// large-output synthesis.
fn handle_tool_call(driver: &WhaleFlowDriver, request: ToolCallRequest) {
    let registry = Arc::clone(&driver.tool_registry);
    let allowed = driver.tool_names.contains(&request.name);
    let broker = driver.spawn_base.approval_broker.clone();
    let event_tx = driver.spawn_base.event_tx.clone();
    let cancel = driver.cancel.clone();
    tokio::spawn(async move {
        tracing::debug!(
            target: "whaleflow",
            tool = %request.name,
            source = ?request.source,
            "executing script tool call"
        );
        let outcome = execute_script_tool_call(
            &registry,
            allowed,
            broker,
            event_tx,
            cancel,
            &request.name,
            request.input,
        )
        .await;
        let _ = request.reply.send(outcome);
    });
}

/// The per-call posture-parity flow for one `tools.<name>()` invocation.
/// See the semantics table in the module docs: Auto → execute;
/// Required/Suggest + auto_approve → execute (non-bypassable tools still
/// prompt); Required/Suggest + manual + broker → real approval round trip;
/// Required/Suggest + manual without a broker → honest catchable error.
async fn execute_script_tool_call(
    registry: &SubAgentToolRegistry,
    allowed: bool,
    broker: Option<Arc<ToolApprovalBroker>>,
    event_tx: Option<mpsc::Sender<Event>>,
    cancel: CancellationToken,
    name: &str,
    input: Value,
) -> Result<ToolCallOutcome, String> {
    if !allowed {
        return Err(format!(
            "tool '{name}' is not exposed to WhaleFlow scripts in this session"
        ));
    }
    // Defense-in-depth: re-check allowlist + role posture at call time so a
    // stale VM surface can never out-privilege the calling context.
    registry
        .availability_gate(name)
        .map_err(|err| err.to_string())?;
    let Some(spec) = registry.registry.get(name) else {
        return Err(format!("Tool {name} is not registered"));
    };
    // Same source the model path reads at approval-computation time.
    let auto_approve = registry.registry.context().auto_approve;
    let approval_required = registered_tool_approval_required(
        name,
        spec.approval_requirement_for(&input),
        auto_approve,
    );

    let mut context_override: Option<ToolContext> = None;
    if approval_required {
        let (Some(broker), Some(event_tx)) = (broker, event_tx) else {
            return Err(format!(
                "Tool '{name}' requires interactive approval and this workflow is running \
                 without a UI (child agent or headless session); rerun the parent under \
                 auto-approve, or delegate this step to a child agent via task() with a \
                 write-capable role"
            ));
        };
        let id = format!("wfcall_{}", uuid::Uuid::new_v4());
        // Keys are computed on the PRE-`raw` input so script fingerprints
        // match model-issued fingerprints and session approval caching
        // covers both identically (#360 semantics).
        let approval_key = build_approval_key(name, &input).0;
        let approval_grouping_key = build_approval_grouping_key(name, &input).0;
        let rx = broker.register(&id);
        let sent = event_tx
            .send(Event::ApprovalRequired {
                id: id.clone(),
                tool_name: name.to_string(),
                input: input.clone(),
                description: spec.approval_description_for(&input),
                approval_key,
                approval_grouping_key,
                intent_summary: None,
                approval_force_prompt: false,
            })
            .await;
        if sent.is_err() {
            broker.unregister(&id);
            return Err(format!(
                "Tool '{name}' requires approval but the event stream is closed — \
                 engine is shutting down"
            ));
        }
        tokio::select! {
            // Biased: a session cancel (turn interrupt / modal Abort) must
            // win over a simultaneously-delivered verdict so teardown never
            // races a stale approval into execution.
            biased;
            () = cancel.cancelled() => {
                broker.unregister(&id);
                return Err(format!("tool '{name}' cancelled while awaiting approval"));
            }
            verdict = rx => match verdict {
                Ok(BrokerVerdict::Approved) => {}
                Ok(BrokerVerdict::Denied) => {
                    // Same wording as the model path so scripts get a
                    // recognizable, catchable JS error naming the tool.
                    return Err(format!("Tool '{name}' denied by user"));
                }
                Ok(BrokerVerdict::RetryWithPolicy(policy)) => {
                    context_override = Some(
                        registry
                            .registry
                            .context()
                            .clone()
                            .with_elevated_sandbox_policy(policy),
                    );
                }
                Err(_) => {
                    return Err(
                        "Approval channel closed — engine is shutting down. The approval \
                         modal can no longer reach the workflow; this is typically a \
                         teardown race, not a user action."
                            .to_string(),
                    );
                }
            }
        }
    }

    // `raw: true` is injected AFTER approval-key computation so the event's
    // input (what the user saw) and the fingerprint agree with the model
    // path's spelling of the same call.
    let mut input = input;
    if let Some(object) = input.as_object_mut() {
        object.entry("raw").or_insert(Value::Bool(true));
    }
    registry
        .execute_full_preapproved(name, input, context_override)
        .await
        .map(|result| ToolCallOutcome {
            success: result.success,
            content: result.content,
            metadata: result.metadata,
        })
        .map_err(|err| err.to_string())
}

// === Approval-prompt plan summary (static scan) ===

/// Heuristic-preview disclaimer appended to every plan summary: the literal
/// `tools.<ident>` / `task(` scan is trivially evaded by aliasing
/// (`const t = tools; t.exec_shell(...)`) or computed member access, so the
/// summary must never read as an authoritative capability claim.
pub(crate) const WHALEFLOW_PLAN_HEURISTIC_NOTE: &str =
    "Static preview — aliased or computed calls may not appear";

/// Compact plan summary derived from the script text for the approval
/// prompt: call-site counts for `task(` / `parallel(` / `pipeline(`,
/// referenced fleet profiles, and referenced `tools.*` names. This is a
/// static string scan — explicitly a PREVIEW, not a sandbox; every summary
/// carries [`WHALEFLOW_PLAN_HEURISTIC_NOTE`] because aliased or computed
/// calls evade the scan. The posture and approval gates at execution time
/// are the authority. The script's own leading comment, when present, is
/// appended as a clearly-quoted UNTRUSTED line (it is model-authored text
/// and must never be presented as the system's assessment).
pub(crate) fn whaleflow_plan_summary(script: &str) -> Vec<String> {
    let task_sites = count_call_sites(script, "task(");
    let parallel_sites = count_call_sites(script, "parallel(");
    let pipeline_sites = count_call_sites(script, "pipeline(");
    let profiles = collect_profile_literals(script);
    let tools = collect_tool_references(script);

    let mut lines = Vec::new();
    if task_sites + parallel_sites + pipeline_sites > 0 {
        let mut parts = Vec::new();
        if task_sites > 0 {
            let noun = if task_sites == 1 { "site" } else { "sites" };
            parts.push(format!("{task_sites} task() {noun}"));
        }
        if parallel_sites > 0 {
            parts.push(format!("{parallel_sites} parallel()"));
        }
        if pipeline_sites > 0 {
            parts.push(format!("{pipeline_sites} pipeline()"));
        }
        lines.push(format!("spawns: {}", parts.join(", ")));
    }
    if !profiles.is_empty() {
        lines.push(format!("profiles: {}", profiles.join(", ")));
    }
    if !tools.is_empty() {
        const MAX_LISTED_TOOLS: usize = 4;
        let listed = tools
            .iter()
            .take(MAX_LISTED_TOOLS)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if tools.len() > MAX_LISTED_TOOLS {
            lines.push(format!(
                "tools: {listed} (+{} more)",
                tools.len() - MAX_LISTED_TOOLS
            ));
        } else {
            lines.push(format!("tools: {listed}"));
        }
    }
    if lines.is_empty() {
        lines.push("no direct task()/tools.* references found — review script".to_string());
    }
    lines.push(WHALEFLOW_PLAN_HEURISTIC_NOTE.to_string());
    if let Some(summary) = leading_script_comment_summary(script) {
        lines.push(format!("Script's own summary (untrusted): \"{summary}\""));
    }
    lines
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$')
}

/// Count occurrences of `needle` (e.g. `"task("`) at identifier boundaries:
/// the previous character must not be an identifier character or `.` — the
/// `.` exclusion avoids counting `tools.task(`-style member calls.
fn count_call_sites(script: &str, needle: &str) -> usize {
    let mut count = 0usize;
    let mut from = 0usize;
    while let Some(pos) = script[from..].find(needle) {
        let at = from + pos;
        let boundary = script[..at]
            .chars()
            .next_back()
            .is_none_or(|prev| !is_identifier_char(prev) && prev != '.');
        if boundary {
            count += 1;
        }
        from = at + needle.len();
    }
    count
}

/// Collect distinct string-literal values following `profile:` /
/// `"profile":` (first-appearance order).
fn collect_profile_literals(script: &str) -> Vec<String> {
    let mut profiles: Vec<String> = Vec::new();
    for key in ["\"profile\":", "'profile':", "profile:"] {
        let mut from = 0usize;
        while let Some(pos) = script[from..].find(key) {
            let at = from + pos;
            // `profile:` must itself sit at an identifier boundary so
            // `myProfile:` doesn't match.
            let boundary = key.starts_with('"')
                || key.starts_with('\'')
                || script[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|prev| !is_identifier_char(prev) && prev != '.');
            from = at + key.len();
            if !boundary {
                continue;
            }
            let rest = script[from..].trim_start();
            if let Some(quote) = rest
                .chars()
                .next()
                .filter(|ch| matches!(ch, '"' | '\'' | '`'))
                && let Some(end) = rest[quote.len_utf8()..].find(quote)
            {
                let value = &rest[quote.len_utf8()..quote.len_utf8() + end];
                if !value.is_empty() && !profiles.iter().any(|known| known == value) {
                    profiles.push(value.to_string());
                }
            }
        }
    }
    profiles
}

/// Collect distinct `tools.<ident>` references (first-appearance order).
fn collect_tool_references(script: &str) -> Vec<String> {
    let mut tools: Vec<String> = Vec::new();
    let needle = "tools.";
    let mut from = 0usize;
    while let Some(pos) = script[from..].find(needle) {
        let at = from + pos;
        let boundary = script[..at]
            .chars()
            .next_back()
            .is_none_or(|prev| !is_identifier_char(prev) && prev != '.');
        from = at + needle.len();
        if !boundary {
            continue;
        }
        let name: String = script[from..]
            .chars()
            .take_while(|ch| is_identifier_char(*ch))
            .collect();
        if !name.is_empty() && !tools.iter().any(|known| known == &name) {
            tools.push(name);
        }
    }
    tools
}

/// First sentence of a leading `//` or `/* */` comment, when the script
/// opens with one — the closest thing a script has to a self-description.
fn leading_script_comment_summary(script: &str) -> Option<String> {
    const MAX_SUMMARY_CHARS: usize = 120;
    let trimmed = script.trim_start();
    let comment = if let Some(rest) = trimmed.strip_prefix("//") {
        rest.lines().next().unwrap_or_default().trim().to_string()
    } else if let Some(rest) = trimmed.strip_prefix("/*") {
        let body = rest.split("*/").next().unwrap_or_default();
        body.lines()
            .map(|line| line.trim().trim_start_matches('*').trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        return None;
    };
    if comment.is_empty() {
        return None;
    }
    let first_sentence = match comment.find(". ") {
        Some(idx) => &comment[..=idx],
        None => comment.as_str(),
    };
    let mut summary = first_sentence.trim().to_string();
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        summary = summary.chars().take(MAX_SUMMARY_CHARS).collect::<String>() + "...";
    }
    Some(summary)
}

// === Model-facing tool (M7) ===

/// Teaching surface for the `whaleflow` tool. Mirrors the quality bar of
/// deepagents' task-tool description: API contract, limits, and explicit
/// when-to-use / when-not-to guidance, bounded well under the tool
/// description budget.
const WHALEFLOW_TOOL_DESCRIPTION: &str = r#"Author and run a WhaleFlow script: one sandboxed JavaScript program that orchestrates a whole fan-out — spawning child agents, calling this session's tools under its normal approval rules, and scaling work to a token budget — instead of you issuing many separate agent calls. The script runs to completion and this call returns its `return` value; spawned children stream onto the same fan-out panel as regular agent spawns.

API inside the script (host calls are async):
- await task({description, subagentType, profile, model, modelStrength, thinking, allowedTools, maxDepth, tokenBudget}) — spawn a child agent; resolves with its full report text. Omit tokenBudget to share the run's budget pool. profile applies a fleet party member (see the Fleet party roster on the agent tool description, when present). Rejections (depth/budget/admission) throw catchable errors.
- await tools.<name>(args) — call this session's tools directly: the full tool surface of the calling session (minus agent/whaleflow — delegate via task()), with the session's approval semantics. Approval-gated calls prompt the user in Agent mode and throw a catchable error in child-hosted/headless runs; wrap them in try/catch. Results decode as metadata ?? JSON.parse(content) ?? String(content).
- budget.total, await budget.spent(), await budget.remaining() — live token-pool readings for the active scope; remaining subtracts in-flight task reservations and is Infinity when unlimited.
- parallel(thunks) — all-settled barrier, per-item errors become null, max 4096 items. pipeline(items, ...stages) — per-item staged flow, no barrier between stages. log(msg) — progress line on the panel.
- Plain JS otherwise (Promise.all, loops, try/catch). No fs/net/import — the functions above are the entire host surface.

Limits: nested depth obeys the session spawn-depth cap (an over-deep task() throws); at most 4096 items per parallel()/pipeline() call; a 1000-spawn lifetime cap per session; the script itself times out after 10 minutes.

Use whaleflow instead of separate agent calls when:
1. fanning out to more than ~2 independent children (map N targets to one task() each, then Promise.all)
2. running staged pipelines where each item's next stage consumes its previous stage's output (explore → implement → verify)
3. budget-scaled loops: keep dispatching while (await budget.remaining()) > per-task estimate.
Do NOT use it for one or two simple children (call agent directly), for work that needs your judgment between steps, or for anything trivial enough to do inline.

Write the whole program in `script`; top-level await and return are supported. Children are ephemeral and stateless — give each task() a complete brief and say exactly what it must return."#;

/// Model-facing `whaleflow` tool: author-and-run a dynamic workflow script
/// against the calling turn's runtime (design M7). Gated like `agent`:
/// same approval posture (`Required`, `ExecutesCode`), registered on the
/// same surface (so it disappears with subagents), delegation governed by
/// the spawn-depth budget.
pub struct WhaleFlowTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    /// Role posture of the CALLING context (the agent whose model invokes
    /// `whaleflow`). Threaded into the script's `tools.*` registry so a
    /// read-only or Custom caller keeps its own posture inside the script.
    caller_agent_type: SubAgentType,
    /// The calling context's explicit tool allowlist (`Some` for Custom /
    /// narrowed children, `None` for full inheritance). The script's
    /// `tools.*` surface is built with exactly this filter — a restricted
    /// child can never widen its surface by routing calls through a script.
    caller_allowed_tools: Option<Vec<String>>,
}

impl WhaleFlowTool {
    /// Root / full-inheritance construction: the caller has no explicit
    /// allowlist and a `General` posture, so the script sees the caller's
    /// real full surface (parity, not restriction).
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self::new_scoped(manager, runtime, SubAgentType::General, None)
    }

    /// Construction scoped to the CALLING context's effective tool surface.
    /// `caller_agent_type` and `caller_allowed_tools` must be the same role
    /// posture and explicit allowlist that govern the caller's own registry
    /// (see `SubAgentToolRegistry::new_with_owner`), so scripts inherit
    /// exactly the caller's `tools.*` surface minus delegation tools.
    #[must_use]
    pub fn new_scoped(
        manager: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        caller_agent_type: SubAgentType,
        caller_allowed_tools: Option<Vec<String>>,
    ) -> Self {
        Self {
            manager,
            runtime,
            caller_agent_type,
            caller_allowed_tools,
        }
    }
}

#[async_trait]
impl ToolSpec for WhaleFlowTool {
    fn name(&self) -> &'static str {
        "whaleflow"
    }

    fn description(&self) -> &'static str {
        WHALEFLOW_TOOL_DESCRIPTION
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "The WhaleFlow JavaScript program to run. Top-level await and return are supported; the returned value becomes this tool's result."
                },
                "label": {
                    "type": "string",
                    "description": "Optional short human label for this run, shown on status/progress surfaces."
                }
            },
            "required": ["script"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    /// One-line approval headline — the full `description()` is a ~2KB
    /// teaching text that would leak into the status line, desktop
    /// notification, and audit log.
    fn approval_description_for(&self, input: &Value) -> String {
        let label = input
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(|label| format!(" '{label}'"))
            .unwrap_or_default();
        // Script-derived text (the leading comment) is model/script-authored
        // and untrusted; it must never reach this headline, which also feeds
        // desktop notifications and the audit log. The comment is rendered
        // only inside the approval card's Plan details, clearly quoted and
        // attributed (see `whaleflow_plan_summary`).
        format!(
            "Run a dynamic workflow{label}? A script will orchestrate sub-agents and tools on your behalf."
        )
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let script = input
            .get("script")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|script| !script.is_empty())
            .ok_or_else(|| ToolError::missing_field("script"))?;
        let label = input
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_string);

        // The script host itself respects the spawn-depth ceiling: a child
        // agent at the cap cannot recurse through a script either.
        if self.runtime.would_exceed_depth() {
            return Err(ToolError::execution_failed(format!(
                "WhaleFlow scripts cannot run here: sub-agent depth limit reached \
                 (current depth {}, max {}). Increase via [subagents] max_depth in config.toml.",
                self.runtime.spawn_depth, self.runtime.max_spawn_depth
            )));
        }

        // Run with the CALLING turn's posture and cancellation: a turn
        // interrupt cancels the session token, the VM interrupt handler
        // hard-kills running JS, and `run_script`'s grace window guarantees
        // this call returns even if the VM thread is wedged.
        let mut runtime = self.runtime.clone();
        runtime.context.auto_approve = context.auto_approve;
        if let Some(token) = context.cancel_token.clone() {
            runtime.cancel_token = token;
        }

        if let (Some(label), Some(event_tx)) = (label.as_deref(), runtime.event_tx.as_ref()) {
            let _ = event_tx.try_send(Event::status(format!("WhaleFlow script '{label}' running")));
        }

        // The script's `tools.*` surface is scoped to the CALLING context:
        // the caller's role posture and explicit allowlist, exactly as they
        // govern the caller's own registry (posture-escalation fix).
        let options = WhaleFlowScriptOptions {
            tool_role: self.caller_agent_type.clone(),
            explicit_allowed_tools: self.caller_allowed_tools.clone(),
            ..WhaleFlowScriptOptions::default()
        };
        match run_whaleflow_script(Arc::clone(&self.manager), runtime, script, options).await {
            Ok(value) => {
                let content = match value {
                    Value::Null => "(script completed with no return value)".to_string(),
                    Value::String(text) => text,
                    other => {
                        serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string())
                    }
                };
                let mut result = ToolResult::success(content);
                result.metadata = Some(json!({
                    "source": "whaleflow",
                    "label": label,
                }));
                Ok(result)
            }
            Err(message) => Err(ToolError::execution_failed(format!(
                "WhaleFlow script failed: {message}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests;
