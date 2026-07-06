//! Fleet worker runtime — bridges fleet task specs to headless sub-agent execution.
//!
//! This module makes fleet workers real: instead of simulating task completion,
//! each fleet worker spawns a headless sub-agent that runs the task instructions
//! and streams progress back into the fleet ledger.
//!
//! Architecture:
//! - `FleetTaskSpec` + `FleetWorkerSpec` → `AgentWorkerSpec`
//! - `SubAgentManager::register_worker()` tracks the worker
//! - Sub-agent spawn happens through the existing `agent` machinery
//! - Mailbox events stream into fleet ledger as `FleetWorkerEventPayload`
//! - `FleetWorkerInspection` reads both ledger state and sub-agent worker records

#![allow(dead_code)]

use anyhow::{Result, bail};
use codewhale_protocol::fleet::{
    FleetEffectivePermissions, FleetResolvedRoute, FleetTaskSpec, FleetTaskWorkerProfile,
    FleetWorkerSpec,
};

use super::profile::AgentProfile;
use crate::config::ApiProvider;
use crate::route_runtime::resolve_route_candidate;
use crate::tools::subagent::{AgentWorkerSpec, AgentWorkerToolProfile, SubAgentType};
use crate::worker_profile::{ModelRoute, ToolScope, WorkerRuntimeProfile};

/// Validate that every task referencing a workspace agent profile can resolve it.
///
/// This is intended to run at Fleet run creation time, before leasing any
/// worker or appending lifecycle events.
pub fn validate_task_agent_profiles(
    tasks: &[FleetTaskSpec],
    agent_profiles: &[AgentProfile],
) -> Result<()> {
    for task in tasks {
        resolve_task_agent_profile(task, agent_profiles)?;
    }
    Ok(())
}

/// Build a sub-agent worker spec after resolving workspace Fleet profile input.
///
/// This keeps Fleet and sub-agents on the same runtime substrate: profile files
/// and task-level role/loadout intent are composed into the existing
/// `AgentWorkerSpec` / `WorkerRuntimeProfile` pair, then optionally intersected
/// with a parent profile when the caller has one.
#[allow(clippy::too_many_arguments)]
pub fn fleet_task_to_worker_spec_with_profiles(
    worker_id: &str,
    run_id: &str,
    task_spec: &FleetTaskSpec,
    _worker_spec: &FleetWorkerSpec,
    model: &str,
    workspace: &std::path::Path,
    agent_profiles: &[AgentProfile],
    parent_runtime_profile: Option<&WorkerRuntimeProfile>,
) -> Result<AgentWorkerSpec> {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)?;
    let worker_profile = task_spec.worker.as_ref();
    let role = effective_fleet_role(worker_profile, agent_profile);
    let agent_type = fleet_role_to_agent_type(role.as_deref());
    let tool_profile = fleet_tool_profile(worker_profile);
    let objective = fleet_task_prompt_with_profile(task_spec, agent_profile);
    let max_spawn_depth = codewhale_config::FleetExecConfig::default().max_spawn_depth;
    let effective_model = effective_fleet_model(model, worker_profile, agent_profile);
    let mut requested_runtime = fleet_worker_runtime_profile(
        &agent_type,
        &tool_profile,
        &effective_model,
        0,
        max_spawn_depth,
    );
    if let Some(agent_profile) = agent_profile
        && let Some(profile_depth) = agent_profile.profile.delegation.max_spawn_depth
    {
        requested_runtime.max_spawn_depth = requested_runtime.max_spawn_depth.min(profile_depth);
    }
    let runtime_profile = parent_runtime_profile
        .map(|parent| parent.derive_child(&requested_runtime))
        .unwrap_or(requested_runtime);

    Ok(AgentWorkerSpec {
        worker_id: worker_id.to_string(),
        run_id: run_id.to_string(),
        parent_run_id: None,
        session_name: Some(format!("fleet-{}-{}", worker_id, task_spec.id)),
        objective,
        role,
        agent_type,
        model: effective_model,
        workspace: workspace.to_path_buf(),
        git_branch: None,
        context_mode: "fresh".to_string(),
        fork_context: false,
        tool_profile,
        runtime_profile: runtime_profile.clone(),
        max_steps: task_spec
            .budget
            .as_ref()
            .and_then(|b| b.max_tool_calls)
            .unwrap_or(u32::MAX),
        spawn_depth: 0,
        max_spawn_depth: runtime_profile.max_spawn_depth,
    })
}

/// Mint a [`FleetResolvedRoute`] snapshot for a fleet task (#3154).
///
/// This calls the existing hermetic resolver bridge
/// ([`resolve_route_candidate`]) so the persisted route reflects the same
/// resolution semantics the runtime would use, then records only non-sensitive
/// shape (provider id/kind, model ids, protocol) combined with the already
/// computed effective role/loadout/model-class intent. `source` is
/// `"resolver"`.
///
/// Honesty rules:
/// - `canonical_model` stays `None` when the resolver could not pin one.
/// - The provider comes from the resolver default (the worker profile carries
///   no provider authority); a task-level `model` selector is forwarded as the
///   model selector. No reasoning/pricing fields are fabricated.
///
/// Returns `None` (never a fabricated route) when resolution fails, so callers
/// degrade gracefully without inventing detail.
pub(crate) fn resolve_fleet_route(
    task_spec: &FleetTaskSpec,
    agent_profiles: &[AgentProfile],
) -> Option<FleetResolvedRoute> {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)
        .ok()
        .flatten();
    let worker_profile = task_spec.worker.as_ref();
    let (role, role_source) = effective_fleet_role_with_source(worker_profile, agent_profile);
    let (loadout, loadout_source) =
        effective_fleet_loadout_with_source(worker_profile, agent_profile);
    let (model_class, model_class_source) = task_model_class_with_source(worker_profile);

    // Task/profile model pins are visible route intent; otherwise let the
    // resolver pick the provider default. Provider authority belongs to route
    // resolution, so we do not infer a provider here.
    let (model_selector, model_source) =
        fleet_route_model_selector_with_source(worker_profile, agent_profile);
    let model_selector = model_selector.as_deref();

    // The worker profile carries no provider authority, so resolve within the
    // default provider scope (mirrors `ProviderKind::default()`). The resolver
    // is fully offline/hermetic and never reads secrets, env, or config.
    let candidate =
        resolve_route_candidate(ApiProvider::Deepseek, model_selector, None, None, None).ok()?;

    Some(FleetResolvedRoute {
        provider_id: candidate.provider_id.as_str().to_string(),
        provider_kind: candidate.provider_kind.as_str().to_string(),
        canonical_model: candidate
            .canonical_model
            .as_ref()
            .map(|model| model.as_str().to_string()),
        wire_model_id: candidate.wire_model_id.as_str().to_string(),
        protocol: route_protocol_label(candidate.protocol).to_string(),
        role,
        loadout: loadout_intent_label(&loadout),
        model_class,
        model_route: Some(
            model_route_label(&fleet_model_route(model_selector.unwrap_or("auto"))).to_string(),
        ),
        // The offline resolver path does not know the concrete sub-agent
        // thinking tier. Leave it absent rather than fabricating one.
        reasoning_effort: None,
        role_source: role_source.map(str::to_string),
        loadout_source: loadout_source.map(str::to_string),
        model_class_source: model_class_source.map(str::to_string),
        model_source: Some(model_source.to_string()),
        source: "resolver".to_string(),
    })
}

