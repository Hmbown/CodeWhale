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
    let loadout = effective_fleet_loadout(worker_profile, agent_profile);
    let (effective_model, model_source) =
        effective_fleet_model_with_source(model, worker_profile, agent_profile);
    let mut requested_runtime = fleet_worker_runtime_profile_for_loadout(
        &agent_type,
        &tool_profile,
        &effective_model,
        0,
        max_spawn_depth,
        &loadout,
        model_source,
    );
    requested_runtime.provider = explicit_fleet_provider_id(agent_profile);
    requested_runtime.reasoning_effort = effective_fleet_reasoning_effort(agent_profile);
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
/// - The provider comes from the resolved agent profile's own explicit
///   `provider` field when it has one (#4093) — a Fleet worker profile can be
///   pinned to a route independent of the parent/current session provider.
///   Absent an explicit pin, the worker profile carries no provider authority
///   and resolution falls back to the existing default scope. Either way, the
///   provider is NEVER inferred by sniffing a substring/prefix out of `model`
///   (EPIC #2608: explicit config only). A task-level `model` selector is
///   forwarded as the model selector. No reasoning/pricing fields are
///   fabricated.
///
/// Returns `None` (never a fabricated route) when resolution fails, so callers
/// degrade gracefully without inventing detail.
pub(crate) fn resolve_fleet_route(
    task_spec: &FleetTaskSpec,
    agent_profiles: &[AgentProfile],
    session_model: Option<&str>,
) -> Option<FleetResolvedRoute> {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)
        .ok()
        .flatten();
    let worker_profile = task_spec.worker.as_ref();
    let (role, role_source) = effective_fleet_role_with_source(worker_profile, agent_profile);
    let (loadout, loadout_source) =
        effective_fleet_loadout_with_source(worker_profile, agent_profile);
    let (model_class, model_class_source) = task_model_class_with_source(worker_profile);

    // Task/profile model pins are visible route intent; next the session
    // route (the operator's model) applies as the run-level fallback; only
    // then does the resolver pick the provider default.
    let (model_selector, model_source) =
        fleet_route_model_selector_with_source(worker_profile, agent_profile, session_model);
    let model_selector = model_selector.as_deref();

    // Resolve within the profile's own explicit provider scope when it has
    // one (#4093); otherwise fall back to the existing default scope (mirrors
    // `ProviderKind::default()`). The resolver is fully offline/hermetic and
    // never reads secrets, env, or config. User-named custom providers need the
    // session Config to resolve their table, so this receipt path omits the
    // route instead of fabricating DeepSeek details for them (#3965).
    let provider = match explicit_fleet_provider_id(agent_profile).as_deref() {
        Some(provider_id) => ApiProvider::parse(provider_id)?,
        None => ApiProvider::Deepseek,
    };
    let candidate = resolve_route_candidate(provider, model_selector, None, None, None).ok()?;

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
            model_route_label(&fleet_model_route_for_loadout(
                model_selector.unwrap_or("auto"),
                &loadout,
            ))
            .to_string(),
        ),
        reasoning_effort: effective_fleet_reasoning_effort(agent_profile),
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