/// Plain-string label for a resolved wire protocol (no config type leaks).
fn route_protocol_label(protocol: codewhale_config::route::RequestProtocol) -> &'static str {
    use codewhale_config::route::RequestProtocol;
    match protocol {
        RequestProtocol::ChatCompletions => "chat_completions",
        RequestProtocol::Responses => "responses",
        RequestProtocol::AnthropicMessages => "anthropic_messages",
    }
}

/// Collapse an `inherit` (no-op) loadout to `None` for the receipt.
fn loadout_intent_label(loadout: &codewhale_config::FleetLoadout) -> Option<String> {
    if *loadout == codewhale_config::FleetLoadout::Inherit {
        None
    } else {
        Some(loadout.as_str().to_string())
    }
}

fn model_route_label(route: &ModelRoute) -> &'static str {
    match route {
        ModelRoute::Inherit => "inherit",
        ModelRoute::Faster => "faster",
        ModelRoute::Auto => "auto",
        ModelRoute::Fixed(_) => "fixed",
    }
}

pub(crate) fn fleet_task_prompt(task_spec: &FleetTaskSpec) -> String {
    fleet_task_prompt_with_profile(task_spec, None)
}

pub(crate) fn fleet_task_prompt_with_profiles(
    task_spec: &FleetTaskSpec,
    agent_profiles: &[AgentProfile],
) -> Result<String> {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)?;
    Ok(fleet_task_prompt_with_profile(task_spec, agent_profile))
}

fn fleet_task_prompt_with_profile(
    task_spec: &FleetTaskSpec,
    agent_profile: Option<&AgentProfile>,
) -> String {
    let role = task_spec
        .worker
        .as_ref()
        .and_then(|worker| worker.role.as_deref())
        .or_else(|| agent_profile.map(|profile| profile.profile.role.name.as_str()))
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .unwrap_or("general");
    let mut prompt = String::new();
    prompt.push_str("You have been summoned as a CodeWhale Fleet member (");
    prompt.push_str(role);
    prompt.push_str(") by the Fleet orchestrator.\n\n");
    prompt.push_str("Fleet operating contract:\n");
    prompt.push_str("- Work only the assigned slice; keep sibling or topology assumptions out of your answer.\n");
    prompt.push_str("- Use the policy-gated tools available in this headless worker run.\n");
    prompt.push_str("- Treat the active provider/model route as inherited unless this task or profile pins a model.\n");
    prompt.push_str(
        "- Return concise evidence, gaps, and next actions; the orchestrator will integrate and verify.\n\n",
    );
    prompt.push_str("Fleet task: ");
    prompt.push_str(&task_spec.name);

    if let Some(objective) = task_spec.objective.as_deref() {
        prompt.push_str("\n\nObjective:\n");
        prompt.push_str(objective);
    } else if let Some(description) = task_spec.description.as_deref() {
        prompt.push_str("\n\nObjective:\n");
        prompt.push_str(description);
    }

    prompt.push_str("\n\nInstructions:\n");
    prompt.push_str(&task_spec.instructions);

    if !task_spec.context.is_empty() {
        prompt.push_str("\n\nContext:\n");
        for item in &task_spec.context {
            prompt.push_str("- ");
            prompt.push_str(item);
            prompt.push('\n');
        }
    }

    if !task_spec.input_files.is_empty() {
        prompt.push_str("\nInput files:\n");
        for path in &task_spec.input_files {
            prompt.push_str("- ");
            prompt.push_str(&path.display().to_string());
            prompt.push('\n');
        }
    }

    if let Some(agent_profile) = agent_profile {
        prompt.push_str("\nFleet profile: ");
        prompt.push_str(&agent_profile.id);
        if let Some(display_name) = agent_profile.display_name.as_deref() {
            prompt.push_str(" (");
            prompt.push_str(display_name);
            prompt.push(')');
        }
        if let Some(description) = agent_profile.description.as_deref() {
            prompt.push_str("\nProfile description:\n");
            prompt.push_str(description);
        }
        if let Some(instructions) = agent_profile.profile.role.instructions.as_deref() {
            prompt.push_str("\nProfile instructions:\n");
            prompt.push_str(instructions);
        }
    }

    prompt
}

fn resolve_task_agent_profile<'a>(
    task_spec: &FleetTaskSpec,
    agent_profiles: &'a [AgentProfile],
) -> Result<Option<&'a AgentProfile>> {
    let Some(profile_id) = task_spec
        .worker
        .as_ref()
        .and_then(|worker| worker.agent_profile.as_deref())
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Ok(None);
    };
    let Some(profile) = agent_profiles
        .iter()
        .find(|profile| profile.id == profile_id)
    else {
        bail!(
            "fleet task {} references unknown agent profile {profile_id:?}",
            task_spec.id
        );
    };
    Ok(Some(profile))
}