fn effective_fleet_loadout(
    worker_profile: Option<&FleetTaskWorkerProfile>,
    agent_profile: Option<&AgentProfile>,
) -> codewhale_config::FleetLoadout {
    effective_fleet_loadout_with_source(worker_profile, agent_profile).0
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

/// The provider id a resolved agent profile EXPLICITLY pins, if any (#4093).
///
/// This preserves user-named OpenAI-compatible custom providers such as
/// `lm-studio` instead of collapsing them through [`ApiProvider`]. Runtime
/// launch paths can set `Config.provider` to this exact id so the normal config
/// resolver finds `[providers.<id>]` (#3965).
///
/// Returns `None` when no profile names a provider — never invents a DeepSeek
/// default — so launch paths can omit `--provider` and leave profile-less
/// workers on their own session default. EPIC #2608: never inferred from
/// `model`.
pub(crate) fn explicit_fleet_provider_id(agent_profile: Option<&AgentProfile>) -> Option<String> {
    agent_profile
        .and_then(|profile| profile.profile.provider.as_deref())
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .map(str::to_string)
}

/// The built-in provider a resolved agent profile EXPLICITLY pins, if any (#4093).
///
/// This returns `None` (never the DeepSeek default) when no profile names a
/// provider, so call sites can leave `--provider` off the worker argv and
/// preserve today's behavior for profile-less / provider-less workers (they
/// resolve their provider from their own session default). EPIC #2608: never
/// inferred from `model`.
///
/// `pub(crate)` so the interactive-TUI in-process spawn path
/// (`tools::subagent`) resolves the pinned provider from the SAME
/// explicit-only source as the headless `codewhale exec` launch route (#4193),
/// instead of re-deriving it and risking a second, divergent policy. User-named
/// custom providers intentionally return `None` here; launch paths that can
/// carry strings should use [`explicit_fleet_provider_id`].
pub(crate) fn explicit_fleet_provider(agent_profile: Option<&AgentProfile>) -> Option<ApiProvider> {
    explicit_fleet_provider_id(agent_profile)
        .as_deref()
        .and_then(ApiProvider::parse)
}

pub(crate) fn effective_fleet_reasoning_effort(
    agent_profile: Option<&AgentProfile>,
) -> Option<String> {
    agent_profile
        .and_then(|profile| profile.profile.reasoning_effort.as_deref())
        .map(str::trim)
        .filter(|effort| !effort.is_empty())
        .map(str::to_string)
}

/// The explicit reasoning/thinking tier a fleet worker should launch with.
///
/// This is the launch-side twin of the receipt/runtime-profile field: it reads
/// only the resolved AgentProfile tier, so task model overrides can change the
/// model without accidentally inventing a thinking tier.
pub(crate) fn fleet_worker_launch_reasoning_effort(
    task_spec: &FleetTaskSpec,
    agent_profiles: &[AgentProfile],
) -> Option<String> {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)
        .ok()
        .flatten();
    effective_fleet_reasoning_effort(agent_profile)
}

/// The route (model selector + optional explicit provider id) that a fleet
/// worker's actual `codewhale exec` subprocess should launch on (#4093 AC #4).
///
/// This is the launch-side twin of [`resolve_fleet_route`] (the receipt): both
/// read the worker's model from the same task/profile/run precedence
/// ([`effective_fleet_model`]) and the provider from the same explicit-only
/// source ([`explicit_fleet_provider_id`]), so a worker whose profile is pinned
/// to provider B launches on provider B even when the parent session is on
/// provider A.
///
/// - `model`: never empty in practice — falls back to `run_model` when neither
///   the task nor the profile pins a model, matching pre-#4093 dispatch.
/// - `provider`: `Some(provider_id)` ONLY when the resolved agent profile
///   explicitly pins a provider. `None` means "no provider authority" — the
///   caller omits `--provider` and the worker keeps its own session default,
///   preserving today's behavior for profile-less workers. Built-ins use their
///   canonical ids; user-named custom providers preserve the profile's id so
///   `codewhale exec --provider <id>` can resolve `[providers.<id>]`.
pub(crate) fn fleet_worker_launch_route(
    task_spec: &FleetTaskSpec,
    agent_profiles: &[AgentProfile],
    run_model: &str,
) -> (String, Option<String>) {
    let agent_profile = resolve_task_agent_profile(task_spec, agent_profiles)
        .ok()
        .flatten();
    let worker_profile = task_spec.worker.as_ref();
    let model = effective_fleet_model(run_model, worker_profile, agent_profile);
    let provider = explicit_fleet_provider_id(agent_profile);
    (model, provider)
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
    session_model: Option<&str>,
) -> (Option<String>, &'static str) {
    // The session route (operator model) is the run-level fallback, matching
    // the dispatch path where FleetManager::run_model() feeds
    // `effective_fleet_model_with_source`. Empty/"auto" stays resolver-default.
    let run_model = session_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .unwrap_or("auto");
    let (model, source) =
        effective_fleet_model_with_source(run_model, worker_profile, agent_profile);
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
        // General surface (#fleet-roster cutover (v0.8.67)). The operator is
        // the helm of the whole operation (it assigns managers to workflows);
        // the manager is the middle manager of one workflow. Both coordinate,
        // so both get the General surface — explicitly, not by fall-through.
        Some("manager") | Some("coordinator") | Some("operator") => SubAgentType::General,
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
    profile.model = if model == "auto" {
        ModelRoute::Auto
    } else {
        ModelRoute::Fixed(model.to_string())
    };
    profile.max_spawn_depth = max_spawn_depth.saturating_sub(spawn_depth);
    profile.background = true;
    profile
}

fn fleet_worker_runtime_profile_for_loadout(
    agent_type: &SubAgentType,
    tool_profile: &AgentWorkerToolProfile,
    model: &str,
    spawn_depth: u32,
    max_spawn_depth: u32,
    loadout: &codewhale_config::FleetLoadout,
    model_source: &'static str,
) -> WorkerRuntimeProfile {
    let mut profile = fleet_worker_runtime_profile(
        agent_type,
        tool_profile,
        model,
        spawn_depth,
        max_spawn_depth,
    );
    profile.model = if matches!(model_source, "task.model" | "agent_profile.model") {
        fleet_model_route_for_loadout(model, &codewhale_config::FleetLoadout::Inherit)
    } else {
        fleet_model_route_for_loadout("auto", loadout)
    };
    profile
}

fn non_empty_trimmed(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub(crate) fn fleet_model_route_for_loadout(
    model: &str,
    loadout: &codewhale_config::FleetLoadout,
) -> ModelRoute {
    let model = model.trim();
    if !model.is_empty() && !model.eq_ignore_ascii_case("auto") {
        return ModelRoute::Fixed(model.to_string());
    }
    match loadout {
        codewhale_config::FleetLoadout::Inherit => ModelRoute::Inherit,
        codewhale_config::FleetLoadout::Fast => ModelRoute::Faster,
        codewhale_config::FleetLoadout::Custom(_) => ModelRoute::Auto,
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
    // #4042: thread `FleetExecConfig.disallowed_tools` into the runtime profile's
    // deny-list so it is enforced at run time even for `Inherited` tool profiles,
    // which `filter_tool_profile` cannot narrow at spec time. Union with any
    // already-inherited entries (deny never relaxes). The subprocess Fleet exec
    // path separately passes `--disallowed-tools` on the CLI.
    for rule in &exec.disallowed_tools {
        if !spec.runtime_profile.denied_tools.contains(rule) {
            spec.runtime_profile.denied_tools.push(rule.clone());
        }
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
                provider: None,
                reasoning_effort: None,
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
    fn fleet_role_operator_maps_to_general_explicitly() {
        // The operator coordinates the whole operation (assigns managers to
        // workflows), so it needs the full General surface — by an explicit
        // match arm, not the unknown-role fall-through.
        assert_eq!(
            fleet_role_to_agent_type(Some("operator")),
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
        let route =
            resolve_fleet_route(&task, &[], None).expect("default route should resolve offline");

        // Honest, non-empty route shape from the resolver.
        assert!(!route.provider_id.is_empty());
        assert!(!route.provider_kind.is_empty());
        assert!(!route.wire_model_id.is_empty());
        assert_eq!(route.protocol, "chat_completions");
        assert_eq!(route.role.as_deref(), Some("builder"));
        assert_eq!(route.loadout.as_deref(), Some("fast"));
        assert_eq!(route.model_class, None);
        assert_eq!(route.model_route.as_deref(), Some("faster"));
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
        let route = resolve_fleet_route(&task, &[], None).expect("route should resolve");
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
            codewhale_config::FleetLoadout::Inherit,
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
        let route =
            resolve_fleet_route(&task, &[profile], None).expect("profile route should resolve");

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
            codewhale_config::FleetLoadout::Custom("balanced".to_string()),
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
        assert_eq!(spec.runtime_profile.model, ModelRoute::Auto);

        let permissions = fleet_effective_permissions_for_task(&task, &profiles, &spec);
        assert_eq!(permissions.profile_id.as_deref(), Some("reviewer"));
        assert_eq!(permissions.profile_origin.as_deref(), Some("workspace"));
        assert_eq!(permissions.source, "worker_runtime_profile");
    }

    #[test]
    fn fleet_worker_spec_inherits_session_run_model_when_unpinned() {
        // No task-level model, no roster profile model: the run model (the
        // session route — the operator's model) must flow through to the
        // worker spec, so the model picked in /model is the model that runs.
        let task = fleet_task("build", None);
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
            "deepseek-v4-flash",
            std::path::Path::new("/tmp"),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(spec.model, "deepseek-v4-flash");

        // Legacy headless callers with no session still get the auto sentinel.
        let legacy = fleet_task_to_worker_spec_with_profiles(
            "worker-1",
            "run-1",
            &task,
            &worker,
            "auto",
            std::path::Path::new("/tmp"),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(legacy.model, "auto");
    }

    #[test]
    fn resolve_fleet_route_uses_session_model_as_run_fallback() {
        // Route receipts must agree with dispatch: when neither the task nor
        // a roster profile pins a model, the session route is the run-level
        // fallback and the receipt records it came from `run.model`.
        let task = fleet_task("route-session", None);
        let route = resolve_fleet_route(&task, &[], Some("deepseek-v4-flash"))
            .expect("session-model route should resolve offline");
        assert_eq!(route.model_source.as_deref(), Some("run.model"));
        assert_eq!(route.wire_model_id, "deepseek-v4-flash");

        // Task/profile pins still win over the session route.
        let mut profile = agent_profile(
            "audit",
            "reviewer",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        profile.profile.model = Some("deepseek-v4-pro".to_string());
        let pinned_task = fleet_task(
            "route-pinned",
            Some(worker_profile(
                Some("audit"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );
        let pinned = resolve_fleet_route(&pinned_task, &[profile], Some("deepseek-v4-flash"))
            .expect("pinned route should resolve");
        assert_eq!(pinned.model_source.as_deref(), Some("agent_profile.model"));
        assert_eq!(pinned.wire_model_id, "deepseek-v4-pro");
    }

    #[test]
    fn resolve_fleet_route_honors_explicit_profile_provider_not_the_default() {
        // EPIC #2608 / #4093: the resolved provider must come ONLY from the
        // profile's explicit `provider` field — never inferred from a
        // provider-shaped substring in `model`, and never the parent/session
        // route's provider. `deepseek-v4-flash` is deliberately DeepSeek-shaped
        // while the profile pins `openrouter`.
        let mut profile = agent_profile(
            "cross-provider",
            "scout",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        profile.profile.model = Some("deepseek-v4-flash".to_string());
        profile.profile.provider = Some("openrouter".to_string());
        profile.profile.reasoning_effort = Some("max".to_string());
        let task = fleet_task(
            "route-cross-provider",
            Some(worker_profile(
                Some("cross-provider"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );

        // The "parent"/session route is a completely different provider's
        // model, proving the resolved route does not fall back to it.
        let route = resolve_fleet_route(&task, &[profile], Some("deepseek-v4-pro"))
            .expect("cross-provider profile route should resolve");

        assert_eq!(route.model_source.as_deref(), Some("agent_profile.model"));

        // Resolving `openrouter` directly with the same selector is the
        // ground truth for what this route SHOULD produce — comparing
        // against it (rather than hardcoding a wire id) proves the profile's
        // provider actually drove resolution, whatever wire id/aggregator
        // mapping the resolver's catalog assigns.
        let openrouter_candidate = resolve_route_candidate(
            ApiProvider::Openrouter,
            Some("deepseek-v4-flash"),
            None,
            None,
            None,
        )
        .expect("openrouter should resolve the pinned model directly");
        assert_eq!(
            route.wire_model_id,
            openrouter_candidate.wire_model_id.as_str()
        );
        assert_eq!(route.provider_id, openrouter_candidate.provider_id.as_str());
        assert_eq!(
            route.provider_kind,
            openrouter_candidate.provider_kind.as_str()
        );
        assert_eq!(route.reasoning_effort.as_deref(), Some("max"));
        // Differs from DeepSeek — the pre-#4093 hardcoded default AND the
        // parent/session's provider.
        assert_ne!(route.provider_id, "deepseek");
    }

    #[test]
    fn cross_provider_profile_saves_reloads_and_resolves_to_its_own_provider() {
        // Required cross-provider save/load/launch coverage for #4093: create
        // a Fleet profile whose provider differs from the parent/session
        // provider, save it to a real TOML file, reload it from disk through
        // the same loader Fleet uses, then resolve its route and confirm the
        // resolved provider+model are the SAVED ones — never the parent's.
        let draft = crate::fleet::profile::FleetProfileDraft {
            id: "scout-openrouter".to_string(),
            display_name: Some("Scout".to_string()),
            description: Some("Cross-provider scout profile.".to_string()),
            role_hint: "scout".to_string(),
            model_class_hint: None,
            model: Some("deepseek-v4-flash".to_string()),
            provider: Some("openrouter".to_string()),
            reasoning_effort: Some("max".to_string()),
            instructions: None,
        };

        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(draft.file_name()), draft.render_toml()).unwrap();
        let profiles = crate::fleet::profile::load_agent_profiles_from_dir(dir.path())
            .expect("rendered profile TOML loads");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].profile.provider.as_deref(), Some("openrouter"));
        assert_eq!(profiles[0].profile.reasoning_effort.as_deref(), Some("max"));
        assert_eq!(
            profiles[0].profile.model.as_deref(),
            Some("deepseek-v4-flash")
        );

        let task = fleet_task(
            "route-saved-profile",
            Some(worker_profile(
                Some("scout-openrouter"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );

        // "Parent"/session route: a different provider's model entirely, so a
        // fallback to it would be an obvious, loud test failure.
        let route = resolve_fleet_route(&task, &profiles, Some("deepseek-v4-pro"))
            .expect("saved cross-provider profile route should resolve");

        let openrouter_candidate = resolve_route_candidate(
            ApiProvider::Openrouter,
            Some("deepseek-v4-flash"),
            None,
            None,
            None,
        )
        .expect("openrouter should resolve the saved model directly");
        assert_eq!(
            route.wire_model_id,
            openrouter_candidate.wire_model_id.as_str()
        );
        assert_eq!(route.provider_id, openrouter_candidate.provider_id.as_str());
        assert_eq!(route.reasoning_effort.as_deref(), Some("max"));
        assert_ne!(route.provider_id, "deepseek");
    }

    #[test]
    fn resolve_fleet_route_omits_custom_provider_without_config_snapshot() {
        let mut profile = agent_profile(
            "local",
            "scout",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        profile.profile.model = Some("qwen-2.5-7b".to_string());
        profile.profile.provider = Some("lm-studio".to_string());
        let task = fleet_task(
            "custom-receipt",
            Some(worker_profile(
                Some("local"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );

        let route = resolve_fleet_route(&task, &[profile], Some("deepseek-v4-pro"));

        assert!(
            route.is_none(),
            "custom providers need the session Config snapshot; do not fabricate a DeepSeek receipt"
        );
    }

    #[test]
    fn fleet_worker_launch_route_is_explicit_provider_only() {
        // The LAUNCH resolver (twin of the receipt) must emit a provider ONLY
        // when the profile explicitly pins one, and NEVER infer it from a
        // provider-shaped model id (EPIC #2608).

        // 1) Explicit cross-provider pin: model + provider both come from the
        //    profile, not the parent/session model.
        let mut pinned = agent_profile(
            "cross",
            "scout",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        pinned.profile.model = Some("glm-5.2".to_string());
        pinned.profile.provider = Some("openrouter".to_string());
        pinned.profile.reasoning_effort = Some("high".to_string());
        let pinned_task = fleet_task(
            "launch-pinned",
            Some(worker_profile(
                Some("cross"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );
        let pinned_profiles = vec![pinned];
        let (model, provider) =
            fleet_worker_launch_route(&pinned_task, &pinned_profiles, "deepseek-v4-pro");
        assert_eq!(model, "glm-5.2");
        assert_eq!(provider.as_deref(), Some("openrouter"));
        assert_eq!(
            fleet_worker_launch_reasoning_effort(&pinned_task, &pinned_profiles).as_deref(),
            Some("high")
        );

        // 1b) User-named OpenAI-compatible providers are launchable too: keep
        //     the exact provider id so `codewhale exec --provider lm-studio`
        //     can resolve `[providers.lm-studio]` from config (#3965).
        let mut custom = agent_profile(
            "local",
            "scout",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        custom.profile.model = Some("qwen-2.5-7b".to_string());
        custom.profile.provider = Some("lm-studio".to_string());
        let custom_task = fleet_task(
            "launch-custom",
            Some(worker_profile(
                Some("local"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );
        let custom_profiles = vec![custom];
        let (model, provider) =
            fleet_worker_launch_route(&custom_task, &custom_profiles, "deepseek-v4-pro");
        assert_eq!(model, "qwen-2.5-7b");
        assert_eq!(provider.as_deref(), Some("lm-studio"));

        // 2) A DeepSeek-shaped model with NO explicit provider must NOT infer a
        //    provider — provider stays None so the worker keeps its own session
        //    default, and no `--provider` is emitted.
        let mut model_only = agent_profile(
            "modelonly",
            "scout",
            None,
            codewhale_config::FleetLoadout::Inherit,
        );
        model_only.profile.model = Some("deepseek-v4-flash".to_string());
        let model_only_task = fleet_task(
            "launch-model-only",
            Some(worker_profile(
                Some("modelonly"),
                None,
                None,
                None,
                None,
                vec![],
            )),
        );
        let model_only_profiles = vec![model_only];
        let (model, provider) =
            fleet_worker_launch_route(&model_only_task, &model_only_profiles, "deepseek-v4-pro");
        assert_eq!(model, "deepseek-v4-flash");
        assert_eq!(provider, None);
        assert_eq!(
            fleet_worker_launch_reasoning_effort(&model_only_task, &model_only_profiles),
            None
        );

        // 3) No profile at all: run-level model, no provider (unchanged).
        let bare = fleet_task("launch-bare", None);
        let (model, provider) = fleet_worker_launch_route(&bare, &[], "deepseek-v4-pro");
        assert_eq!(model, "deepseek-v4-pro");
        assert_eq!(provider, None);
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
            codewhale_config::FleetLoadout::Inherit,
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
    fn fleet_worker_spec_carries_agent_profile_provider_through_runtime_contract() {
        let mut profile = agent_profile(
            "scout-openrouter",
            "scout",
            Some("Use the OpenRouter scout route."),
            codewhale_config::FleetLoadout::Fast,
        );
        profile.profile.model = Some("deepseek-v4-flash".to_string());
        profile.profile.provider = Some("openrouter".to_string());
        profile.profile.reasoning_effort = Some("max".to_string());
        let task = fleet_task(
            "scout",
            Some(worker_profile(
                Some("scout-openrouter"),
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
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.provider = Some("deepseek".to_string());
        parent.reasoning_effort = Some("low".to_string());
        parent.max_spawn_depth = 3;

        let spec = fleet_task_to_worker_spec_with_profiles(
            "worker-1",
            "run-1",
            &task,
            &worker,
            "deepseek-v4-pro",
            std::path::Path::new("/tmp"),
            &[profile],
            Some(&parent),
        )
        .unwrap();

        assert_eq!(spec.model, "deepseek-v4-flash");
        assert_eq!(
            spec.runtime_profile.model,
            ModelRoute::Fixed("deepseek-v4-flash".to_string())
        );
        assert_eq!(spec.runtime_profile.provider.as_deref(), Some("openrouter"));
        assert_eq!(
            spec.runtime_profile.reasoning_effort.as_deref(),
            Some("max")
        );
        assert_eq!(spec.runtime_profile.max_spawn_depth, 2);
    }

    #[test]
    fn fleet_worker_spec_model_route_precedence_is_task_profile_role_then_session() {
        let worker = FleetWorkerSpec {
            id: "worker-1".to_string(),
            name: "Worker".to_string(),
            host: FleetHostSpec::Local,
            trust_level: None,
            labels: Default::default(),
            capabilities: vec![],
            max_concurrent_tasks: None,
        };
        let run_model = "deepseek-v4-pro";

        let mut profile =
            agent_profile("scout", "scout", None, codewhale_config::FleetLoadout::Fast);
        profile.profile.model = Some("deepseek-v4-flash".to_string());

        let task_model = fleet_task_to_worker_spec_with_profiles(
            "worker-task",
            "run-1",
            &fleet_task(
                "task-model",
                Some(worker_profile(
                    Some("scout"),
                    None,
                    None,
                    None,
                    Some("deepseek-v4.1"),
                    vec![],
                )),
            ),
            &worker,
            run_model,
            std::path::Path::new("/tmp"),
            &[profile.clone()],
            None,
        )
        .unwrap();
        assert_eq!(task_model.model, "deepseek-v4.1");
        assert_eq!(
            task_model.runtime_profile.model,
            ModelRoute::Fixed("deepseek-v4.1".to_string())
        );

        let profile_model = fleet_task_to_worker_spec_with_profiles(
            "worker-profile",
            "run-1",
            &fleet_task(
                "profile-model",
                Some(worker_profile(
                    Some("scout"),
                    None,
                    None,
                    None,
                    None,
                    vec![],
                )),
            ),
            &worker,
            run_model,
            std::path::Path::new("/tmp"),
            &[profile],
            None,
        )
        .unwrap();
        assert_eq!(profile_model.model, "deepseek-v4-flash");
        assert_eq!(
            profile_model.runtime_profile.model,
            ModelRoute::Fixed("deepseek-v4-flash".to_string())
        );

        let role_default = fleet_task_to_worker_spec_with_profiles(
            "worker-role",
            "run-1",
            &fleet_task(
                "role-default",
                Some(worker_profile(
                    None,
                    Some("scout"),
                    Some("fast"),
                    None,
                    None,
                    vec![],
                )),
            ),
            &worker,
            run_model,
            std::path::Path::new("/tmp"),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(role_default.model, run_model);
        assert_eq!(role_default.runtime_profile.model, ModelRoute::Faster);

        let inherited = fleet_task_to_worker_spec_with_profiles(
            "worker-inherit",
            "run-1",
            &fleet_task("inherit", None),
            &worker,
            run_model,
            std::path::Path::new("/tmp"),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(inherited.model, run_model);
        assert_eq!(inherited.runtime_profile.model, ModelRoute::Inherit);
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
        assert_eq!(spec.runtime_profile.model, ModelRoute::Faster);
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
                ModelRoute::Inherit,
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
    fn fleet_route_parity_uses_shared_router_candidates() {
        use crate::config::ApiProvider;
        use crate::model_routing::{RouterCandidates, provider_router_candidates};

        // Fleet emits the SAME `ModelRoute` seam the sub-agent assignment path
        // consumes (`SubAgentModelStrength::model_route`: fast -> Faster,
        // same/inherit -> Inherit). No fleet-specific provider/model table is
        // involved — only the shared enum.
        assert_eq!(
            fleet_model_route_for_loadout("auto", &codewhale_config::FleetLoadout::Fast),
            ModelRoute::Faster,
        );
        assert_eq!(
            fleet_model_route_for_loadout("auto", &codewhale_config::FleetLoadout::Inherit),
            ModelRoute::Inherit,
        );
        assert_eq!(
            fleet_model_route_for_loadout(
                "auto",
                &codewhale_config::FleetLoadout::Custom("strong".to_string())
            ),
            ModelRoute::Auto,
        );
        // An explicit model always pins to a Fixed route, regardless of loadout.
        assert_eq!(
            fleet_model_route_for_loadout(
                "deepseek-v4-flash",
                &codewhale_config::FleetLoadout::Custom("strong".to_string())
            ),
            ModelRoute::Fixed("deepseek-v4-flash".to_string()),
        );

        // The sub-agent runtime resolves a `ModelRoute` to a concrete model via
        // `provider_router_candidates` (see `worker_profile_subagent_assignment_route`):
        //   Fixed(m)      -> m
        //   Faster | Auto -> candidates.cheap (else parent)
        //   Inherit       -> parent
        // A fleet worker hands its `ModelRoute` to that same resolution, so a
        // fleet "fast" loadout lands on the provider's cheap sibling.
        let parent = "deepseek-v4-pro";
        let resolve = |route: &ModelRoute, candidates: &RouterCandidates| match route {
            ModelRoute::Fixed(model) => model.clone(),
            ModelRoute::Faster | ModelRoute::Auto => candidates
                .cheap
                .clone()
                .unwrap_or_else(|| parent.to_string()),
            ModelRoute::Inherit => parent.to_string(),
        };

        let deepseek = provider_router_candidates(ApiProvider::Deepseek, parent);
        assert_eq!(
            resolve(
                &fleet_model_route_for_loadout("auto", &codewhale_config::FleetLoadout::Fast),
                &deepseek,
            ),
            "deepseek-v4-flash",
            "fleet fast loadout resolves to the provider cheap sibling via the shared router",
        );

        // A provider with no known fast sibling must keep children on the parent
        // model rather than fabricating a cloud id (#3166 route assertion).
        let no_sibling = provider_router_candidates(ApiProvider::Anthropic, parent);
        assert_eq!(no_sibling.cheap, None);
        assert_eq!(
            resolve(
                &fleet_model_route_for_loadout("auto", &codewhale_config::FleetLoadout::Fast),
                &no_sibling,
            ),
            parent,
            "fast with no provider sibling stays on the parent/default model",
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