fn effective_fleet_role(
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> Option<String> {
    effective_fleet_role_with_source(worker_profile, agent_profile).0
}

fn effective_fleet_role_with_source(
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> (Option<String>, Option<&'static str>) {
    worker_profile
        .and_then(|worker| worker.role.as_deref())
        .map(str::trim)
        .filter(|role| !role.is_empty())
        .map(str::to_string)
        .map(|role| (Some(role), Some("task.role")))
        .unwrap_or_else(|| {
            agent_profile
                .map(|profile| {
                    (
                        Some(profile.profile.role.name.clone()),
                        Some("agent_profile.role"),
                    )
                })
                .unwrap_or((None, None))
        })
}

fn effective_fleet_loadout_with_source(
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> (codewhale_config::FleetLoadout, Option<&'static str>) {
    if let Some(model_class) = worker_profile
        .and_then(|worker| worker.model_class.as_deref())
        .and_then(non_empty_trimmed)
    {
        return (
            codewhale_config::FleetLoadout::from_name(model_class),
            Some("task.model_class"),
        );
    }
    if let Some(loadout) = worker_profile
        .and_then(|worker| worker.loadout.as_deref())
        .and_then(non_empty_trimmed)
    {
        return (
            codewhale_config::FleetLoadout::from_name(loadout),
            Some("task.loadout"),
        );
    }
    if let Some(loadout) = agent_profile
        .map(|profile| profile.profile.loadout.clone())
        .filter(|loadout| *loadout != codewhale_config::FleetLoadout::Inherit)
    {
        return (loadout, Some("agent_profile.loadout"));
    }
    (codewhale_config::FleetLoadout::Inherit, None)
}

fn effective_fleet_model(
    run_model: &str,
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> String {
    effective_fleet_model_with_source(run_model, worker_profile, agent_profile).0
}

fn effective_fleet_model_with_source(
    run_model: &str,
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> (String, &'static str) {
    if let Some(model) = worker_profile
        .and_then(|worker| worker.model.as_deref())
        .and_then(non_empty_trimmed)
    {
        return (model.to_string(), "task.model");
    }
    if let Some(model) = agent_profile
        .and_then(|profile| profile.profile.model.as_deref())
        .and_then(non_empty_trimmed)
    {
        return (model.to_string(), "agent_profile.model");
    }
    (run_model.to_string(), "run.model")
}

fn task_model_class_with_source(
    worker_profile: Option<&FleetTaskWorkerProfile>,
) -> (Option<String>, Option<&'static str>) {
    worker_profile
        .and_then(|worker| worker.model_class.as_deref())
        .and_then(non_empty_trimmed)
        .map(|model_class| (Some(model_class.to_string()), Some("task.model_class")))
        .unwrap_or((None, None))
}

fn fleet_route_model_selector_with_source(
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> (Option<String>, &'static str) {
    let (model, source) = effective_fleet_model_with_source("auto", worker_profile, agent_profile);
    if model.trim().is_empty() || model.eq_ignore_ascii_case("auto") {
        (None, "resolver.default")
    } else {
        (Some(model), source)
    }
}

/// Map a fleet role name to a `SubAgentType`. Unknown roles default to `General`.
pub(crate) fn fleet_role_to_agent_type(role: Option<&str>) -> SubAgentType {
    match role {
        Some("smoke-runner") => SubAgentType::Verifier,
        Some("scout") => SubAgentType::Explore,
        Some("read-only") => SubAgentType::Explore,
        Some("reviewer") => SubAgentType::Review,
        Some("builder") => SubAgentType::Implementer,
        Some("verifier") | Some("tester") => SubAgentType::Verifier,
        Some("planner") => SubAgentType::Plan,
        Some("explorer") => SubAgentType::Explore,
        // Coordination happens through delegation, which needs the full
        // General surface (#fleet-roster cutover (v0.8.67)).
        Some("manager") | Some("coordinator") => SubAgentType::General,
        // Synthesis is read-only, no shell: it must never fall through to
        // General's full-write posture (#fleet-roster cutover (v0.8.67)).
        Some("synthesizer") | Some("summarizer") | Some("reducer") => SubAgentType::Plan,
        Some("general") | None => SubAgentType::General,
        Some(other) => {
            // Try parsing as a SubAgentType directly
            SubAgentType::from_str(other).unwrap_or(SubAgentType::General)
        }
    }
}

/// Runtime agent type for a roster member: role name first, falling back to
/// the org-chart slot name when the role name is empty (#fleet-roster cutover
/// (v0.8.67)).
pub(crate) fn roster_member_agent_type(member: &AgentProfile) -> SubAgentType {
    let role_name = member.profile.role.name.trim();
    if role_name.is_empty() {
        fleet_role_to_agent_type(Some(member.profile.slot.as_str()))
    } else {
        fleet_role_to_agent_type(Some(role_name))
    }
}

/// Convert a fleet worker profile's tool list into an `AgentWorkerToolProfile`.
fn fleet_tool_profile(profile: Option<&FleetTaskWorkerProfile>) -> AgentWorkerToolProfile {
    match profile {
        Some(p) if !p.tools.is_empty() => AgentWorkerToolProfile::Explicit(p.tools.clone()),
        _ => AgentWorkerToolProfile::Inherited,
    }
}

fn fleet_worker_runtime_profile(
    agent_type: &SubAgentType,
    tool_profile: &AgentWorkerToolProfile,
    model: &str,
    spawn_depth: u32,
    max_spawn_depth: u32,
) -> WorkerRuntimeProfile {
    let mut profile = WorkerRuntimeProfile::for_role(agent_type.clone());
    profile.tools = match tool_profile {
        AgentWorkerToolProfile::Inherited => ToolScope::Inherit,
        AgentWorkerToolProfile::Explicit(tools) => ToolScope::Explicit(tools.clone()),
    };
    // Concrete pin -> Fixed; unset/"auto" -> Inherit (follow the operator).
    profile.model = fleet_model_route(model);
    profile.max_spawn_depth = max_spawn_depth.saturating_sub(spawn_depth);
    profile.background = true;
    profile
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Resolve a fleet slot's model route. Model selection is operator-centric and
/// concrete: a slot either pins an explicit model (`Fixed`) or inherits the
/// operator/session model (`Inherit`). The legacy `FleetLoadout` model classes
/// (fast/balanced/strong/…) are retired and no longer influence routing — a
/// `loadout` param is retained only where a caller still threads one through,
/// and is ignored. If a user wants a cheaper or stronger model for a slot, they
/// pick it explicitly rather than relying on class magic.
pub(crate) fn fleet_model_route(model: &str) -> ModelRoute {
    let model = model.trim();
    if !model.is_empty() && !model.eq_ignore_ascii_case("auto") {
        ModelRoute::Fixed(model.to_string())
    } else {
        ModelRoute::Inherit
    }
}


/// Apply exec hardening to a worker spec from fleet config (#3027).
///
/// Filters tools against allowed/disallowed lists, caps max_steps to
/// config's max_turns, and returns the objective with system prompt
/// appended when configured.
pub fn apply_exec_hardening(
    mut spec: AgentWorkerSpec,
    exec: &codewhale_config::FleetExecConfig,
) -> AgentWorkerSpec {
    // Cap max_steps to config max_turns
    if exec.max_turns > 0 && exec.max_turns != u32::MAX {
        spec.max_steps = spec.max_steps.min(exec.max_turns);
    }
    spec.max_spawn_depth = exec
        .max_spawn_depth
        .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
    spec.runtime_profile.max_spawn_depth = spec.max_spawn_depth.saturating_sub(spec.spawn_depth);

    // Apply tool filtering
    if !exec.allowed_tools.is_empty() || !exec.disallowed_tools.is_empty() {
        spec.tool_profile = filter_tool_profile(&spec.tool_profile, exec);
        spec.runtime_profile.tools = match &spec.tool_profile {
            AgentWorkerToolProfile::Inherited => ToolScope::Inherit,
            AgentWorkerToolProfile::Explicit(tools) => ToolScope::Explicit(tools.clone()),
        };
    }

    // Append system prompt
    if !exec.append_system_prompt.is_empty() {
        spec.objective = format!(
            "{}\n\n[Policy]\n{}",
            spec.objective, exec.append_system_prompt
        );
    }

    spec
}

pub(crate) fn fleet_effective_permissions_from_worker_spec(
    spec: &AgentWorkerSpec,
) -> FleetEffectivePermissions {
    fleet_effective_permissions_from_runtime_profile(&spec.runtime_profile, None)
}

pub(crate) fn fleet_effective_permissions_for_task(
    task_spec: &FleetTaskSpec,
    agent_profiles: &[AgentProfile],
    spec: &AgentWorkerSpec,
) -> FleetEffectivePermissions {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)
        .ok()
        .flatten();
    fleet_effective_permissions_from_runtime_profile(&spec.runtime_profile, agent_profile)
}

fn fleet_effective_permissions_from_runtime_profile(
    profile: &WorkerRuntimeProfile,
    agent_profile: Option<&AgentProfile>,
) -> FleetEffectivePermissions {
    FleetEffectivePermissions {
        write: profile.permissions.write,
        network: profile.permissions.network,
        shell: shell_policy_label(profile.shell).to_string(),
        tool_scope: tool_scope_label(&profile.tools).to_string(),
        tools: match &profile.tools {
            ToolScope::Inherit => Vec::new(),
            ToolScope::Explicit(tools) => tools.clone(),
        },
        background: profile.background,
        max_spawn_depth: profile.max_spawn_depth,
        profile_id: agent_profile.map(|profile| profile.id.clone()),
        profile_origin: agent_profile
            .map(|profile| profile_origin_label(profile.origin).to_string()),
        source: "worker_runtime_profile".to_string(),
    }
}

fn profile_origin_label(origin: crate::fleet::roster::ProfileOrigin) -> &'static str {
    match origin {
        crate::fleet::roster::ProfileOrigin::BuiltIn => "built_in",
        crate::fleet::roster::ProfileOrigin::Config => "config",
        crate::fleet::roster::ProfileOrigin::Workspace => "workspace",
    }
}

fn shell_policy_label(shell: crate::worker_profile::ShellPolicy) -> &'static str {
    match shell {
        crate::worker_profile::ShellPolicy::None => "none",
        crate::worker_profile::ShellPolicy::ReadOnly => "read_only",
        crate::worker_profile::ShellPolicy::Full => "full",
    }
}

fn tool_scope_label(tools: &ToolScope) -> &'static str {
    match tools {
        ToolScope::Inherit => "inherit",
        ToolScope::Explicit(_) => "explicit",
    }
}

/// Filter a tool profile against allowed/disallowed lists.
fn filter_tool_profile(
    profile: &AgentWorkerToolProfile,
    exec: &codewhale_config::FleetExecConfig,
) -> AgentWorkerToolProfile {
    match profile {
        AgentWorkerToolProfile::Explicit(tools) => {
            let filtered: Vec<String> = tools
                .iter()
                .filter(|t| {
                    // If allowed_tools is non-empty, only keep tools in the list
                    if !exec.allowed_tools.is_empty() && !exec.allowed_tools.contains(t) {
                        return false;
                    }
                    // Disallowed tools always win
                    !exec.disallowed_tools.contains(t)
                })
                .cloned()
                .collect();
            AgentWorkerToolProfile::Explicit(filtered)
        }
        AgentWorkerToolProfile::Inherited => {
            // Inherited profiles can't be filtered at spec time;
            // the sub-agent spawn path applies tool filtering.
            AgentWorkerToolProfile::Inherited
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_protocol::fleet::FleetHostSpec;

    fn fleet_task(id: &str, worker: Option<FleetTaskWorkerProfile>) -> FleetTaskSpec {
        FleetTaskSpec {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            objective: Some(format!("Complete {id}")),
            instructions: format!("do {id}"),
            worker,
            workspace: None,
            input_files: Vec::new(),
            context: Vec::new(),
            budget: None,
            tags: Vec::new(),
            expected_artifacts: Vec::new(),
            scorer: None,
            retry_policy: None,
            alert_policy: None,
            timeout_seconds: None,
            metadata: Default::default(),
        }
    }

    fn worker_profile(
        agent_profile: Option<&str>,
        role: Option<&str>,
        loadout: Option<&str>,
        model_class: Option<&str>,
        model: Option<&str>,
        tools: Vec<&str>,
    ) -> FleetTaskWorkerProfile {
        FleetTaskWorkerProfile {
            agent_profile: agent_profile.map(str::to_string),
            role: role.map(str::to_string),
            loadout: loadout.map(str::to_string),
            model_class: model_class.map(str::to_string),
            model: model.map(str::to_string),
            tool_profile: None,
            tools: tools.into_iter().map(str::to_string).collect(),
            capabilities: Vec::new(),
        }
    }

    fn agent_profile(
        id: &str,
        role: &str,
        instructions: Option<&str>,
        loadout: codewhale_config::FleetLoadout,
    ) -> AgentProfile {
        AgentProfile {
            id: id.to_string(),
            display_name: Some(format!("{role} profile")),
            description: Some(format!("{role} description")),
            profile: codewhale_config::FleetProfile {
                slot: codewhale_config::FleetSlot::from_name(role),
                role: codewhale_config::FleetRole {
                    name: role.to_string(),
                    description: Some(format!("{role} role")),
                    instructions: instructions.map(str::to_string),
                },
                loadout,
                model: None,
                permissions: codewhale_config::FleetProfilePermissions::default(),
                delegation: codewhale_config::FleetDelegationHints::default(),
            },
            source: std::path::PathBuf::from(format!("{id}.toml")),
            origin: crate::fleet::roster::ProfileOrigin::Workspace,
        }
    }

    #[test]
    fn fleet_role_smoke_runner_maps_to_verifier() {
        assert_eq!(
            fleet_role_to_agent_type(Some("smoke-runner")),
            SubAgentType::Verifier
        );
    }

    #[test]
    fn fleet_role_read_only_maps_to_explore() {
        assert_eq!(
            fleet_role_to_agent_type(Some("read-only")),
            SubAgentType::Explore
        );
    }

    #[test]
    fn fleet_role_reviewer_maps_to_review() {
        assert_eq!(
            fleet_role_to_agent_type(Some("reviewer")),
            SubAgentType::Review
        );
    }

    #[test]
    fn fleet_role_builder_maps_to_implementer() {
        assert_eq!(
            fleet_role_to_agent_type(Some("builder")),
            SubAgentType::Implementer
        );
    }

    #[test]
    fn fleet_role_none_maps_to_general() {
        assert_eq!(fleet_role_to_agent_type(None), SubAgentType::General);
    }

    #[test]
    fn fleet_role_manager_and_coordinator_map_to_general() {
        assert_eq!(
            fleet_role_to_agent_type(Some("manager")),
            SubAgentType::General
        );
        assert_eq!(
            fleet_role_to_agent_type(Some("coordinator")),
            SubAgentType::General
        );
    }

    #[test]
    fn fleet_role_synthesizer_family_maps_to_read_only_plan() {
        // A synthesizer must never fall through to General's full-write
        // posture; Plan is read-only with no shell.
        for role in ["synthesizer", "summarizer", "reducer"] {
            assert_eq!(
                fleet_role_to_agent_type(Some(role)),
                SubAgentType::Plan,
                "role {role}"
            );
        }
    }

    #[test]
    fn roster_member_agent_type_uses_role_then_slot() {
        let member = agent_profile(
            "synthesizer",
            "synthesizer",
            None,
            codewhale_config::FleetLoadout::Fast,
        );
        assert_eq!(roster_member_agent_type(&member), SubAgentType::Plan);

        let mut slot_only = agent_profile(
            "custom-summarizer",
            "summarizer",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        slot_only.profile.role.name = String::new();
        assert_eq!(
            slot_only.profile.slot,
            codewhale_config::FleetSlot::Summarizer
        );
        assert_eq!(roster_member_agent_type(&slot_only), SubAgentType::Plan);
    }

    #[test]
    fn unknown_role_maps_to_general() {
        assert_eq!(
            fleet_role_to_agent_type(Some("nonexistent-role")),
            SubAgentType::General
        );
    }

    #[test]
    fn resolve_fleet_route_mints_secret_free_snapshot_from_resolver() {
        let task = fleet_task(
            "route-1",
            Some(worker_profile(
                None,
                Some("builder"),
                Some("fast"),
                None,
                None,
                vec!["read_file"],
            )),
        );
        let route = resolve_fleet_route(&task, &[]).expect("default route should resolve offline");

        // Honest, non-empty route shape from the resolver.
        assert!(!route.provider_id.is_empty());
        assert!(!route.provider_kind.is_empty());
        assert!(!route.wire_model_id.is_empty());
        assert_eq!(route.protocol, "chat_completions");
        assert_eq!(route.role.as_deref(), Some("builder"));
        // The receipt still records the configured loadout as intent metadata,
        // but the actual route is honest: an unpinned slot inherits the operator
        // model. Model classes no longer route to a faster/other model.
        assert_eq!(route.loadout.as_deref(), Some("fast"));
        assert_eq!(route.model_class, None);
        assert_eq!(route.model_route.as_deref(), Some("inherit"));
        assert_eq!(route.reasoning_effort, None);
        assert_eq!(route.role_source.as_deref(), Some("task.role"));
        assert_eq!(route.loadout_source.as_deref(), Some("task.loadout"));
        assert_eq!(route.model_class_source, None);
        assert_eq!(route.model_source.as_deref(), Some("resolver.default"));
        assert_eq!(route.source, "resolver");

        // No-secrets: the serialized snapshot carries no credential markers.
        let json = serde_json::to_string(&route).unwrap();
        let haystack = json.to_ascii_lowercase();
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
                "resolved-route JSON must not contain secret marker {needle:?}: {json}"
            );
        }
    }

    #[test]
    fn resolve_fleet_route_omits_inherit_loadout() {
        // No loadout/model_class intent → `inherit` collapses to None, never an
        // "inherit" string on the receipt.
        let task = fleet_task(
            "route-2",
            Some(worker_profile(
                None,
                Some("scout"),
                None,
                None,
                None,
                vec!["read_file"],
            )),
        );
        let route = resolve_fleet_route(&task, &[]).expect("route should resolve");
        assert_eq!(route.role.as_deref(), Some("scout"));
        assert!(route.loadout.is_none());
        assert_eq!(route.loadout_source, None);
        assert_eq!(route.model_route.as_deref(), Some("inherit"));
        assert_eq!(route.model_source.as_deref(), Some("resolver.default"));
    }

    #[test]
    fn resolve_fleet_route_records_model_class_and_profile_sources() {
        let mut profile = agent_profile(
            "audit",
            "reviewer",
            None,
            codewhale_config::FleetLoadout::Review,
        );
        profile.profile.model = Some("deepseek-v4-flash".to_string());
        let task = fleet_task(
            "route-profile",
            Some(worker_profile(
                Some("audit"),
                None,
                None,
                Some("balanced"),
                None,
                vec!["read_file"],
            )),
        );
        let route = resolve_fleet_route(&task, &[profile]).expect("profile route should resolve");

        assert_eq!(route.role.as_deref(), Some("reviewer"));
        assert_eq!(route.role_source.as_deref(), Some("agent_profile.role"));
        assert_eq!(route.loadout.as_deref(), Some("balanced"));
        assert_eq!(route.loadout_source.as_deref(), Some("task.model_class"));
        assert_eq!(route.model_class.as_deref(), Some("balanced"));
        assert_eq!(
            route.model_class_source.as_deref(),
            Some("task.model_class")
        );
        assert_eq!(route.model_source.as_deref(), Some("agent_profile.model"));
        assert_eq!(route.model_route.as_deref(), Some("fixed"));
        assert_eq!(route.wire_model_id, "deepseek-v4-flash");
        assert_eq!(route.reasoning_effort, None);
    }

    #[test]
    fn fleet_tool_profile_empty_uses_inherited() {
        let profile = FleetTaskWorkerProfile {
            agent_profile: None,
            role: None,
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: None,
            tools: vec![],
            capabilities: vec![],
        };
        assert_eq!(
            fleet_tool_profile(Some(&profile)),
            AgentWorkerToolProfile::Inherited
        );
    }

    #[test]
    fn fleet_tool_profile_explicit_passes_tools() {
        let profile = FleetTaskWorkerProfile {
            agent_profile: None,
            role: None,
            loadout: None,
            model_class: None,
            model: None,
            tool_profile: None,
            tools: vec!["cargo".to_string(), "git".to_string()],
            capabilities: vec![],
        };
        assert_eq!(
            fleet_tool_profile(Some(&profile)),
            AgentWorkerToolProfile::Explicit(vec!["cargo".to_string(), "git".to_string()])
        );
    }

    #[test]
    fn fleet_task_prompt_includes_instructions_context_and_input_files() {
        let task = FleetTaskSpec {
            id: "review".to_string(),
            name: "Review protocol".to_string(),
            description: None,
            objective: Some("Find protocol regressions".to_string()),
            instructions: "Read the fleet protocol and report issues.".to_string(),
            worker: None,
            workspace: None,
            input_files: vec![std::path::PathBuf::from("crates/protocol/src/fleet.rs")],
            context: vec!["Keep the report concise.".to_string()],
            budget: None,
            tags: vec![],
            expected_artifacts: vec![],
            scorer: None,
            retry_policy: None,
            alert_policy: None,
            timeout_seconds: None,
            metadata: Default::default(),
        };

        let prompt = fleet_task_prompt(&task);

        assert!(prompt.contains("summoned as a CodeWhale Fleet member (general)"));
        assert!(prompt.contains("Fleet operating contract:"));
        assert!(prompt.contains("keep sibling or topology assumptions out of your answer"));
        assert!(prompt.contains("Review protocol"));
        assert!(prompt.contains("Find protocol regressions"));
        assert!(prompt.contains("Read the fleet protocol and report issues."));
        assert!(prompt.contains("Keep the report concise."));
        assert!(prompt.contains("crates/protocol/src/fleet.rs"));
    }

    #[test]
    fn fleet_worker_spec_resolves_agent_profile_role_prompt_and_loadout() {
        let profile = agent_profile(
            "reviewer",
            "reviewer",
            Some("Focus on regressions and missing tests."),
            codewhale_config::FleetLoadout::Balanced,
        );
        let task = fleet_task(
            "review",
            Some(worker_profile(
                Some("reviewer"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );
        let worker = FleetWorkerSpec {
            id: "worker-1".to_string(),
            name: "Worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };

        let profiles = vec![profile];
        let spec = fleet_task_to_worker_spec_with_profiles(
            "worker-1",
            "run-1",
            &task,
            &worker,
            "auto",
            std::path::Path::new("/tmp"),
            &profiles,
            None,
        )
        .unwrap();

        assert_eq!(spec.role.as_deref(), Some("reviewer"));
        assert_eq!(spec.agent_type, SubAgentType::Review);
        assert!(
            spec.objective
                .contains("summoned as a CodeWhale Fleet member (reviewer)")
        );
        assert!(spec.objective.contains("Fleet profile: reviewer"));
        assert!(
            spec.objective
                .contains("Focus on regressions and missing tests.")
        );
        assert_eq!(spec.runtime_profile.role, SubAgentType::Review);
        // No concrete model pin: the slot inherits the operator/session model.
        // The legacy `Balanced` class no longer downgrades the route.
        assert_eq!(spec.runtime_profile.model, ModelRoute::Inherit);

        let permissions = fleet_effective_permissions_for_task(&task, &profiles, &spec);
        assert_eq!(permissions.profile_id.as_deref(), Some("reviewer"));
        assert_eq!(permissions.profile_origin.as_deref(), Some("workspace"));
        assert_eq!(permissions.source, "worker_runtime_profile");
    }

    #[test]
    fn fleet_worker_spec_rejects_unknown_agent_profile_before_spawn() {
        let task = fleet_task(
            "review",
            Some(worker_profile(
                Some("missing"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );

        let err = validate_task_agent_profiles(&[task], &[])
            .expect_err("unknown agent profile must fail validation");

        assert!(
            err.to_string()
                .contains("references unknown agent profile \"missing\"")
        );
    }

    #[test]
    fn fleet_worker_spec_uses_profile_model_and_task_model_precedence() {
        let mut profile = agent_profile(
            "reviewer",
            "reviewer",
            Some("Focus on regressions and missing tests."),
            codewhale_config::FleetLoadout::Balanced,
        );
        profile.profile.model = Some("glm-5.2".to_string());
        let worker = FleetWorkerSpec {
            id: "worker-1".to_string(),
            name: "Worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };

        let profile_model_spec = fleet_task_to_worker_spec_with_profiles(
            "worker-1",
            "run-1",
            &fleet_task(
                "review",
                Some(worker_profile(
                    Some("reviewer"),
                    None,
                    None,
                    None,
                    None,
                    vec![],
                )),
            ),
            &worker,
            "auto",
            std::path::Path::new("/tmp"),
            &[profile.clone()],
            None,
        )
        .unwrap();

        assert_eq!(profile_model_spec.model, "glm-5.2");
        assert_eq!(
            profile_model_spec.runtime_profile.model,
            ModelRoute::Fixed("glm-5.2".to_string())
        );

        let task_model_spec = fleet_task_to_worker_spec_with_profiles(
            "worker-2",
            "run-1",
            &fleet_task(
                "review",
                Some(worker_profile(
                    Some("reviewer"),
                    None,
                    None,
                    None,
                    Some("deepseek-v4-pro"),
                    vec![],
                )),
            ),
            &worker,
            "auto",
            std::path::Path::new("/tmp"),
            &[profile],
            None,
        )
        .unwrap();

        assert_eq!(task_model_spec.model, "deepseek-v4-pro");
        assert_eq!(
            task_model_spec.runtime_profile.model,
            ModelRoute::Fixed("deepseek-v4-pro".to_string())
        );
    }

    #[test]
    fn fleet_worker_spec_intersects_task_tools_with_parent_runtime_profile() {
        let task = fleet_task(
            "build",
            Some(worker_profile(
                None,
                Some("builder"),
                None,
                Some("fast"),
                None,
                vec!["read_file", "apply_patch"],
            )),
        );
        let worker = FleetWorkerSpec {
            id: "worker-1".to_string(),
            name: "Worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
        parent.tools = ToolScope::Explicit(vec!["read_file".to_string()]);
        parent.max_spawn_depth = 2;

        let spec = fleet_task_to_worker_spec_with_profiles(
            "worker-1",
            "run-1",
            &task,
            &worker,
            "auto",
            std::path::Path::new("/tmp"),
            &[],
            Some(&parent),
        )
        .unwrap();

        assert_eq!(spec.agent_type, SubAgentType::Implementer);
        assert!(!spec.runtime_profile.permissions.write);
        assert!(!spec.runtime_profile.permissions.network);
        assert_eq!(
            spec.runtime_profile.shell,
            crate::worker_profile::ShellPolicy::ReadOnly
        );
        assert_eq!(
            spec.runtime_profile.tools,
            ToolScope::Explicit(vec!["read_file".to_string()])
        );
        // Unpinned slot inherits the operator model (legacy fast class no
        // longer routes to a faster sibling).
        assert_eq!(spec.runtime_profile.model, ModelRoute::Inherit);
        assert_eq!(spec.max_spawn_depth, 1);

        let permissions = fleet_effective_permissions_from_worker_spec(&spec);
        assert!(!permissions.write);
        assert!(!permissions.network);
        assert_eq!(permissions.shell, "read_only");
        assert_eq!(permissions.tool_scope, "explicit");
        assert_eq!(permissions.tools, vec!["read_file".to_string()]);
        assert!(permissions.background);
        assert_eq!(permissions.max_spawn_depth, 1);
        assert_eq!(permissions.source, "worker_runtime_profile");
    }

    #[test]
    fn fleet_worker_spec_defaults_to_shared_subagent_depth() {
        let task = FleetTaskSpec {
            id: "task-1".to_string(),
            name: "Task".to_string(),
            description: None,
            objective: None,
            instructions: "Do the task.".to_string(),
            worker: None,
            workspace: None,
            input_files: vec![],
            context: vec![],
            budget: None,
            tags: vec![],
            expected_artifacts: vec![],
            scorer: None,
            retry_policy: None,
            alert_policy: None,
            timeout_seconds: None,
            metadata: Default::default(),
        };
        let worker = FleetWorkerSpec {
            id: "worker-1".to_string(),
            name: "Worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };

        let spec = fleet_task_to_worker_spec_with_profiles(
            "worker-1",
            "run-1",
            &task,
            &worker,
            "auto",
            std::path::Path::new("/tmp"),
            &[],
            None,
        )
        .expect("worker spec with empty profiles");

        // Root fleet worker runs at depth 0; its budget equals the shared
        // sub-agent default (3) so fleet and sub-agents are one substrate and
        // at least 3 nested delegation levels are afforded.
        assert_eq!(spec.spawn_depth, 0);
        assert_eq!(spec.max_spawn_depth, codewhale_config::DEFAULT_SPAWN_DEPTH);
        assert_eq!(spec.max_spawn_depth, 3);

        // End-to-end reachability: walk the SAME gate the SubAgentRuntime
        // enforces (`would_exceed_depth` = `spawn_depth + 1 > max_spawn_depth`).
        // A depth-0 root must reach 3 nested levels, then stop. This fails if
        // anyone lowers the shared default below 3 (Hunter: afford >= 3).
        let hardened = apply_exec_hardening(spec, &codewhale_config::FleetExecConfig::default());
        let would_exceed = |spawn_depth: u32| spawn_depth + 1 > hardened.max_spawn_depth;
        assert!(
            !would_exceed(0),
            "root (depth 0) must spawn a child at depth 1"
        );
        assert!(!would_exceed(1), "depth-1 child must spawn to depth 2");
        assert!(!would_exceed(2), "depth-2 child must spawn to depth 3");
        assert!(
            would_exceed(3),
            "depth 3 is the afforded ceiling; depth 4 is blocked"
        );
    }

    #[test]
    fn fleet_fanout_role_loadouts_keep_distinct_child_models() {
        let worker = FleetWorkerSpec {
            id: "local-worker".to_string(),
            name: "Local worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };

        let cases = [
            (
                "scout",
                "deepseek-v4-flash",
                SubAgentType::Explore,
                AgentWorkerToolProfile::Explicit(vec![
                    "read_file".to_string(),
                    "grep_files".to_string(),
                ]),
            ),
            (
                "builder",
                "deepseek-v4-pro",
                SubAgentType::Implementer,
                AgentWorkerToolProfile::Explicit(vec![
                    "read_file".to_string(),
                    "apply_patch".to_string(),
                ]),
            ),
            (
                "verifier",
                "deepseek-v4-pro",
                SubAgentType::Verifier,
                AgentWorkerToolProfile::Explicit(vec![
                    "exec_shell".to_string(),
                    "read_file".to_string(),
                ]),
            ),
        ];

        let parent_model = "parent-session-model";
        let mut child_models = std::collections::BTreeSet::new();
        for (role, model, expected_type, expected_tools) in cases {
            let task = FleetTaskSpec {
                id: format!("{role}-task"),
                name: format!("{role} task"),
                description: None,
                objective: Some(format!("{role} objective")),
                instructions: "Complete the assigned fanout lane.".to_string(),
                worker: Some(FleetTaskWorkerProfile {
                    agent_profile: None,
                    role: Some(role.to_string()),
                    loadout: None,
                    model_class: None,
                    model: None,
                    tool_profile: None,
                    tools: match &expected_tools {
                        AgentWorkerToolProfile::Explicit(tools) => tools.clone(),
                        AgentWorkerToolProfile::Inherited => Vec::new(),
                    },
                    capabilities: vec![],
                }),
                workspace: None,
                input_files: vec![],
                context: vec![],
                budget: None,
                tags: vec![],
                expected_artifacts: vec![],
                scorer: None,
                retry_policy: None,
                alert_policy: None,
                timeout_seconds: None,
                metadata: Default::default(),
            };

            let spec = fleet_task_to_worker_spec_with_profiles(
                &format!("{role}-worker"),
                "run-3289",
                &task,
                &worker,
                model,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .expect("worker spec with empty profiles");

            assert_eq!(spec.role.as_deref(), Some(role));
            assert_eq!(spec.agent_type, expected_type, "role {role}");
            assert_eq!(spec.tool_profile, expected_tools, "role {role}");
            assert_eq!(spec.model, model, "role {role}");
            assert_ne!(
                spec.model, parent_model,
                "Fleet fanout child {role} must use its resolved loadout, not blindly inherit"
            );
            assert_eq!(
                spec.runtime_profile.model,
                ModelRoute::Fixed(model.to_string()),
                "role {role}"
            );
            assert_eq!(spec.runtime_profile.role, expected_type, "role {role}");
            child_models.insert(spec.model.clone());
        }
        assert_eq!(
            child_models,
            std::collections::BTreeSet::from([
                "deepseek-v4-flash".to_string(),
                "deepseek-v4-pro".to_string(),
            ]),
            "Fleet fanout should preserve a mixed scout/builder/verifier loadout"
        );
    }

    #[test]
    fn model_route_is_concrete_pin_or_operator_inherit() {
        // Model selection is operator-centric and concrete: an unpinned slot
        // inherits the operator/session model; a concrete model pins exactly
        // that model. There is no provider-specific "faster sibling" magic and
        // no model-class routing — if a user wants a cheaper or stronger model
        // for a slot, they pick it explicitly.
        assert_eq!(fleet_model_route("auto"), ModelRoute::Inherit);
        assert_eq!(fleet_model_route(""), ModelRoute::Inherit);
        assert_eq!(fleet_model_route("  "), ModelRoute::Inherit);
        assert_eq!(
            fleet_model_route("deepseek-v4-flash"),
            ModelRoute::Fixed("deepseek-v4-flash".to_string()),
        );
        assert_eq!(
            fleet_model_route("glm-5.2"),
            ModelRoute::Fixed("glm-5.2".to_string()),
        );
    }

    #[test]
    fn fleet_slots_with_concrete_models_resolve_to_distinct_models() {
        // Acceptance bar: with one loaded operator model, an operator slot
        // (no model pin) plus two slots each pinned to a different model must
        // resolve to THREE distinct models — not all the operator's model.
        let operator_model = "glm-5.2";
        let worker = FleetWorkerSpec {
            id: "worker-1".to_string(),
            name: "Worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };
        let cases = [
            ("operator", None, operator_model),
            ("reviewer", Some("deepseek-v4-flash"), "deepseek-v4-flash"),
            ("builder", Some("kimi-k2"), "kimi-k2"),
        ];
        let mut resolved = std::collections::BTreeSet::new();
        for (role, pinned_model, expected_model) in cases {
            let task = fleet_task(
                role,
                Some(worker_profile(None, Some(role), None, None, pinned_model, vec![])),
            );
            let spec = fleet_task_to_worker_spec_with_profiles(
                &format!("{role}-w"),
                "run-x",
                &task,
                &worker,
                operator_model,
                std::path::Path::new("/tmp"),
                &[],
                None,
            )
            .expect("worker spec");
            assert_eq!(spec.model, expected_model, "slot {role} effective model");
            assert_eq!(
                spec.runtime_profile.model,
                ModelRoute::Fixed(expected_model.to_string()),
                "slot {role} route pins its resolved model",
            );
            resolved.insert(spec.model.clone());
        }
        assert_eq!(
            resolved.len(),
            3,
            "operator + two pinned slots must resolve to three distinct models"
        );
    }

    #[test]
    fn exec_hardening_caps_max_steps_to_max_turns() {
        let spec = AgentWorkerSpec {
            worker_id: "w1".to_string(),
            run_id: "r1".to_string(),
            parent_run_id: None,
            session_name: None,
            objective: "test".to_string(),
            role: None,
            agent_type: SubAgentType::General,
            model: "auto".to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            git_branch: None,
            context_mode: "fresh".to_string(),
            fork_context: false,
            tool_profile: AgentWorkerToolProfile::Inherited,
            runtime_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
            max_steps: 1000,
            spawn_depth: 0,
            max_spawn_depth: 0,
        };
        let exec = codewhale_config::FleetExecConfig {
            max_turns: 50,
            ..Default::default()
        };
        let hardened = apply_exec_hardening(spec, &exec);
        assert_eq!(hardened.max_steps, 50);
    }

    #[test]
    fn exec_hardening_applies_and_clamps_spawn_depth() {
        let spec = AgentWorkerSpec {
            worker_id: "w1".to_string(),
            run_id: "r1".to_string(),
            parent_run_id: None,
            session_name: None,
            objective: "test".to_string(),
            role: None,
            agent_type: SubAgentType::General,
            model: "auto".to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            git_branch: None,
            context_mode: "fresh".to_string(),
            fork_context: false,
            tool_profile: AgentWorkerToolProfile::Inherited,
            runtime_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
            max_steps: 1000,
            spawn_depth: 0,
            max_spawn_depth: 0,
        };

        let exec = codewhale_config::FleetExecConfig {
            max_spawn_depth: 2,
            ..Default::default()
        };
        let hardened = apply_exec_hardening(spec.clone(), &exec);
        assert_eq!(hardened.max_spawn_depth, 2);

        let exec = codewhale_config::FleetExecConfig {
            max_spawn_depth: 99,
            ..Default::default()
        };
        let hardened = apply_exec_hardening(spec.clone(), &exec);
        assert_eq!(
            hardened.max_spawn_depth,
            codewhale_config::MAX_SPAWN_DEPTH_CEILING
        );

        let exec = codewhale_config::FleetExecConfig {
            max_spawn_depth: 0,
            ..Default::default()
        };
        let hardened = apply_exec_hardening(spec, &exec);
        assert_eq!(hardened.max_spawn_depth, 0);
    }

    #[test]
    fn exec_hardening_filters_disallowed_tools() {
        let profile = AgentWorkerToolProfile::Explicit(vec![
            "read_file".to_string(),
            "exec_shell".to_string(),
            "git_diff".to_string(),
        ]);
        let exec = codewhale_config::FleetExecConfig {
            disallowed_tools: vec!["exec_shell".to_string()],
            ..Default::default()
        };
        let filtered = filter_tool_profile(&profile, &exec);
        assert_eq!(
            filtered,
            AgentWorkerToolProfile::Explicit(
                vec!["read_file".to_string(), "git_diff".to_string(),]
            )
        );
    }

    #[test]
    fn exec_hardening_allowed_tools_acts_as_allowlist() {
        let profile = AgentWorkerToolProfile::Explicit(vec![
            "read_file".to_string(),
            "exec_shell".to_string(),
            "git_diff".to_string(),
        ]);
        let exec = codewhale_config::FleetExecConfig {
            allowed_tools: vec!["read_file".to_string(), "git_diff".to_string()],
            ..Default::default()
        };
        let filtered = filter_tool_profile(&profile, &exec);
        assert_eq!(
            filtered,
            AgentWorkerToolProfile::Explicit(
                vec!["read_file".to_string(), "git_diff".to_string(),]
            )
        );
    }

    #[test]
    fn exec_hardening_allowed_plus_disallowed_disallowed_wins() {
        let profile = AgentWorkerToolProfile::Explicit(vec![
            "read_file".to_string(),
            "exec_shell".to_string(),
        ]);
        let exec = codewhale_config::FleetExecConfig {
            allowed_tools: vec!["read_file".to_string(), "exec_shell".to_string()],
            disallowed_tools: vec!["exec_shell".to_string()],
            ..Default::default()
        };
        let filtered = filter_tool_profile(&profile, &exec);
        assert_eq!(
            filtered,
            AgentWorkerToolProfile::Explicit(vec!["read_file".to_string(),])
        );
    }

    #[test]
    fn exec_hardening_appends_system_prompt() {
        let spec = AgentWorkerSpec {
            worker_id: "w1".to_string(),
            run_id: "r1".to_string(),
            parent_run_id: None,
            session_name: None,
            objective: "do the thing".to_string(),
            role: None,
            agent_type: SubAgentType::General,
            model: "auto".to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            git_branch: None,
            context_mode: "fresh".to_string(),
            fork_context: false,
            tool_profile: AgentWorkerToolProfile::Inherited,
            runtime_profile: WorkerRuntimeProfile::for_role(SubAgentType::General),
            max_steps: 100,
            spawn_depth: 0,
            max_spawn_depth: 0,
        };
        let exec = codewhale_config::FleetExecConfig {
            append_system_prompt: "never push to main".to_string(),
            ..Default::default()
        };
        let hardened = apply_exec_hardening(spec, &exec);
        assert!(hardened.objective.contains("do the thing"));
        assert!(hardened.objective.contains("[Policy]"));
        assert!(hardened.objective.contains("never push to main"));
    }
}
