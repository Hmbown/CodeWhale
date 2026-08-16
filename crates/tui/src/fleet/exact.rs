//! Runtime for an **exact** named Fleet (`schema = "exact"`).
//!
//! The saved Fleet is the Fleet that runs. At Workflow start the definition is
//! read from the ordinary fleet search roots, every worker route is
//! **preflighted and frozen**, the attached Reasoning Router service is
//! resolved, and the whole thing is captured into an immutable
//! [`FleetSnapshot`] projected onto the roster/profile machinery the in-process
//! spawn path already uses.
//!
//! Five invariants govern everything below.
//!
//! 1. **Routes freeze first, and are checked while freezing.** Provider
//!    identity, canonical wire model, endpoint, local credential readiness, and
//!    reasoning capability are all resolved before the Workflow starts — and
//!    certainly before any Router is asked anything. Nothing downstream may
//!    move them: not a task option, not the Router.
//! 2. **Admission comes before cost.** A task is resolved against the roster,
//!    checked against gates, and given a concurrency slot *before* the Router
//!    is called. A rejected or capacity-blocked task spends no Router tokens
//!    and discloses nothing to a Router's provider.
//! 3. **Auto is a reasoning decision, and the attached Router makes it.**
//!    `reasoning = "auto"` always goes to the Fleet's Reasoning Router — no
//!    provider-native-adaptive bypass, no legacy model routing, no local
//!    keyword heuristic. A manual tier calls no Router at all.
//! 4. **Ceilings narrow the real child.** The saved permission ceiling is
//!    intersected with the live parent posture and turned into an actual tool
//!    policy the child runtime enforces — not a label on a receipt.
//! 5. **Receipts are truthful and content-free.** The tier a selector picked,
//!    the control a provider actually receives, and what a Router cost are
//!    recorded separately; task text never is.

use std::sync::Arc;

use async_trait::async_trait;
use codewhale_workflow::{
    CapturedReasoningRouter, CredentialReadiness, EffectiveReasoning, EndpointIdentity,
    FleetDocument, FleetRouterRef, FleetSearchRoot, FleetSnapshot, FleetSnapshotMember,
    FleetTaskReceipt, NamedFleetError, PermissionCeiling, PreflightError, PreflightedRoute,
    ProviderReasoningControl, QualifiedFleetId, ReasoningCapability, ReasoningRouterProfile,
    ReasoningTier, ResolvedReasoning, RoutePreflight, RouterAvailability, RouterCallInput,
    RouterCallPlan, RouterIdentity, RoutingDisclosure, ShellCeiling, bounded_routing_payload,
    captured_legacy_inline_router, parse_router_decision, resolve_exact_member_reasoning,
    router_call_plan, router_system_prompt, router_user_message,
};

use crate::config::{ApiProvider, Config};
use crate::fleet::profile::AgentProfile;
use crate::fleet::roster::{FleetRoster, ProfileOrigin};
use crate::llm_client::LlmClient;
use crate::tui::app::ReasoningEffort;

/// Where exact fleet definitions and Reasoning Router profiles are looked up,
/// labelled so an identity can be qualified (`workspace/glm-pair`) instead of
/// silently shadowed.
fn personal_fleet_root() -> anyhow::Result<std::path::PathBuf> {
    codewhale_config::codewhale_home()
}

pub(crate) fn personal_fleet_definitions_dir() -> anyhow::Result<std::path::PathBuf> {
    Ok(personal_fleet_root()?.join("fleets"))
}

#[must_use]
pub(crate) fn fleet_search_roots(workspace: &std::path::Path) -> Vec<FleetSearchRoot> {
    let mut roots = Vec::new();
    if let Ok(home) = personal_fleet_root() {
        roots.push(FleetSearchRoot::new("codewhale_home", home));
    }
    roots.push(FleetSearchRoot::new("workspace", workspace.to_path_buf()));
    roots
}

/// Load a fleet document by (optionally qualified) name from the standard
/// roots. Ambiguity between origins is surfaced, never resolved by shadowing.
pub(crate) fn load_fleet_document(
    name: &str,
    workspace: &std::path::Path,
) -> Result<(FleetDocument, QualifiedFleetId), NamedFleetError> {
    FleetDocument::load_by_name(name, &fleet_search_roots(workspace))
}

// ── Child authority: the ceiling, as the child actually experiences it ───────

/// Tool names that give a model its own reach onto the network.
///
/// `network_tool = false` must remove **all** of them from the child's
/// model-visible surface, not merely block them at call time — a model that can
/// see a tool will try it, and a refusal is a worse experience than an absent
/// capability. The child registry hides denied tools from
/// `tools_for_model` and refuses them in `is_tool_allowed`, so one deny list
/// covers both.
///
/// The `mcp*` wildcards are load-bearing: a remote MCP server's tools arrive
/// under a runtime-generated name, so they cannot be enumerated here and must be
/// matched by prefix. `is_tool_denied` supports `prefix*` globs for exactly this.
pub(crate) const NETWORK_TOOL_DENYLIST: &[&str] = &[
    // Web search / fetch / browse, and the canonical family that fronts them.
    //
    // The `Web` family name itself is deliberately NOT denied. Its `search`
    // and `fetch` actions are the read-only web surface a network-denied
    // member is entitled to (parity with an ordinary scout), and the family
    // is classified read-only at the capability envelope, so removing the
    // *name* from this list grants exactly those two actions and nothing
    // else. What this list removes is every other spelling of the browsing
    // surface: the separate `web.run` browse tool, the legacy `web_search` /
    // `fetch_url` / `wait_for_dev_server` action aliases, and the `web_*` /
    // `web.*` name families, so a deny list that stops at `Web` can never
    // leave `web.run` visible and callable, which is the entire browsing
    // capability by another spelling. The explicit names are kept because
    // they document intent and because two of them (`fetch_url`,
    // `wait_for_dev_server`) are not matched by either glob.
    //
    // The child registry's action seam (`SubAgentToolRegistry::is_action_allowed`)
    // lets a network-denied child keep exactly `Web{search, fetch}` past the
    // denied aliases, and the URL-input guard refuses a URL-addressed
    // `fetch` at dispatch, so the reach stays closed.
    "web_*",
    "web.*",
    "web.run",
    "web_run",
    "web_search",
    "web.fetch",
    "web_fetch",
    "fetch_url",
    "wait_for_dev_server",
    "browse",
    "browser",
    // Networked service tools.
    "github",
    "finance",
    // The RLM session family's two reaching actions.
    //
    // `rlm_open` accepts a `url` and fetches it by calling `FetchUrlTool`
    // *in-process*, under its own name — so denying `fetch_url` never sees the
    // call. `rlm_eval` runs operator-supplied Python against a live kernel,
    // which owns a socket API no inspection of the *call* can bound.
    //
    // Both are denied outright rather than gated on the input. The narrower
    // contract was considered and rejected: `rlm_open` chooses its source from
    // *input fields* (`file_path` / `content` / `url` / `session_object`), not
    // from the action name, and the action-policy seam
    // ([`crate::tools::canonical_action`]) resolves names, not field shapes —
    // it cannot prove a source is local before execution. So this fails closed.
    // A network-denied member loses `rlm` loading and evaluation entirely,
    // including the purely local `file_path` form, and keeps only the bounded
    // metadata actions (`session_objects` / `configure` / `close`), which the
    // per-action alias entries make expressible. See `docs/FLEET.md`.
    "rlm_open",
    "rlm_eval",
    // Every MCP surface, including remote servers registered at runtime.
    "mcp*",
    "start_mcp_server",
    "list_mcp_resources",
    "list_mcp_resource_templates",
    "read_mcp_resource",
];

/// The deny-list entry that stands for "this child has no network".
///
/// The deny list *is* how `network_tool = false` reaches a child registry
/// (through `worker_profile.denied_tools`), so posture is read back off the
/// list rather than carried as a second field that could disagree with it.
/// `fetch_url` is the sentinel because every network denial installs it and no
/// narrower deny list does — the `web_*` / `web.*` globs deliberately do not
/// match it, which is why it is spelled out above.
pub(crate) const NETWORK_DENIAL_SENTINEL: &str = "fetch_url";

/// Tool names that mutate the workspace directly.
///
/// A member whose clamped ceiling says `write = false` must not merely be
/// *labelled* read-only — the mutating tools have to be gone from the surface
/// it can see and call. Only the action aliases are listed, never the `File`
/// family itself: denying `File` would take `read`/`list`/`search` with it, and
/// the registry already resolves `File{action:"write"}` through the alias table
/// to `write_file`, so denying the alias covers both spellings.
///
/// `rlm_eval` is here for the same reason it is on the network list and not for
/// a different one: the Python it runs against a live kernel calls `open(...,
/// "w")` as readily as it opens a socket. It is a mutation primitive that
/// happens to be spelled as an analysis tool, and leaving it on a `write =
/// false` surface would let a read-only member rewrite the workspace while the
/// receipt said otherwise. The rest of the family — including the local
/// `file_path` load — survives a write denial, because reading a large file
/// into a kernel is exactly what a read-only member is for.
pub(crate) const MUTATING_TOOL_DENYLIST: &[&str] = &[
    "write_file",
    "edit_file",
    "apply_patch",
    "fim_edit",
    "revert_turn",
    "rlm_eval",
];

/// The raw shell surface — arbitrary operator-supplied commands.
///
/// A read-only member with `shell = "full"` is the honest-labelling problem
/// this list exists for. `full` was saved so the member could *run checks*, but
/// raw shell is a general mutation primitive: `rm`, `git checkout`, or a `>`
/// redirect writes the workspace just as surely as `write_file`, while the
/// receipt says `write=false`. Denying the raw shell entries and leaving the
/// bounded verification surface (`Run` / `run_tests` / `run_verifiers`) intact
/// keeps the verifier able to do its job under a contract that is true.
/// Scout/reviewer read-only inspection selectively removes only canonical `Bash` from this
/// deny list after the role is known; its input-specific read-only classifier
/// remains the authority for that narrow exception.
///
/// That surface is bounded only in its **default** form, and the distinction is
/// load-bearing: `run_verifiers` accepts a `commands` array of arbitrary
/// `program` + `args` pairs, and `run_tests` accepts a raw `args` string. Either
/// one is a general command primitive by another name — `{"program": "bash",
/// "args": ["-lc", "..."]}` is precisely the raw shell this list just removed.
/// Denying the tools outright would take the verifier's whole purpose with
/// them, so the *unbounded arguments* are refused at the execution seam
/// instead; see `reject_unbounded_verification` in
/// [`crate::tools::subagent`]. The name deny list and that guard are one
/// contract split across the only two places that can each see half of it.
pub(crate) const RAW_SHELL_DENYLIST: &[&str] = &[
    "Bash",
    "exec_shell",
    "exec_shell_wait",
    "exec_wait",
    "exec_shell_interact",
    "exec_interact",
    "exec_shell_cancel",
    "task_shell_start",
    "task_shell_wait",
    // The persistent PTY surface registers as `terminal/run`, `terminal/send`,
    // … — a glob, because the family is open-ended and every member of it is a
    // raw command channel.
    "terminal/*",
];

/// The deny-list entry that stands for "this child has no raw shell".
///
/// Same construction as [`NETWORK_DENIAL_SENTINEL`], and for the same reason:
/// posture is read back off the list that enforces it rather than carried as a
/// second field that could disagree. `exec_shell` is the sentinel because every
/// raw-shell denial installs it and no narrower deny list does.
///
/// Read by the tests that assert the raw-shell denial actually landed. It is
/// deliberately *not* what the execution envelope consults for shell
/// authority — see [`SHELL_AUTHORITY_SENTINEL`] for why those are two
/// different questions.
#[allow(dead_code)]
pub(crate) const RAW_SHELL_SENTINEL: &str = "exec_shell";

/// The built-in verification surface: the workspace's own configured checks.
///
/// Bounded in its arguments (see [`crate::tools::execution_envelope`]) but not
/// free of consequence — every entry forks a process. A member whose shell
/// ceiling is narrower than `full` holds no authority to start one, so this
/// list comes off its surface entirely. A `write = false, shell = "full"`
/// member keeps it, because running the checks is what that preset is for.
pub(crate) const VERIFICATION_SURFACE_DENYLIST: &[&str] = &["Run", "run_tests", "run_verifiers"];

/// The deny-list entry that stands for "this child holds no shell authority".
///
/// Distinct from [`RAW_SHELL_SENTINEL`], and the distinction is the point.
/// `exec_shell` is installed whenever the *raw* shell is removed, which
/// includes the write-denied verifier that still holds shell authority — so
/// reading shell authority off it reports every verifier as shell-less and
/// takes the verification surface away from the one role that exists to use
/// it. `run_tests` is installed only when the shell *ceiling* itself is
/// narrower than `full`, which is exactly the posture that has no authority to
/// start a process.
pub(crate) const SHELL_AUTHORITY_SENTINEL: &str = "run_tests";

/// Execution primitives that are **not** spelled as shell.
///
/// Every entry runs an operator-supplied program or schedules one: `gate_run`
/// takes a command line, the mutating `automation` actions execute or schedule
/// a stored automation with its own cwd and prompt, `start_mcp_server` spawns a
/// process, and `pr_attempt_*` writes durable work state. They are listed here
/// so a write-denied child never *sees* them; the authoritative refusal is
/// capability-derived and lives in [`crate::tools::execution_envelope`], which
/// also covers the ones no list can name — repository plugin tools and MCP
/// server tools registered at runtime.
///
/// Listing the per-action alias rather than the family is deliberate and is
/// what the canonical-action seam exists for: denying `tasks` outright would
/// take `list`/`read` with it, and durable-task bookkeeping is exactly what a
/// read-only member should keep.
pub(crate) const NON_SHELL_EXECUTION_DENYLIST: &[&str] = &[
    "task_gate_run",
    "task_create",
    "task_cancel",
    "pr_attempt_record",
    "pr_attempt_preflight",
    "automation_run",
    "automation_create",
    "automation_update",
    "automation_pause",
    "automation_resume",
    "automation_delete",
    "start_mcp_server",
];

/// Whether a deny rule was installed by an **enforced posture** rather than by
/// operator preference.
///
/// `inherit_disallowed_tools: false` exists so a child can start from a clean
/// surface instead of the session's `--disallowed-tools` taste. It must not be
/// able to drop a rule that expresses a *ceiling*: a Fleet member clamped to
/// `network_tool = false` that spawns a grandchild with
/// `inherit_disallowed_tools: false` would otherwise hand that grandchild the
/// network back, which is a child widening its parent's envelope by asking
/// politely.
#[must_use]
pub(crate) fn is_posture_denial(rule: &str) -> bool {
    [
        NETWORK_TOOL_DENYLIST,
        MUTATING_TOOL_DENYLIST,
        RAW_SHELL_DENYLIST,
        VERIFICATION_SURFACE_DENYLIST,
        NON_SHELL_EXECUTION_DENYLIST,
    ]
    .iter()
    .flat_map(|list| list.iter())
    .any(|entry| entry.eq_ignore_ascii_case(rule.trim()))
}

/// The saved ceiling, intersected with the live parent posture and translated
/// into the concrete knobs a child spawn actually carries.
///
/// This is where [`PermissionCeiling::clamp_to`] has its real caller: an
/// authority is never computed from the Fleet file alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChildAuthority {
    /// The clamped ceiling. Never wider than either input.
    pub(crate) ceiling: PermissionCeiling,
    /// `Some(list)` narrows the child's model-visible surface to exactly
    /// `list`. `Some(vec![])` — the `tools = false` case — means *no tools at
    /// all*, which is what the child registry's empty-allowlist path produces.
    /// `None` means full inheritance from the parent surface.
    pub(crate) allowed_tools: Option<Vec<String>>,
    /// Names/globs the child must never see or call. Deny wins over allow.
    pub(crate) disallowed_tools: Vec<String>,
    /// Spawn write authority implied by the clamped ceiling.
    pub(crate) write_authority: &'static str,
    /// Nested-delegation budget, clamped.
    pub(crate) max_depth: u32,
    /// Canonical posture role that governs the child's tool posture.
    pub(crate) posture_role: &'static str,
}

impl ChildAuthority {
    /// Intersect a saved member ceiling with the live session posture.
    ///
    /// Every field takes the more restrictive side, so a saved Fleet can only
    /// ever narrow live authority. A member saved as `full` inside a read-only
    /// session runs read-only.
    #[must_use]
    pub(crate) fn clamp(member: PermissionCeiling, session: PermissionCeiling) -> Self {
        let ceiling = member.clamp_to(session);

        // `tools = false` is total: an empty allowlist leaves the child with no
        // model-visible tools and nothing it is permitted to call.
        let allowed_tools = (!ceiling.tools).then(Vec::new);

        // The deny list is the union of the parent's restrictions and this
        // member's, so a descendant can never drop something an ancestor
        // imposed.
        let mut disallowed_tools = Vec::new();
        if !ceiling.network_tool {
            disallowed_tools.extend(NETWORK_TOOL_DENYLIST.iter().map(|name| (*name).to_string()));
        }
        // Raw shell requires the ceiling to *say* `shell = "full"`. Any narrower
        // shell posture — `none` (the `analyst` preset) or `read_only` — loses
        // the raw command surface outright. `clamp_for_role` may subsequently
        // retain canonical `Bash` for a named scout/reviewer, whose concrete
        // calls are bounded by the strict read-only classifier.
        //
        // This is deliberately keyed on the shell field rather than only on
        // `write`, and that is the whole repair: the execution envelope reads
        // its `shell` bit back off this deny list
        // ([`RAW_SHELL_SENTINEL`]), so a ceiling whose shell posture never
        // installed a denial was invisible to it. A clamped ceiling of
        // `write = true, shell = none` — which any write-capable member inherits
        // inside a session that has no shell authority — therefore reached the
        // envelope claiming full shell authority and could start a process the
        // ceiling had refused it.
        if !(ceiling.write && ceiling.shell == ShellCeiling::Full) {
            disallowed_tools.extend(RAW_SHELL_DENYLIST.iter().map(|name| (*name).to_string()));
        }
        // Losing the *raw* shell and holding no shell authority at all are two
        // different postures, and only the second one loses the bounded
        // verification surface.
        //
        // A `verifier`/`tester` member (`write = false, shell = "full"`) is the
        // case that separates them: the rule above takes its raw shell away as
        // a mutation control, but the member still holds shell authority and
        // running the workspace's own checks is its entire purpose. A ceiling
        // whose shell posture is narrower than `full` holds no such authority,
        // so for it the checks are just another way to start a process.
        if ceiling.shell != ShellCeiling::Full {
            disallowed_tools.extend(
                VERIFICATION_SURFACE_DENYLIST
                    .iter()
                    .map(|name| (*name).to_string()),
            );
        }
        if !ceiling.write {
            // `write = false` has to be a fact about the child's tool surface,
            // not a word on a receipt, so the mutating file tools go. The raw
            // shell is already gone by the rule above; a named scout/reviewer
            // may regain only canonical Bash in `clamp_for_role`, behind its
            // input-specific read-only classifier. The bounded verification
            // surface (`Run` / `run_tests` / `run_verifiers`) is deliberately
            // left for a full-shell verifier.
            disallowed_tools.extend(
                MUTATING_TOOL_DENYLIST
                    .iter()
                    .map(|name| (*name).to_string()),
            );
            // Removing the shell is not enough on its own. An execution
            // primitive spelled as bookkeeping — a verification gate that takes
            // a command line, an automation that runs one on a schedule, an MCP
            // server that spawns a process — mutates the workspace exactly as
            // well as the shell just removed, while the receipt says
            // `write=false`. These names take them off the visible surface;
            // `crate::tools::execution_envelope` refuses them by capability,
            // including the ones no list can name.
            disallowed_tools.extend(
                NON_SHELL_EXECUTION_DENYLIST
                    .iter()
                    .map(|name| (*name).to_string()),
            );
        }

        Self {
            ceiling,
            allowed_tools,
            disallowed_tools,
            write_authority: if ceiling.write {
                "workspace_write"
            } else {
                "read_only"
            },
            max_depth: ceiling.delegation_depth,
            posture_role: posture_role_for(ceiling),
        }
    }

    /// A stable, content-free fingerprint of the envelope this authority
    /// actually installs.
    ///
    /// This is the value that turns "the Fleet computed a ceiling" into
    /// something a later layer can *check*. It covers every field a spawn
    /// carries — allowlist, deny list, write authority, delegation budget, and
    /// posture role — so a request that drifted between admission, routing, and
    /// construction cannot pass for the one the Fleet resolved. Two authorities
    /// with the same fingerprint install the same child surface; that is the
    /// whole contract.
    ///
    /// Deliberately human-readable rather than hashed: it appears verbatim in
    /// the fail-closed error, and an operator debugging a refused launch should
    /// be able to see which side differs without a lookup table.
    #[must_use]
    pub(crate) fn fingerprint(&self) -> String {
        let allowed = match &self.allowed_tools {
            None => "inherit".to_string(),
            Some(list) if list.is_empty() => "none".to_string(),
            Some(list) => {
                let mut list = list.clone();
                list.sort();
                list.join(",")
            }
        };
        let mut denied = self.disallowed_tools.clone();
        denied.sort();
        denied.dedup();
        format!(
            "v1;posture={};write={};depth={};tools={};network={};shell={};allow={};deny={}",
            self.posture_role,
            self.write_authority,
            self.max_depth,
            self.ceiling.tools,
            self.ceiling.network_tool,
            self.ceiling.shell.as_str(),
            allowed,
            denied.join(","),
        )
    }

    /// [`Self::clamp`], for a member whose **semantic role** is known.
    ///
    /// The two are not interchangeable and the difference is the whole of
    /// blocker eight: the ceiling alone can only say which posture a ceiling
    /// *permits*, while a named role that already fits inside that ceiling is
    /// the posture the roster profile actually installs. Reporting the
    /// ceiling-derived posture for a member the operator named `reviewer` would
    /// put `scout` on the receipt while `reviewer` governed the run.
    ///
    /// The role never widens anything: [`posture_role_for_member`] falls back
    /// to the ceiling-derived posture unless the named role's built-in posture
    /// already fits.
    #[must_use]
    pub(crate) fn clamp_for_role(
        role: &str,
        member: PermissionCeiling,
        session: PermissionCeiling,
    ) -> Self {
        let mut authority = Self::clamp(member, session);
        authority.posture_role = posture_role_for_member(role, authority.ceiling);
        let bounded_inspection_role = matches!(
            role.trim().to_ascii_lowercase().as_str(),
            "scout"
                | "explore"
                | "explorer"
                | "reviewer"
                | "review"
                | "planner"
                | "plan"
                | "planning"
                | "awaiter"
        );
        if bounded_inspection_role && authority.ceiling.shell != ShellCeiling::None {
            // Read-only inspection needs ordinary `git`/`rg`/`gh ... view|list` inspection.
            // Keep one canonical foreground tool and leave every legacy,
            // background, interactive, and terminal alias denied. The Bash
            // spec and the machine authority envelope both reclassify the
            // concrete input, so removing the name denial does not grant an
            // arbitrary command channel.
            authority
                .disallowed_tools
                .retain(|name| !name.eq_ignore_ascii_case("Bash"));
        }
        authority
    }
}

/// The active session's posture, expressed as a ceiling so a saved Fleet
/// ceiling can be intersected with it.
///
/// Read off the live parent runtime rather than assumed: this is the value that
/// makes "a Fleet cannot widen what the operator is currently allowed to do"
/// true at runtime instead of on paper.
#[must_use]
pub(crate) fn session_permission_ceiling(
    runtime: &crate::tools::subagent::SubAgentRuntime,
) -> PermissionCeiling {
    PermissionCeiling {
        write: runtime.worker_profile.permissions.write,
        network_tool: runtime.worker_profile.permissions.network
            && runtime.agent_tool_surface_options.web_search_enabled,
        shell: session_shell_ceiling(runtime.worker_profile.shell, runtime.allow_shell),
        delegation_depth: runtime.worker_profile.max_spawn_depth,
        // The session side never withholds the tool bit: a parent that has
        // reached this code is running a Workflow, which is itself a tool call,
        // so `tools = false` here could only ever be a contradiction. The
        // narrowing that matters — `tools = false` on a *member* ceiling — comes
        // from the saved Fleet and survives the clamp untouched.
        tools: true,
    }
}

fn session_shell_ceiling(
    shell: crate::worker_profile::ShellPolicy,
    allow_shell: bool,
) -> ShellCeiling {
    match shell {
        crate::worker_profile::ShellPolicy::None => ShellCeiling::None,
        crate::worker_profile::ShellPolicy::ReadOnly => ShellCeiling::ReadOnly,
        crate::worker_profile::ShellPolicy::Full if allow_shell => ShellCeiling::Full,
        crate::worker_profile::ShellPolicy::Full => ShellCeiling::None,
    }
}

/// The posture role for one member: its own semantic role when that role's
/// built-in posture already fits inside the saved ceiling, and the
/// ceiling-derived posture otherwise.
///
/// Both halves matter. Deriving the posture from the ceiling alone is what
/// stops an arbitrary fleet role (`auditor`) from falling through
/// [`crate::fleet::worker_runtime::fleet_role_to_agent_type`]'s unknown-role
/// arm onto the full-write General surface. But applying it unconditionally
/// threw away a role the operator did name: a `reviewer` or `consultant`
/// member has a built-in posture no wider than `read_only`, and flattening it
/// to `scout` silently swapped that role's system prompt and step budget for
/// another one's.
///
/// The fit test is a strict subset check against the ceiling, so this can only
/// ever pick a role that is *already* permitted — it never widens.
#[must_use]
pub(crate) fn posture_role_for_member(role: &str, ceiling: PermissionCeiling) -> &'static str {
    canonical_role_within_ceiling(role, ceiling).unwrap_or_else(|| posture_role_for(ceiling))
}

/// The canonical Fleet role `role` names, if its built-in posture is no wider
/// than `ceiling`.
fn canonical_role_within_ceiling(role: &str, ceiling: PermissionCeiling) -> Option<&'static str> {
    use crate::tools::subagent::FleetRole;

    if !ceiling.tools {
        // No tools at all: the narrowest posture is the only honest one, and
        // the empty allowlist makes the choice moot anyway.
        return None;
    }
    let canonical = FleetRole::from_str(role)?;
    if matches!(canonical, FleetRole::Custom) {
        // `custom` is defined by an explicit allowlist supplied at spawn time,
        // not by a saved ceiling. Never derive it from a fleet role name.
        return None;
    }

    let posture = crate::worker_profile::WorkerRuntimeProfile::for_role(canonical.clone());
    let role_shell = match posture.shell {
        crate::worker_profile::ShellPolicy::None => ShellCeiling::None,
        crate::worker_profile::ShellPolicy::ReadOnly => ShellCeiling::ReadOnly,
        crate::worker_profile::ShellPolicy::Full => ShellCeiling::Full,
    };
    if posture.permissions.write && !ceiling.write {
        return None;
    }
    // Full-shell roles (verifier, reviewer, scout) do not fit a narrower
    // ceiling — their job needs that shell. A read-only probe default
    // (planner) intersects with the ceiling instead of flattening the
    // named role into scout.
    if role_shell == ShellCeiling::Full && role_shell > ceiling.shell {
        return None;
    }
    Some(canonical.as_str())
}

/// Map a permission ceiling onto the canonical posture role that governs the
/// child's tool surface.
#[must_use]
pub(crate) fn posture_role_for(ceiling: PermissionCeiling) -> &'static str {
    if !ceiling.tools {
        // No tools at all; the narrowest posture, and the allowlist is empty
        // anyway.
        return "scout";
    }
    if ceiling.write {
        return "builder";
    }
    match ceiling.shell {
        ShellCeiling::None | ShellCeiling::ReadOnly => "scout",
        ShellCeiling::Full => "verifier",
    }
}

/// Spawn write authority implied by a member ceiling. A ceiling can only
/// narrow: a member that may not write is launched read-only.
#[must_use]
#[cfg(test)]
pub(crate) fn write_authority_for(ceiling: PermissionCeiling) -> &'static str {
    if ceiling.write {
        "workspace_write"
    } else {
        "read_only"
    }
}

// ── Preflight: freeze the route, and check it while freezing ─────────────────

/// Derive a route's real reasoning capability from the request shaping the
/// client actually performs, rather than from a hand-maintained claims table.
///
/// The probe builds the request body this exact route would receive for every
/// tier and compares them. Two tiers that produce a byte-identical body are not
/// two provider-effective tiers, whatever the selector calls them — this is why
/// Z.AI's GLM routes come back as
/// [`ProviderReasoningControl::EnabledDisabled`] and why nothing here can claim
/// provider-native adaptive for a route whose body does not say so.
#[must_use]
pub(crate) fn reasoning_capability_for_route(
    provider: ApiProvider,
    base_url: &str,
    wire_model: &str,
) -> ReasoningCapability {
    let body_for = |effort: ReasoningEffort| -> String {
        let mut body = serde_json::json!({});
        let value = effort.api_value_for_route(provider, base_url, wire_model);
        crate::client::apply_reasoning_effort(&mut body, value, provider);
        // `reasoning_split` is a transport concern the client sets for every
        // tier; it carries no reasoning depth, so it must not make tiers look
        // distinct or make a no-control route look controllable.
        if let Some(object) = body.as_object_mut() {
            object.remove("reasoning_split");
        }
        body.to_string()
    };

    let off = body_for(ReasoningEffort::Off);
    let above_off: Vec<String> = [
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Max,
    ]
    .into_iter()
    .map(body_for)
    .collect();

    let empty = "{}";
    let all_empty = off == empty && above_off.iter().all(|body| body == empty);

    let mut distinct = above_off.clone();
    distinct.sort();
    distinct.dedup();

    let control = if all_empty {
        ProviderReasoningControl::None
    } else if distinct.len() == 1 && distinct[0] == off && off.contains("adaptive") {
        // Every tier — including off — produces the same adaptive body: the
        // provider genuinely chooses its own depth. Source-backed, not assumed.
        ProviderReasoningControl::NativeAdaptive
    } else if distinct.len() > 1 {
        ProviderReasoningControl::Tiers
    } else {
        ProviderReasoningControl::EnabledDisabled
    };

    // What each requested tier actually becomes on the wire, straight from the
    // route normalizer that shapes the real request.
    //
    // This subsumes a min/max floor-and-ceiling and expresses what one cannot:
    // most non-Codex routes coerce `low` and `medium` to `high` while leaving
    // `off` alone (first-party DeepSeek routes are the documented exception —
    // their wire carries a real `low`), and an always-thinking route raises
    // `off` instead. Reporting a `low` a route silently sends as `high` is
    // the invisible substitution receipts exist to prevent, so the map — not
    // a clamp — is the authority.
    let wire_tiers = [
        ReasoningEffort::Off,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Max,
    ]
    .map(|effort| {
        tier_of(effort.normalize_for_route(provider, base_url, wire_model))
            .unwrap_or(ReasoningTier::Off)
    });

    ReasoningCapability {
        control,
        min_tier: None,
        max_tier: None,
        wire_tiers: None,
    }
    .with_wire_tiers(wire_tiers)
}

fn tier_of(effort: ReasoningEffort) -> Option<ReasoningTier> {
    match effort {
        ReasoningEffort::Off => Some(ReasoningTier::Off),
        ReasoningEffort::Minimal => Some(ReasoningTier::Low),
        ReasoningEffort::Low => Some(ReasoningTier::Low),
        ReasoningEffort::Medium => Some(ReasoningTier::Medium),
        ReasoningEffort::High => Some(ReasoningTier::High),
        ReasoningEffort::XHigh => Some(ReasoningTier::Max),
        ReasoningEffort::Ultra => Some(ReasoningTier::Max),
        ReasoningEffort::Max => Some(ReasoningTier::Max),
        ReasoningEffort::Auto => None,
    }
}

/// The **provider-facing** reasoning value for one tier on one exact route.
///
/// A tier label (`off`, `max`) is a selector concept; what a request may carry
/// is a provider concept, and the two are not the same string. OpenAI Codex
/// routes spell the top tier `xhigh` and cannot express `off` at all, so
/// placing a bare tier label on a Codex request either sends a value the
/// provider does not accept or silently sends nothing and takes the provider
/// default while the receipt claims the tier. Reading the value back out of the
/// same route normalizer the client uses is what keeps the request and the
/// receipt describing each other.
#[must_use]
pub(crate) fn route_reasoning_setting(
    provider: ApiProvider,
    base_url: &str,
    wire_model: &str,
    tier: ReasoningTier,
) -> String {
    effort_of(tier)
        .as_setting_for_route(provider, base_url, wire_model)
        .to_string()
}

fn effort_of(tier: ReasoningTier) -> ReasoningEffort {
    match tier {
        ReasoningTier::Off => ReasoningEffort::Off,
        ReasoningTier::Low => ReasoningEffort::Low,
        ReasoningTier::Medium => ReasoningEffort::Medium,
        ReasoningTier::High => ReasoningEffort::High,
        ReasoningTier::Max => ReasoningEffort::Max,
    }
}

/// Preflight one exact route: resolve the provider, canonicalize the model,
/// identify the endpoint, decide credential readiness **from local config**,
/// and derive the reasoning capability.
///
/// No provider is contacted. Everything here is a configuration lookup, which
/// is what makes it safe to run before the operator's gates have fired.
pub(crate) fn preflight_route(
    member_id: &str,
    provider: &str,
    model: &str,
    config: &Config,
) -> Result<PreflightedRoute, PreflightError> {
    let identity = config
        .resolve_provider_identity(provider.trim())
        .map_err(|detail| PreflightError::ProviderUnresolved {
            member: member_id.to_string(),
            provider: provider.to_string(),
            detail,
        })?;

    // The canonical wire model, resolved once. The receipt and the child spawn
    // both read this value, so they cannot disagree about what actually ran.
    let wire_model = crate::config::requested_model_for_provider(identity.provider, model.trim())
        .ok_or_else(|| PreflightError::ModelUnresolved {
        member: member_id.to_string(),
        provider: identity.key.clone(),
        model: model.to_string(),
        detail: "not a known model for this provider".to_string(),
    })?;
    crate::config::validate_route(identity.provider, &wire_model).map_err(|detail| {
        PreflightError::ModelUnresolved {
            member: member_id.to_string(),
            provider: identity.key.clone(),
            model: wire_model.clone(),
            detail,
        }
    })?;

    let mut scoped = config.clone();
    scoped.scope_to_provider_identity(&identity);
    let base_url = scoped.deepseek_base_url();

    // Locally decided. A concrete loopback/self-hosted route is keyless by
    // design, and that is a valid, first-class state — not a downgrade and
    // not a missing credential. Ollama Cloud is hosted and falls through to
    // the ordinary credential checks.
    let credential =
        if crate::config::provider_route_is_keyless_self_hosted(identity.provider, &base_url) {
            CredentialReadiness::KeylessLocal
        } else if crate::config::has_api_key_for(&scoped, identity.provider) {
            CredentialReadiness::Configured
        } else {
            // The discriminant only. `Missing { detail }` names the provider table
            // key, which for a custom route is the customer's own string.
            codewhale_telemetry::session_counters()
                .bump_error(codewhale_telemetry::ErrorCounter::AuthPreflightFailed);
            CredentialReadiness::Missing {
                detail: format!("no credential configured for `{}`", identity.key),
            }
        };

    Ok(PreflightedRoute {
        member_id: member_id.to_string(),
        provider_id: identity.key.clone(),
        provider_config_id: identity
            .migrated_legacy_ollama_cloud_route
            .then(|| provider.trim().to_string()),
        provider_kind: if identity.provider == ApiProvider::OllamaCloud {
            identity.provider.as_str().to_string()
        } else {
            format!("{:?}", identity.provider).to_ascii_lowercase()
        },
        declared_model: model.trim().to_string(),
        wire_model: wire_model.clone(),
        endpoint: EndpointIdentity::from_base_url(&base_url),
        credential,
        capability: reasoning_capability_for_route(identity.provider, &base_url, &wire_model),
    })
}

/// Build the client one worker route would actually run on, and throw it away.
///
/// Preflight resolves a route from *configuration*; this proves the same route
/// can be turned into a working client — the step that fails on a malformed
/// base URL, an unusable auth mode, or a transport CodeWhale cannot construct.
/// Doing it at Workflow start, for every member, is what stops a Fleet from
/// paying for a Router decision and only then discovering that the worker it
/// decided for could never have been launched.
///
/// The client is deliberately not retained: the spawn path builds the child's
/// own client from the member's roster profile, and keeping a second one here
/// would create two objects that could drift apart.
fn validate_route_client(route: &PreflightedRoute, config: &Config) -> Result<(), String> {
    let mut scoped = config.clone();
    let identity = config.resolve_provider_identity(route.provider_config_id())?;
    scoped.scope_to_provider_identity(&identity);
    crate::client::DeepSeekClient::new(&scoped)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "member `{}` is pinned to provider `{}` (model `{}`), whose client could not be \
                 built on this machine: {error}",
                route.member_id, route.provider_id, route.wire_model
            )
        })
}

// ── The Reasoning Router, as a service ──────────────────────────────────────

/// The seam a Reasoning Router call goes through. Implemented live against the
/// provider client, and by a fixture in tests so the whole reasoning path is
/// exercised without a network.
#[async_trait]
pub(crate) trait FleetRouterCaller: Send + Sync + std::fmt::Debug {
    /// Return the router's raw text response for one worker task.
    async fn decide(&self, input: &RouterCallInput) -> Result<String, String>;

    /// The Router service's exact identity, for the receipt.
    fn identity(&self) -> RouterIdentity;
}

/// A Reasoning Router bound to its own exact preflighted route.
#[derive(Clone)]
pub(crate) struct LiveFleetRouter {
    client: crate::client::DeepSeekClient,
    captured: CapturedReasoningRouter,
    route: PreflightedRoute,
    /// The Router route's provider kind and base URL, kept so the call's
    /// reasoning value can be shaped by the *actual* configured route rather
    /// than by a generic tier label. Never serialized — the base URL can carry
    /// a credential and receipts are durable.
    provider: ApiProvider,
    base_url: String,
    /// What the Router call is actually made at, plus the four-sided disclosure
    /// for the receipt. Configured by the operator (`off` or `low`), normalized
    /// only against what the Router's own route can express.
    call: RouterCallPlan,
}

impl std::fmt::Debug for LiveFleetRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveFleetRouter")
            .field("router", &self.captured.qualified())
            .field("provider", &self.route.provider_id)
            .field("model", &self.route.wire_model)
            .field("call_reasoning", &self.call.tier)
            .field("client", &"<redacted>")
            .finish()
    }
}

impl LiveFleetRouter {
    /// Resolve the Router service's exact configured route and build its client.
    ///
    /// A Router that cannot be resolved is an error here — at Workflow start,
    /// before any worker is dispatched — not a silent downgrade to legacy
    /// routing. Readiness is decided from local configuration; no live probe.
    pub(crate) fn bind(
        captured: &CapturedReasoningRouter,
        config: &Config,
    ) -> Result<Self, RouterBindError> {
        let route = preflight_route(
            &captured.id,
            &captured.route.provider,
            &captured.route.model,
            config,
        )
        .map_err(|error| RouterBindError {
            reason: error.to_string(),
        })?;
        route.require_ready().map_err(|error| RouterBindError {
            reason: error.to_string(),
        })?;

        let identity = config
            .resolve_provider_identity(route.provider_config_id())
            .map_err(|detail| RouterBindError {
                reason: format!(
                    "reasoning router provider `{}` did not resolve: {detail}",
                    route.provider_id
                ),
            })?;
        let mut scoped = config.clone();
        scoped.scope_to_provider_identity(&identity);
        let base_url = scoped.deepseek_base_url();
        let client =
            crate::client::DeepSeekClient::new(&scoped).map_err(|error| RouterBindError {
                reason: format!(
                    "reasoning router provider `{}` client could not be built: {error}",
                    route.provider_id
                ),
            })?;

        let call = router_call_plan(captured.requested_call_reasoning, &route.capability);

        Ok(Self {
            client,
            captured: captured.clone(),
            route,
            provider: identity.provider,
            base_url,
            call,
        })
    }

    /// The preflighted Router route, for cross-provider disclosure.
    #[must_use]
    pub(crate) fn route(&self) -> &PreflightedRoute {
        &self.route
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RouterBindError {
    pub(crate) reason: String,
}

#[async_trait]
impl FleetRouterCaller for LiveFleetRouter {
    fn identity(&self) -> RouterIdentity {
        RouterIdentity::from_captured(
            &self.captured,
            Some(&self.route),
            Some(self.call.disclosure.clone()),
        )
    }

    async fn decide(&self, input: &RouterCallInput) -> Result<String, String> {
        use crate::models::{ContentBlock, Message, MessageRequest, SystemPrompt};

        // The bounded, redacted summary is transmitted exactly once, in the
        // user turn. The system prompt carries the contract and the frozen
        // route, and no task content at all — sending it twice would double
        // what leaves for this provider while the receipt counted one copy.
        let request = MessageRequest {
            model: self.route.wire_model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: router_user_message(input),
                    cache_control: None,
                }],
            }],
            max_tokens: crate::route_budget::effective_max_output_tokens_for_route(
                self.provider,
                &self.route.wire_model,
                None,
            ),
            system: Some(SystemPrompt::Text(router_system_prompt(input))),
            // A router receives no tools. Ever.
            tools: None,
            tool_choice: None,
            metadata: None,
            thinking: None,
            // The operator-configured call tier remains authoritative. The
            // normal route allowance above leaves room for its hidden
            // reasoning before the small JSON answer is emitted.
            reasoning_effort: Some(route_reasoning_setting(
                self.provider,
                &self.base_url,
                &self.route.wire_model,
                self.call.tier,
            )),
            stream: Some(false),
            temperature: None,
            top_p: None,
        };

        let response = self
            .client
            .create_message(request)
            .await
            .map_err(|error| error.to_string())?;
        if crate::models::is_incomplete_stop_reason(response.stop_reason.as_deref()) {
            return Err(format!(
                "reasoning router response incomplete: provider stop reason `{}`",
                crate::models::stop_reason_detail(response.stop_reason.as_deref())
            ));
        }
        let text = response
            .content
            .into_iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            return Err("reasoning router returned an empty response".to_string());
        }
        Ok(text)
    }
}

// ── The Workflow ───────────────────────────────────────────────────────────

/// An exact Fleet, frozen at Workflow start.
///
/// The snapshot, the preflight, and the roster projected from them are all
/// immutable for the life of the run: editing `fleets/<name>.toml` afterwards
/// changes only the next Workflow.
#[derive(Clone)]
pub(crate) struct ExactFleetWorkflow {
    snapshot: Arc<FleetSnapshot>,
    preflight: Arc<RoutePreflight>,
    roster: Arc<FleetRoster>,
    router: Option<Arc<dyn FleetRouterCaller>>,
    router_unavailable: Option<String>,
}

impl std::fmt::Debug for ExactFleetWorkflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExactFleetWorkflow")
            .field("fleet", &self.snapshot.fleet().qualified())
            .field("members", &self.snapshot.members().len())
            .field("router", &self.router.is_some())
            .finish()
    }
}

/// One member, resolved and admitted — but **not yet routed**.
///
/// This is the value the caller holds between admission and the Router call.
/// Producing it costs nothing: no provider is contacted, so a task that is
/// about to be rejected by a gate or blocked on capacity can be resolved
/// safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactMemberBinding {
    /// Canonical member id — the roster profile id the spawn resolves.
    pub(crate) member_id: String,
    /// Semantic role — what gates, handoffs, and records use.
    pub(crate) member_role: String,
    /// The preflighted, frozen route.
    pub(crate) route: PreflightedRoute,
    /// Whether this member's reasoning comes from the Router.
    pub(crate) requires_router: bool,
    /// The clamped authority the child will actually run under.
    pub(crate) authority: ChildAuthority,
    /// The live session posture this binding was clamped against, kept so the
    /// launch half can **recompute** the authority instead of trusting the copy
    /// it was handed. A binding travels across an await point (gates, a
    /// concurrency slot, a router call); recomputing is what makes a stale or
    /// tampered authority detectable rather than merely improbable.
    pub(crate) session: PermissionCeiling,
}

/// What a launched exact member resolves to, after routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExactMemberLaunch {
    /// Canonical member id; also the roster profile id the spawn resolves.
    pub(crate) member_id: String,
    /// Semantic role, preserved for gates/handoffs/records.
    pub(crate) member_role: String,
    /// Frozen provider id.
    pub(crate) provider: String,
    /// Canonical wire model — the same string the receipt records.
    pub(crate) model: String,
    /// Concrete reasoning setting label for the spawn request.
    pub(crate) thinking: String,
    /// The full requested → effective story, for the receipt.
    pub(crate) reasoning: ResolvedReasoning,
    /// The clamped authority the child runs under.
    pub(crate) authority: ChildAuthority,
    /// The durable, visible receipt for this launch.
    pub(crate) receipt: FleetTaskReceipt,
}

impl ExactFleetWorkflow {
    /// Capture a Workflow from a parsed exact fleet document.
    ///
    /// Everything that can fail locally fails here, before any worker is
    /// dispatched: an unresolvable provider, an unknown model, a missing
    /// credential, an unresolvable Reasoning Router profile, or an `auto`
    /// member with no usable Router.
    pub(crate) fn capture(
        document: &FleetDocument,
        id: QualifiedFleetId,
        captured_at: impl Into<String>,
        config: Option<&Config>,
        search_roots: &[FleetSearchRoot],
    ) -> Result<Self, String> {
        let exact = document
            .exact()
            .ok_or_else(|| "fleet is not an exact fleet".to_string())?;

        // Resolve the attached Reasoning Router *reference* into the one
        // captured service both forms normalize onto.
        let captured_router = match exact.router_ref() {
            None => None,
            Some(FleetRouterRef::LegacyInline(_)) => captured_legacy_inline_router(exact),
            Some(FleetRouterRef::Profile { name }) => {
                let (profile, router_id) =
                    ReasoningRouterProfile::load_by_name(&name, search_roots).map_err(|error| {
                        format!(
                            "exact fleet `{}` references reasoning router `{name}`, which could \
                             not be loaded: {error}",
                            id.qualified()
                        )
                    })?;
                Some(CapturedReasoningRouter::from_profile(
                    &profile,
                    router_id.origin,
                ))
            }
        };

        // Capture, then immediately verify the hash the receipt will vouch for.
        // `capture` computes it, so this can only fail if the value took a
        // detour through `Deserialize` — but that is exactly the case a receipt
        // must not certify, and checking here means no later caller has to
        // remember to.
        let snapshot = FleetSnapshot::capture(id, document, captured_at, captured_router.clone())
            .and_then(FleetSnapshot::into_verified)
            .map_err(|error| error.to_string())?;

        // Preflight every worker route before anything else can happen.
        let (preflight, router) = Self::preflight_and_bind(&snapshot, captured_router, config)?;

        let roster = Arc::new(FleetRoster::from_members(
            snapshot
                .members()
                .iter()
                .map(|member| {
                    let route = preflight.worker(&member.id);
                    exact_member_profile(member, route, document.source_path())
                })
                .collect(),
        ));

        let router_unavailable = match (snapshot.router(), &router) {
            (Some(_), None) => {
                Some("the fleet's reasoning router could not be bound on this machine".to_string())
            }
            _ => None,
        };

        let workflow = Self {
            snapshot: Arc::new(snapshot),
            preflight: Arc::new(preflight),
            roster,
            router,
            router_unavailable,
        };
        workflow.reject_unusable_auto_members()?;
        Ok(workflow)
    }

    /// Preflight every worker route and bind the Router, or fail the start.
    fn preflight_and_bind(
        snapshot: &FleetSnapshot,
        captured_router: Option<CapturedReasoningRouter>,
        config: Option<&Config>,
    ) -> Result<(RoutePreflight, Option<Arc<dyn FleetRouterCaller>>), String> {
        let Some(config) = config else {
            return Err(format!(
                "exact fleet `{}` cannot start: no session config is available to preflight its \
                 members' providers and models. An exact fleet fails closed here rather than \
                 dispatching a worker onto a route it never verified.",
                snapshot.fleet().qualified()
            ));
        };

        let mut workers = Vec::with_capacity(snapshot.members().len());
        for member in snapshot.members() {
            let route = preflight_route(
                &member.id,
                &member.route.provider,
                &member.route.model,
                config,
            )
            .map_err(|error| {
                format!(
                    "exact fleet `{}` cannot start: {error}",
                    snapshot.fleet().qualified()
                )
            })?;
            route.require_ready().map_err(|error| {
                format!(
                    "exact fleet `{}` cannot start: {error}",
                    snapshot.fleet().qualified()
                )
            })?;
            workers.push(route);
        }

        // Every worker client is constructed and validated **before** the
        // Router is bound, let alone called. A member whose client cannot be
        // built is a start-time failure; discovering it after a Router decision
        // means the operator paid for a routing request for a task that could
        // never have run.
        for route in &workers {
            validate_route_client(route, config).map_err(|error| {
                format!(
                    "exact fleet `{}` cannot start: {error}",
                    snapshot.fleet().qualified()
                )
            })?;
        }

        let mut router: Option<Arc<dyn FleetRouterCaller>> = None;
        let mut router_route = None;
        if let Some(captured) = &captured_router {
            match LiveFleetRouter::bind(captured, config) {
                Ok(live) => {
                    router_route = Some(live.route().clone());
                    router = Some(Arc::new(live));
                }
                Err(error) => {
                    // Recorded rather than raised: a fleet with no `auto`
                    // member does not need its router to be usable, and
                    // failing the whole Workflow for an unused service would
                    // be the wrong trade.
                    if snapshot.has_auto_member() {
                        return Err(format!(
                            "exact fleet `{}` cannot start: member(s) {} request reasoning \
                             `auto` but the fleet's reasoning router is unusable ({}). Fix the \
                             router profile or pin an explicit reasoning tier — exact fleets \
                             never fall back to legacy model routing or a local heuristic.",
                            snapshot.fleet().qualified(),
                            snapshot.auto_member_ids().join(", "),
                            error.reason,
                        ));
                    }
                }
            }
        }

        Ok((RoutePreflight::new(workers, router_route), router))
    }

    /// Fail at Workflow start — not at task launch — when a member requests
    /// `auto` and the fleet has no Router it can actually call.
    fn reject_unusable_auto_members(&self) -> Result<(), String> {
        if !self.snapshot.has_auto_member() || self.router.is_some() {
            return Ok(());
        }
        let reason = self
            .router_unavailable
            .clone()
            .unwrap_or_else(|| "this fleet references no reasoning router".to_string());
        Err(format!(
            "exact fleet `{}` cannot start: member(s) {} request reasoning `auto` but the fleet's \
             reasoning router is unusable ({reason}). Attach a working reasoning router or pin an \
             explicit reasoning tier — exact fleets never fall back to legacy model routing or a \
             local heuristic.",
            self.snapshot.fleet().qualified(),
            self.snapshot.auto_member_ids().join(", "),
        ))
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> &Arc<FleetSnapshot> {
        &self.snapshot
    }

    /// The run-scoped roster projected from the snapshot. Installing this on
    /// the spawn runtime is what makes each member's exact provider/model reach
    /// its child client through the existing provider-pin path (#4093/#4193).
    #[must_use]
    pub(crate) fn roster(&self) -> &Arc<FleetRoster> {
        &self.roster
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn member(&self, id_or_role: &str) -> Option<&FleetSnapshotMember> {
        self.snapshot.member_by_id_or_role(id_or_role)
    }

    /// Human-readable roster listing for "unknown member" errors.
    #[must_use]
    pub(crate) fn member_names(&self) -> String {
        self.snapshot
            .members()
            .iter()
            .map(|member| {
                if member.role == member.id {
                    member.id.clone()
                } else {
                    format!("{} (role {})", member.id, member.role)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Resolve a task's `role`/`profile` to one admitted member, **without
    /// contacting any provider**.
    ///
    /// This is deliberately the cheap half of a launch. It runs before gate
    /// evaluation and before a concurrency slot is taken, so a task that is
    /// about to be rejected or queued costs nothing and discloses nothing.
    ///
    /// A task that names both a `profile` and a `role` which resolve to
    /// different members is **rejected**, not silently resolved by precedence:
    /// the two fields would then disagree about who ran, and the receipt could
    /// only record one of them.
    pub(crate) fn bind_member(
        &self,
        profile: Option<&str>,
        role: Option<&str>,
        session: PermissionCeiling,
    ) -> Result<ExactMemberBinding, String> {
        let fleet = self.snapshot.fleet().qualified();
        let profile = profile.map(str::trim).filter(|key| !key.is_empty());
        let role = role.map(str::trim).filter(|key| !key.is_empty());

        let member = match (profile, role) {
            (None, None) => {
                return Err(format!(
                    "fleet `{fleet}` is an exact fleet: every task must name a member via `role` \
                     or `profile`. Members: {}",
                    self.member_names()
                ));
            }
            (Some(profile), None) => self.lookup(profile)?,
            (None, Some(role)) => self.lookup(role)?,
            (Some(profile), Some(role)) => {
                let by_profile = self.lookup(profile)?;
                let by_role = self.lookup(role)?;
                if by_profile.id != by_role.id {
                    return Err(format!(
                        "fleet `{fleet}`: task names profile `{profile}` (member `{}`) and role \
                         `{role}` (member `{}`), which are different members. A task must name \
                         one member; the two fields cannot disagree about who ran.",
                        by_profile.id, by_role.id
                    ));
                }
                by_profile
            }
        };

        let route = self.preflight.worker(&member.id).ok_or_else(|| {
            format!(
                "fleet `{fleet}`: member `{}` has no preflighted route",
                member.id
            )
        })?;

        Ok(ExactMemberBinding {
            member_id: member.id.clone(),
            member_role: member.role.clone(),
            route: route.clone(),
            requires_router: member.requested_reasoning.is_auto(),
            authority: ChildAuthority::clamp_for_role(&member.role, member.permissions, session),
            session,
        })
    }

    fn lookup(&self, key: &str) -> Result<&FleetSnapshotMember, String> {
        self.snapshot.member_by_id_or_role(key).ok_or_else(|| {
            format!(
                "unknown exact fleet member `{key}` in `{}`. Members: {}",
                self.snapshot.fleet().qualified(),
                self.member_names()
            )
        })
    }

    /// Finish an **already admitted** binding: decide only how hard the already
    /// frozen model thinks, then build the receipt.
    ///
    /// This is the half that can cost money. Calling it means the task has
    /// already passed its gates and holds a concurrency slot.
    pub(crate) async fn route_admitted_task(
        &self,
        binding: &ExactMemberBinding,
        task_summary: &str,
    ) -> Result<ExactMemberLaunch, String> {
        // The receipt built at the end of this function stamps
        // `snapshot.content_hash()` as evidence that this launch matched a saved
        // definition. Verify the hash actually describes the snapshot *before*
        // spending a router call or emitting that claim — an unverified hash is
        // not weaker evidence, it is a false receipt.
        self.snapshot
            .verify_content_hash()
            .map_err(|error| error.to_string())?;

        let member = self.snapshot.member(&binding.member_id).ok_or_else(|| {
            format!(
                "fleet `{}`: member `{}` vanished between admission and launch",
                self.snapshot.fleet().qualified(),
                binding.member_id
            )
        })?;

        // Recompute the authority from the snapshot member and the posture this
        // binding was admitted against, and require it to be *identical* to the
        // one the binding carries.
        //
        // A binding crosses gates, a concurrency wait, and (for `auto` members)
        // a router call before it gets here, so "the authority I was handed" and
        // "the authority this member actually has" are two different claims. The
        // launch below is the value the spawn path consumes, so it must be the
        // recomputed one; the equality check is what turns a divergence into a
        // refused launch instead of a silently widened child.
        let authority =
            ChildAuthority::clamp_for_role(&member.role, member.permissions, binding.session);
        if authority != binding.authority {
            return Err(format!(
                "fleet `{}`: member `{}` resolved a different permission envelope at launch than \
                 at admission, so the launch is refused. admitted={} launched={}",
                self.snapshot.fleet().qualified(),
                binding.member_id,
                binding.authority.fingerprint(),
                authority.fingerprint(),
            ));
        }

        // The route is already frozen and preflighted. Nothing below may move
        // it — not a task option, not the Router.
        let frozen = binding.route.frozen();
        let capability = binding.route.capability;

        let availability = self.router_availability();
        let mut router_identity = None;
        let mut routing_summary: Option<RoutingDisclosure> = None;
        let decision = if binding.requires_router {
            let router = self.router.as_ref().ok_or_else(|| {
                format!(
                    "member `{}` requests reasoning `auto` but fleet `{}` has no usable reasoning \
                     router",
                    binding.member_id,
                    self.snapshot.fleet().qualified()
                )
            })?;
            let cross_provider = self.preflight.crosses_providers(&binding.member_id);
            let payload = bounded_routing_payload(task_summary).with_cross_provider(cross_provider);
            // What actually leaves for the router's provider, recorded so the
            // receipt discloses it — counts and hash only, never the text.
            routing_summary = Some(payload.disclosure().clone());
            router_identity = Some(router.identity());
            let input = RouterCallInput {
                fleet: self.snapshot.fleet().qualified(),
                member_id: binding.member_id.clone(),
                frozen: frozen.clone(),
                payload,
            };
            let raw = router.decide(&input).await.map_err(|error| {
                format!(
                    "reasoning router call failed for member `{}`: {error}",
                    binding.member_id
                )
            })?;
            Some(parse_router_decision(&raw).map_err(|error| {
                format!(
                    "reasoning router returned an unusable decision for member `{}`: {error}",
                    binding.member_id
                )
            })?)
        } else {
            None
        };

        let reasoning = resolve_exact_member_reasoning(
            &binding.member_id,
            &frozen,
            member.requested_reasoning,
            &capability,
            &availability,
            decision.as_ref(),
            router_identity.as_ref(),
        )
        .map_err(|error| error.to_string())?;

        // Every exact launch carries a concrete tier. `auto` is resolved by the
        // router above and the literal sentinel never leaves this function.
        //
        // `NativeAdaptive` is no longer reachable here: removing the bypass
        // (so `auto` always asks the router) also removed the one path that
        // produced it. It used to be launched as `off`, which mislabelled the
        // request — a route choosing its own depth is not a route with thinking
        // disabled. Rather than re-introduce that lie, this fails loudly if the
        // variant ever comes back.
        let thinking = match reasoning.effective() {
            EffectiveReasoning::Tier(tier) => effort_of(tier).as_setting().to_string(),
            EffectiveReasoning::NativeAdaptive => {
                return Err(format!(
                    "member `{}` resolved to provider-native adaptive reasoning, which an exact \
                     fleet launch cannot place on a request. Pin an explicit reasoning tier.",
                    binding.member_id
                ));
            }
        };

        // The durable receipt. Built here, at the one place that knows every
        // side of the decision, so no consumer has to re-derive it.
        let receipt = FleetTaskReceipt::new(
            self.snapshot.fleet().qualified(),
            self.snapshot.schema_kind(),
            self.snapshot.schema_revision(),
            self.snapshot.content_hash(),
            binding.member_id.clone(),
            binding.member_role.clone(),
            &binding.route,
            &reasoning,
            routing_summary,
            binding.authority.ceiling.network_tool,
        )
        // The fingerprint of the envelope this launch installs, carried on the
        // durable receipt so the spawn boundary has something to check against
        // rather than a sentinel it can only assume.
        .with_authority_fingerprint(authority.fingerprint())
        // Semantic role and runtime posture stay two separate facts all the way
        // onto the durable receipt: `member_role` is what the operator named
        // and what gates key on, `posture_role` is the tool surface the clamped
        // ceiling actually permits.
        .with_posture_role(binding.authority.posture_role);

        Ok(ExactMemberLaunch {
            member_id: binding.member_id.clone(),
            member_role: binding.member_role.clone(),
            provider: frozen.provider,
            model: frozen.model,
            thinking,
            reasoning,
            authority,
            receipt,
        })
    }

    fn router_availability(&self) -> RouterAvailability {
        match (&self.router, &self.router_unavailable) {
            (Some(_), _) => RouterAvailability::Ready,
            (None, Some(reason)) => RouterAvailability::Unavailable {
                reason: reason.clone(),
            },
            (None, None) => RouterAvailability::Absent,
        }
    }
}

/// Project one snapshot member onto the roster profile the in-process spawn
/// path already understands.
///
/// Two things here are deliberate and load-bearing:
///
/// - The profile is keyed by **member id**, and the member's **semantic role**
///   is carried as the display name. Role is what gates and records mean; id is
///   what resolves a roster entry. Conflating them would make a gate keyed on
///   `builder` silently miss a member whose id happens to be `implementer`.
/// - The profile's *posture* role name is the member's own role when that
///   role's built-in posture fits inside the saved ceiling, and the
///   ceiling-derived posture otherwise. Posture role is what picks the child's
///   tool surface and system prompt, so an arbitrary fleet role such as
///   `auditor` must not fall through to the full-write General surface — while
///   a real role such as `reviewer` must not be flattened into `scout` when the
///   operator's ceiling already permits it. See
///   [`posture_role_for_member`].
fn exact_member_profile(
    member: &FleetSnapshotMember,
    route: Option<&PreflightedRoute>,
    source: Option<&std::path::Path>,
) -> AgentProfile {
    let posture_role = posture_role_for_member(&member.role, member.permissions);
    let bounded_inspection_shell = matches!(posture_role, "scout" | "reviewer")
        && member.permissions.shell != ShellCeiling::None;
    // The canonical wire model, so the child spawns with exactly what the
    // receipt records.
    let wire_model = route.map_or_else(
        || member.route.model.clone(),
        |route| route.wire_model.clone(),
    );
    let provider = route.map_or_else(
        || member.route.provider.clone(),
        |route| route.provider_config_id().to_string(),
    );

    let profile = codewhale_config::FleetProfile {
        slot: codewhale_config::FleetSlot::Custom(member.role.clone()),
        role: codewhale_config::FleetRole {
            name: posture_role.to_string(),
            description: Some(format!("exact fleet member `{}`", member.id)),
            instructions: None,
        },
        loadout: codewhale_config::FleetLoadout::Inherit,
        model: Some(wire_model.clone()),
        // The exact provider pin is the whole point: it is what makes the
        // child client bind to this member's provider instead of the
        // session's (#4093).
        provider: Some(provider.clone()),
        // Reasoning is decided per task (a member may be `auto`), so it is
        // placed on the spawn request explicitly rather than baked in here.
        reasoning_effort: None,
        permissions: codewhale_config::FleetProfilePermissions {
            // This legacy boolean controls whether Bash is registered at all.
            // Named read-only inspection with a non-none ceiling registers it, while the
            // typed worker profile and per-input classifier keep it ReadOnly.
            allow_shell: member.permissions.shell == ShellCeiling::Full || bounded_inspection_shell,
            trust: false,
            approval_required: true,
        },
        delegation: codewhale_config::FleetDelegationHints {
            max_spawn_depth: Some(member.permissions.delegation_depth),
            max_concurrency: None,
        },
    };

    AgentProfile {
        id: member.id.clone(),
        display_name: Some(member.role.clone()),
        description: Some(format!(
            "Exact fleet member `{}` (role `{}`), pinned to {provider}/{wire_model}.",
            member.id, member.role
        )),
        profile,
        source: source
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("<exact fleet>")),
        origin: ProfileOrigin::Config,
    }
}

// ── Test seams ──────────────────────────────────────────────────────────────

/// A Router that answers with a fixed fixture string, recording what it saw.
///
/// Test-only: it is how the exact-Fleet reasoning path is exercised end to end
/// without a provider call, and how "the router was never called" is asserted.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct StaticFleetRouter {
    response: String,
    identity: RouterIdentity,
    pub(crate) seen: std::sync::Mutex<Vec<RouterCallInput>>,
}

#[cfg(test)]
impl StaticFleetRouter {
    pub(crate) fn new(response: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            response: response.into(),
            identity: RouterIdentity {
                id: "luna-low".to_string(),
                origin: "workspace".to_string(),
                service_kind: codewhale_workflow::REASONING_ROUTER_SERVICE_KIND.to_string(),
                legacy_inline: false,
                provider: "openai".to_string(),
                model: "gpt-5.6-luna".to_string(),
                endpoint: Some(EndpointIdentity::from_base_url("https://api.openai.com/v1")),
                call: Some(
                    router_call_plan(
                        codewhale_workflow::RouterCallReasoning::Low,
                        &ReasoningCapability::tiered(),
                    )
                    .disclosure,
                ),
            },
            seen: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// How many router calls were made. Zero is the assertion that matters for
    /// manual reasoning and for rejected/blocked tasks.
    pub(crate) fn call_count(&self) -> usize {
        self.seen.lock().expect("router log").len()
    }
}

#[cfg(test)]
#[async_trait]
impl FleetRouterCaller for StaticFleetRouter {
    fn identity(&self) -> RouterIdentity {
        self.identity.clone()
    }

    async fn decide(&self, input: &RouterCallInput) -> Result<String, String> {
        self.seen.lock().expect("router log").push(input.clone());
        Ok(self.response.clone())
    }
}

#[cfg(test)]
impl ExactFleetWorkflow {
    /// Build a Workflow with an injected Router and a supplied capability,
    /// skipping provider binding so the reasoning path runs with no network and
    /// no configured provider.
    /// Takes the concrete fixture type rather than `Option<Arc<dyn ...>>`:
    /// `Option` does not coerce its payload, so the unsizing is done once here
    /// instead of at every call site.
    pub(crate) fn for_tests(
        document: &FleetDocument,
        id: QualifiedFleetId,
        router: Option<Arc<StaticFleetRouter>>,
    ) -> Self {
        Self::for_tests_with_capability(document, id, router, ReasoningCapability::tiered())
    }

    pub(crate) fn for_tests_with_capability(
        document: &FleetDocument,
        id: QualifiedFleetId,
        router: Option<Arc<StaticFleetRouter>>,
        capability: ReasoningCapability,
    ) -> Self {
        let exact = document.exact().expect("exact fleet");
        let captured = captured_legacy_inline_router(exact).or_else(|| {
            exact.reasoning_router.as_ref().map(|name| {
                CapturedReasoningRouter::from_profile(
                    &ReasoningRouterProfile::parse(&format!(
                        "name = \"{name}\"\nschema = \"reasoning_router\"\nprovider = \
                         \"openai\"\nmodel = \"gpt-5.6-luna\"\ncall_reasoning = \"low\"\n"
                    ))
                    .expect("router profile"),
                    "workspace",
                )
            })
        });
        let snapshot =
            FleetSnapshot::capture(id, document, "2026-07-26T00:00:00Z", captured.clone())
                .expect("valid roster");

        let workers = snapshot
            .members()
            .iter()
            .map(|member| {
                test_route(
                    &member.id,
                    &member.route.provider,
                    &member.route.model,
                    capability,
                )
            })
            .collect::<Vec<_>>();
        let router_route = captured.as_ref().map(|captured| {
            test_route(
                "router",
                &captured.route.provider,
                &captured.route.model,
                capability,
            )
        });
        let preflight = RoutePreflight::new(workers, router_route);

        let roster = Arc::new(FleetRoster::from_members(
            snapshot
                .members()
                .iter()
                .map(|member| {
                    exact_member_profile(
                        member,
                        preflight.worker(&member.id),
                        document.source_path(),
                    )
                })
                .collect(),
        ));
        Self {
            snapshot: Arc::new(snapshot),
            preflight: Arc::new(preflight),
            roster,
            router: router.map(|router| {
                let router: Arc<dyn FleetRouterCaller> = router;
                router
            }),
            router_unavailable: None,
        }
    }

    /// A Workflow whose Router failed to bind locally — the shape
    /// [`Self::capture`] produces when a Router's provider has no credentials
    /// configured on this machine. No network is involved either way.
    pub(crate) fn for_tests_with_unavailable_router(
        document: &FleetDocument,
        id: QualifiedFleetId,
        reason: &str,
    ) -> Result<Self, String> {
        let mut workflow = Self::for_tests(document, id, None);
        workflow.router_unavailable = Some(reason.to_string());
        workflow.reject_unusable_auto_members()?;
        Ok(workflow)
    }
}

#[cfg(test)]
fn test_route(
    member: &str,
    provider: &str,
    model: &str,
    capability: ReasoningCapability,
) -> PreflightedRoute {
    PreflightedRoute {
        member_id: member.to_string(),
        provider_id: provider.to_string(),
        provider_config_id: None,
        provider_kind: provider.to_string(),
        declared_model: model.to_string(),
        wire_model: model.to_string(),
        endpoint: EndpointIdentity::from_base_url("https://api.example.test/v1"),
        credential: CredentialReadiness::Configured,
        capability,
    }
}

#[cfg(test)]
mod shell_ceiling_tests {
    use super::*;

    fn ceiling(write: bool, shell: ShellCeiling) -> PermissionCeiling {
        PermissionCeiling {
            write,
            network_tool: false,
            shell,
            delegation_depth: 0,
            tools: true,
        }
    }

    fn session() -> PermissionCeiling {
        ceiling(true, ShellCeiling::Full)
    }

    fn denies_raw_shell(authority: &ChildAuthority) -> bool {
        authority
            .disallowed_tools
            .iter()
            .any(|rule| rule == RAW_SHELL_SENTINEL)
    }

    /// The `analyst` preset grants no shell. The envelope reads its shell bit
    /// back off the deny list, so the denial has to actually be installed —
    /// otherwise a shell-less ceiling reaches dispatch claiming full shell
    /// authority and can start a verification process.
    #[test]
    fn a_shell_less_ceiling_installs_the_raw_shell_denial() {
        for shell in [ShellCeiling::None, ShellCeiling::ReadOnly] {
            let authority = ChildAuthority::clamp(ceiling(false, shell), session());
            assert!(
                denies_raw_shell(&authority),
                "{shell:?} must deny raw shell"
            );
        }
    }

    /// The gap this repair closed: a write-capable member inside a session with
    /// no shell authority clamps to `write = true, shell = none`. Keying the
    /// denial on `write` alone left that combination with no denial installed —
    /// and therefore with an envelope that claimed shell authority the ceiling
    /// had refused.
    #[test]
    fn a_write_capable_member_clamped_to_no_shell_still_loses_raw_shell() {
        let authority = ChildAuthority::clamp(
            ceiling(true, ShellCeiling::Full),
            ceiling(true, ShellCeiling::None),
        );

        assert_eq!(authority.ceiling.shell, ShellCeiling::None);
        assert!(authority.ceiling.write, "the write half is unchanged");
        assert!(denies_raw_shell(&authority));
    }

    /// Prior behavior preserved: a `verifier`/`tester` ceiling
    /// (`write = false, shell = "full"`) still loses raw shell, and a fully
    /// write-capable member still keeps it.
    #[test]
    fn the_existing_verifier_and_full_ceilings_are_unchanged() {
        let verifier = ChildAuthority::clamp(ceiling(false, ShellCeiling::Full), session());
        assert!(denies_raw_shell(&verifier));
        assert_eq!(verifier.posture_role, "verifier");

        let full = ChildAuthority::clamp(ceiling(true, ShellCeiling::Full), session());
        assert!(!denies_raw_shell(&full));
        assert_eq!(full.posture_role, "builder");
    }

    #[test]
    fn bounded_inspection_role_keeps_only_classifier_bounded_bash() {
        for role in ["scout", "reviewer", "planner"] {
            let authority = ChildAuthority::clamp_for_role(
                role,
                ceiling(false, ShellCeiling::ReadOnly),
                session(),
            );
            assert!(
                !authority
                    .disallowed_tools
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case("Bash")),
                "{role} keeps canonical Bash for per-input classification"
            );
            for denied in [
                "exec_shell",
                "task_shell_start",
                "task_shell_wait",
                "terminal/*",
                "write_file",
                "apply_patch",
            ] {
                assert!(
                    authority.disallowed_tools.iter().any(|name| name == denied),
                    "{role} must still deny {denied}: {:?}",
                    authority.disallowed_tools
                );
            }
        }

        for role in ["consultant", "verifier"] {
            let authority = ChildAuthority::clamp_for_role(
                role,
                ceiling(false, ShellCeiling::ReadOnly),
                session(),
            );
            assert!(
                authority
                    .disallowed_tools
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case("Bash")),
                "{role} must not gain the read-only inspection exception"
            );
        }

        let parent_shell_off = ChildAuthority::clamp_for_role(
            "scout",
            ceiling(false, ShellCeiling::Full),
            ceiling(true, ShellCeiling::None),
        );
        assert!(
            parent_shell_off
                .disallowed_tools
                .iter()
                .any(|name| name.eq_ignore_ascii_case("Bash")),
            "a named Scout may not turn a parent shell-off ceiling into ReadOnly"
        );
        let planner_parent_shell_off = ChildAuthority::clamp_for_role(
            "planner",
            ceiling(false, ShellCeiling::ReadOnly),
            ceiling(true, ShellCeiling::None),
        );
        assert!(
            planner_parent_shell_off
                .disallowed_tools
                .iter()
                .any(|name| name.eq_ignore_ascii_case("Bash")),
            "a named planner may not turn a parent shell-off ceiling into ReadOnly"
        );
        assert_eq!(planner_parent_shell_off.posture_role, "planner");
        assert_eq!(
            session_shell_ceiling(crate::worker_profile::ShellPolicy::Full, false),
            ShellCeiling::None
        );
    }

    /// #5426 acceptance 2, made mechanical: delegation moves work, never
    /// authority. A read-only scout's own runtime posture is the "session"
    /// its children clamp against, so a `builder` member dispatched from a
    /// read-only parent lands read-only — raw shell gone, ceiling-derived
    /// posture, mutating tools denied — while the delegation itself stays
    /// available (the depth budget is the parent's, not zero). The escape
    /// hatch is work capacity, never a wider envelope.
    #[test]
    fn a_read_only_parents_delegation_never_widens_authority() {
        // The scout's live runtime posture, expressed as the session ceiling
        // a child clamps against: no writes, read-only shell, network kept,
        // one level of delegation budget left.
        let scout_runtime = PermissionCeiling {
            write: false,
            network_tool: true,
            shell: ShellCeiling::ReadOnly,
            delegation_depth: 1,
            tools: true,
        };
        let builder_member = PermissionCeiling {
            write: true,
            network_tool: true,
            shell: ShellCeiling::Full,
            delegation_depth: 1,
            tools: true,
        };
        assert!(builder_member.write && builder_member.shell == ShellCeiling::Full);

        let authority = ChildAuthority::clamp_for_role("builder", builder_member, scout_runtime);

        // Authority does not widen through delegation: the child is read-only.
        assert!(!authority.ceiling.write);
        assert_eq!(authority.ceiling.shell, ShellCeiling::ReadOnly);
        assert_eq!(authority.write_authority, "read_only");
        assert_eq!(
            authority.posture_role, "scout",
            "a write-capable role name must not smuggle write through delegation"
        );
        assert!(denies_raw_shell(&authority));
        for mutating in ["write_file", "apply_patch"] {
            assert!(
                authority
                    .disallowed_tools
                    .iter()
                    .any(|rule| rule == mutating),
                "{mutating} must stay denied for a scout-delegated builder: {:?}",
                authority.disallowed_tools
            );
        }

        // The escape hatch itself stays open: delegation is still possible
        // (the parent's budget is intact). But it is useless for shell:
        // canonical Bash is denied to a delegated child (it is not a bounded
        // inspection role), so a scout can never obtain bash by spawning —
        // the scout's own bounded read-only Bash from #5428 is the only shell
        // path a read-only parent has.
        assert_eq!(authority.max_depth, 1);
        assert!(
            authority
                .disallowed_tools
                .iter()
                .any(|name| name.eq_ignore_ascii_case("Bash")),
            "a scout-delegated child must not gain canonical Bash: {:?}",
            authority.disallowed_tools
        );
    }

    /// The deny list feeds the fingerprint, so a ceiling that now denies more
    /// must fingerprint differently from one that does not. Two postures that
    /// install different surfaces may never share a fingerprint.
    #[test]
    fn the_shell_denial_is_visible_in_the_fingerprint() {
        let no_shell = ChildAuthority::clamp(ceiling(false, ShellCeiling::None), session());
        let full = ChildAuthority::clamp(ceiling(true, ShellCeiling::Full), session());

        assert_ne!(no_shell.fingerprint(), full.fingerprint());
        assert!(no_shell.fingerprint().contains("shell=none"));
    }

    /// Every rule the shell clamp installs is a *posture* denial, so a
    /// grandchild spawned with `inherit_disallowed_tools: false` cannot drop it.
    #[test]
    fn the_installed_shell_denials_are_posture_denials() {
        let authority = ChildAuthority::clamp(ceiling(false, ShellCeiling::None), session());
        for rule in &authority.disallowed_tools {
            assert!(is_posture_denial(rule), "{rule} must be a posture denial");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_workflow::{
        EffectiveReasoningSource, ProviderEffectiveReasoning, RequestedReasoning,
    };

    /// A Fleet that references a saved, reusable Reasoning Router service.
    const GLM_FLEET: &str = r#"
name = "glm-pair"
schema = "exact"
reasoning_router = "luna-low"

[[members]]
id = "implementer"
role = "builder"
provider = "zai"
model = "glm-5"
reasoning = "auto"
permissions = "read_write"

[[members]]
id = "auditor"
role = "reviewer"
provider = "zai"
model = "glm-5"
reasoning = "high"
permissions = "read_only"
"#;

    fn id() -> QualifiedFleetId {
        QualifiedFleetId {
            name: "glm-pair".to_string(),
            origin: "workspace".to_string(),
        }
    }

    fn full_session() -> PermissionCeiling {
        PermissionCeiling::preset("full").expect("preset")
    }

    /// Takes the concrete fixture type: `Option` does not coerce its payload,
    /// so the unsizing to `Arc<dyn FleetRouterCaller>` is spelled out here once
    /// rather than at every call site.
    fn workflow_with(router: Option<Arc<StaticFleetRouter>>, text: &str) -> ExactFleetWorkflow {
        let document = FleetDocument::parse(text).expect("parse");
        ExactFleetWorkflow::for_tests(&document, id(), router)
    }

    #[tokio::test]
    async fn an_auto_member_takes_a_reasoning_only_router_decision_on_a_frozen_route() {
        let router = StaticFleetRouter::new(r#"{"reasoning":"max"}"#);
        let workflow = workflow_with(Some(router.clone()), GLM_FLEET);

        let binding = workflow
            .bind_member(None, Some("builder"), full_session())
            .expect("role resolves");
        assert_eq!(
            router.call_count(),
            0,
            "binding a member must not cost a router call"
        );

        let launch = workflow
            .route_admitted_task(&binding, "refactor three crates")
            .await
            .expect("auto resolves through the router");

        // The route did not move.
        assert_eq!(launch.provider, "zai");
        assert_eq!(launch.model, "glm-5");
        assert_eq!(launch.thinking, "max");
        assert_eq!(launch.member_id, "implementer");
        assert_eq!(launch.member_role, "builder");
        assert_eq!(launch.reasoning.requested(), RequestedReasoning::Auto);
        assert_eq!(
            launch.reasoning.source(),
            EffectiveReasoningSource::FleetRouter
        );

        // The router saw the frozen route as context, never as a question, and
        // received the bounded payload rather than the raw task.
        let seen = router.seen.lock().expect("log");
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].frozen.model, "glm-5");
        assert_eq!(seen[0].member_id, "implementer");
        assert_eq!(seen[0].payload.text(), "refactor three crates");
    }

    /// The semantic role must survive onto the launch and the receipt: a gate
    /// or handoff keyed on `builder` has to still see `builder` even though the
    /// roster resolves the distinct profile id `implementer`.
    #[tokio::test]
    async fn the_semantic_role_survives_while_the_id_addresses_the_roster() {
        let workflow = workflow_with(
            Some(StaticFleetRouter::new(r#"{"reasoning":"low"}"#)),
            GLM_FLEET,
        );

        let binding = workflow
            .bind_member(None, Some("reviewer"), full_session())
            .expect("role lookup");
        assert_eq!(binding.member_id, "auditor");
        assert_eq!(binding.member_role, "reviewer");

        let launch = workflow
            .route_admitted_task(&binding, "read the diff")
            .await
            .expect("launch");
        assert_eq!(launch.receipt.member_id, "auditor");
        assert_eq!(
            launch.receipt.member_role, "reviewer",
            "the receipt records the semantic role, not the profile id"
        );

        // The roster is addressed by id; the role is the display name.
        let entry = workflow.roster().get("auditor").expect("roster entry");
        assert_eq!(entry.display_name.as_deref(), Some("reviewer"));
    }

    /// A task that names a profile and a role belonging to different members is
    /// rejected — the two fields cannot disagree about who ran.
    #[test]
    fn a_conflicting_task_role_and_profile_is_rejected() {
        let workflow = workflow_with(None, GLM_FLEET);

        let err = workflow
            .bind_member(Some("implementer"), Some("reviewer"), full_session())
            .expect_err("conflicting identity");
        assert!(err.contains("different members"), "{err}");
        assert!(err.contains("implementer"), "{err}");
        assert!(err.contains("auditor"), "{err}");

        // Agreeing fields are fine: id plus that member's own role.
        let binding = workflow
            .bind_member(Some("implementer"), Some("builder"), full_session())
            .expect("agreeing identity");
        assert_eq!(binding.member_id, "implementer");
    }

    /// Manual reasoning uses no Router at all — not a call whose answer is
    /// discarded, but zero calls.
    #[tokio::test]
    async fn an_explicit_tier_member_never_calls_the_router() {
        let router = StaticFleetRouter::new(r#"{"reasoning":"off"}"#);
        let workflow = workflow_with(Some(router.clone()), GLM_FLEET);

        let binding = workflow
            .bind_member(Some("auditor"), None, full_session())
            .expect("bind");
        assert!(!binding.requires_router);

        let launch = workflow
            .route_admitted_task(&binding, "read the diff")
            .await
            .expect("explicit tier");

        assert_eq!(launch.thinking, "high");
        assert_eq!(
            launch.reasoning.source(),
            EffectiveReasoningSource::MemberExplicit
        );
        assert_eq!(
            router.call_count(),
            0,
            "an explicit tier must not spend a router call"
        );
        assert!(launch.receipt.router.is_none());
        assert!(launch.receipt.routing_summary.is_none());
        assert!(!launch.receipt.cross_provider_inference);
    }

    /// A task that never reaches admission must never reach the Router. This
    /// is the shape of a gate rejection or a capacity block: the caller binds,
    /// decides not to proceed, and no provider was contacted.
    #[test]
    fn a_task_that_is_never_admitted_costs_no_router_call() {
        let router = StaticFleetRouter::new(r#"{"reasoning":"max"}"#);
        let workflow = workflow_with(Some(router.clone()), GLM_FLEET);

        // Unknown member: rejected during binding, before any cost.
        assert!(
            workflow
                .bind_member(None, Some("wizard"), full_session())
                .is_err()
        );
        // Conflicting identity: likewise.
        assert!(
            workflow
                .bind_member(Some("implementer"), Some("reviewer"), full_session())
                .is_err()
        );
        // A valid binding that the caller then abandons (gate reject / no slot).
        let _binding = workflow
            .bind_member(None, Some("builder"), full_session())
            .expect("valid binding");

        assert_eq!(
            router.call_count(),
            0,
            "no router call may happen before a task is admitted"
        );
    }

    #[tokio::test]
    async fn a_router_that_tries_to_move_the_route_fails_the_launch() {
        let workflow = workflow_with(
            Some(StaticFleetRouter::new(
                r#"{"reasoning":"max","model":"glm-4"}"#,
            )),
            GLM_FLEET,
        );
        let binding = workflow
            .bind_member(None, Some("builder"), full_session())
            .expect("bind");

        let err = workflow
            .route_admitted_task(&binding, "anything")
            .await
            .expect_err("a route mutation must fail the launch");
        assert!(err.contains("frozen"), "{err}");
    }

    #[tokio::test]
    async fn a_duplicate_reasoning_key_fails_the_launch() {
        let workflow = workflow_with(
            Some(StaticFleetRouter::new(
                r#"{"reasoning":"off","reasoning":"max"}"#,
            )),
            GLM_FLEET,
        );
        let binding = workflow
            .bind_member(None, Some("builder"), full_session())
            .expect("bind");

        let err = workflow
            .route_admitted_task(&binding, "anything")
            .await
            .expect_err("duplicate key");
        assert!(err.contains("more than once"), "{err}");
    }

    #[test]
    fn a_missing_router_fails_before_any_worker_is_dispatched() {
        let router_less = GLM_FLEET.replace("reasoning_router = \"luna-low\"\n", "");
        let document = FleetDocument::parse(&router_less).expect("parse");
        let workflow = ExactFleetWorkflow::for_tests(&document, id(), None);

        let err = workflow
            .reject_unusable_auto_members()
            .expect_err("auto without a router must not start");
        assert!(err.contains("implementer"), "{err}");
        assert!(err.contains("reasoning router"), "{err}");
        assert!(
            err.contains("never fall back"),
            "the error must rule out legacy fallback: {err}"
        );
    }

    #[test]
    fn a_fleet_with_no_auto_member_starts_without_a_router() {
        let text = r#"
name = "pinned"
schema = "exact"

[[members]]
id = "auditor"
provider = "zai"
model = "glm-5"
reasoning = "high"
permissions = "read_only"
"#;
        let document = FleetDocument::parse(text).expect("parse");
        let workflow = ExactFleetWorkflow::for_tests(
            &document,
            QualifiedFleetId {
                name: "pinned".to_string(),
                origin: "workspace".to_string(),
            },
            None,
        );
        workflow
            .reject_unusable_auto_members()
            .expect("no auto member means no router requirement");
        assert_eq!(workflow.snapshot().members().len(), 1);
    }

    /// A Router whose credentials are locally absent fails the Workflow before
    /// any worker is dispatched — decided from local config, never from a live
    /// probe of the provider.
    #[test]
    fn a_locally_unusable_router_fails_before_any_worker_is_dispatched() {
        let document = FleetDocument::parse(GLM_FLEET).expect("parse");
        let err = ExactFleetWorkflow::for_tests_with_unavailable_router(
            &document,
            id(),
            "no credential configured for `openai`",
        )
        .expect_err("an unusable router must not start an auto fleet");

        assert!(err.contains("cannot start"), "{err}");
        assert!(err.contains("implementer"), "{err}");
        assert!(err.contains("no credential configured"), "{err}");
        assert!(err.contains("never fall back"), "{err}");
    }

    #[test]
    fn the_projected_roster_pins_each_members_exact_provider_and_model() {
        let workflow = workflow_with(None, GLM_FLEET);
        let member = workflow.roster().get("implementer").expect("roster member");

        assert_eq!(member.profile.provider.as_deref(), Some("zai"));
        assert_eq!(member.profile.model.as_deref(), Some("glm-5"));
        assert_eq!(
            member.profile.reasoning_effort, None,
            "reasoning is decided per task, not baked into the projected profile"
        );
    }

    /// A saved ceiling must not be widened by an unusual fleet role name — and
    /// a role the ceiling *does* permit must survive rather than be flattened.
    #[test]
    fn a_read_only_member_projects_a_read_oriented_posture() {
        let workflow = workflow_with(None, GLM_FLEET);
        let auditor = workflow.roster().get("auditor").expect("auditor");

        // `reviewer`'s built-in posture now needs a full shell (read-only inspection:
        // bounded verification surface + network), which the `read_only`
        // ceiling this member saved refuses — so it is flattened into
        // `scout`, exactly as a `verifier` member under the same ceiling is.
        assert_eq!(auditor.profile.role.name, "scout");
        assert!(
            auditor.profile.permissions.allow_shell,
            "the projected Scout needs Bash registration for its typed read-only subset"
        );
        assert_eq!(
            write_authority_for(workflow.member("auditor").expect("member").permissions),
            "read_only"
        );

        let implementer = workflow.roster().get("implementer").expect("implementer");
        assert_eq!(implementer.profile.role.name, "builder");
    }

    /// The posture role may only ever pick a role the ceiling already permits.
    /// An unknown role, and a real role whose posture is wider than the saved
    /// ceiling, both fall back to the ceiling-derived posture.
    #[test]
    fn a_members_posture_role_can_never_be_wider_than_its_ceiling() {
        let read_only = PermissionCeiling::preset("read_only").expect("preset");
        let read_write = PermissionCeiling::preset("read_write").expect("preset");

        // Roles that fit are preserved, including the renamed public role.
        // `reviewer` needs a full shell for its read-only inspection posture, which a
        // read-only ceiling refuses, so it flattens to `scout` like
        // `verifier` does below.
        assert_eq!(posture_role_for_member("reviewer", read_only), "scout");
        assert_eq!(posture_role_for_member("reviewer", read_write), "reviewer");
        assert_eq!(posture_role_for_member("planner", read_only), "planner");
        let analyst = PermissionCeiling::preset("analyst").expect("preset");
        assert_eq!(
            posture_role_for_member("planner", analyst),
            "planner",
            "a named planner under a no-shell ceiling stays planner; shell intersects"
        );
        assert_eq!(
            posture_role_for_member("consultant", read_only),
            "consultant"
        );
        // …and the compatibility aliases resolve to the same canonical role.
        assert_eq!(posture_role_for_member("oracle", read_only), "consultant");

        // A verifier needs a full shell; a read-only ceiling refuses it.
        assert_eq!(posture_role_for_member("verifier", read_only), "scout");
        assert_eq!(
            posture_role_for_member("verifier", read_write),
            "verifier",
            "a full-shell ceiling does permit it"
        );

        // A builder needs write authority.
        assert_eq!(posture_role_for_member("builder", read_only), "scout");
        assert_eq!(posture_role_for_member("worker", read_only), "scout");
        assert_eq!(posture_role_for_member("builder", read_write), "builder");

        // An arbitrary domain role must not fall through to General.
        assert_eq!(posture_role_for_member("auditor", read_only), "scout");
        assert_eq!(posture_role_for_member("auditor", read_write), "builder");

        // `custom` is defined by an explicit allowlist, never by a role name.
        assert_eq!(posture_role_for_member("custom", read_write), "builder");

        // `tools = false` keeps the narrowest posture whatever the role says.
        assert_eq!(
            posture_role_for_member("builder", PermissionCeiling::ROUTER),
            "scout"
        );
    }

    /// A preserved role must still resolve to the runtime agent type it names,
    /// or the roster projection would have swapped one surface for another.
    #[test]
    fn a_preserved_posture_role_resolves_to_its_runtime_agent_type() {
        use crate::tools::subagent::FleetRole;

        let workflow = workflow_with(None, GLM_FLEET);
        for (id, expected) in [
            // auditor saved `permissions = "read_only"`, which refuses
            // reviewer's read-only inspection shell posture, so it projects scout.
            ("auditor", FleetRole::Scout),
            ("implementer", FleetRole::Builder),
        ] {
            let member = workflow.roster().get(id).expect("roster entry");
            assert_eq!(
                crate::fleet::worker_runtime::roster_member_agent_type(member),
                expected,
                "{id} must resolve to the role its projected posture names"
            );
        }
    }

    // ── Permission ceilings, as the child actually experiences them ─────────

    /// `tools = false` means zero model tools — an empty allowlist, which the
    /// child registry treats as "nothing is visible and nothing is callable".
    #[test]
    fn tools_false_yields_an_empty_tool_surface() {
        let authority = ChildAuthority::clamp(PermissionCeiling::ROUTER, full_session());

        assert!(!authority.ceiling.tools);
        assert_eq!(
            authority.allowed_tools.as_deref(),
            Some(&[] as &[String]),
            "tools = false must be an empty allowlist, not an absent one"
        );
        assert_eq!(authority.write_authority, "read_only");
        assert_eq!(authority.max_depth, 0);
    }

    /// `network_tool = false` removes every model-visible network, browser,
    /// and remote-MCP surface except the `Web` family's two read-only actions
    /// — even when `tools = true`. The family *name* must survive the deny
    /// list so the child registry's action seam can grant exactly
    /// `search`/`fetch`; every other browsing spelling is denied.
    #[test]
    fn network_disabled_denies_every_network_surface_even_with_tools_enabled() {
        let member = PermissionCeiling::preset("read_write").expect("preset");
        assert!(member.tools);
        assert!(!member.network_tool);

        let authority = ChildAuthority::clamp(member, full_session());

        assert!(
            authority.allowed_tools.is_none(),
            "a tool-using member keeps full inheritance, narrowed by the deny list"
        );
        for expected in [
            "web.run",
            "web_run",
            "web_search",
            "fetch_url",
            "wait_for_dev_server",
            "github",
            "mcp*",
        ] {
            assert!(
                authority
                    .disallowed_tools
                    .iter()
                    .any(|name| name == expected),
                "{expected} must be denied: {:?}",
                authority.disallowed_tools
            );
        }
        // The canonical family name is what the read-only web surface
        // dispatches under; only its reaching spellings are denied.
        assert!(
            !authority.disallowed_tools.iter().any(|name| name == "Web"),
            "the Web family name must survive so search/fetch stay reachable: {:?}",
            authority.disallowed_tools
        );

        // A member that IS allowed a network tool gets no such deny list.
        let networked = ChildAuthority::clamp(
            PermissionCeiling::preset("full").expect("preset"),
            full_session(),
        );
        assert!(networked.ceiling.network_tool);
        assert!(networked.disallowed_tools.is_empty());
    }

    /// The browsing capability is registered under several names, and `web.run`
    /// is the one a deny list stopping at the `Web` family name leaves behind.
    /// A network-denied member that can still call `web.run` is not
    /// network-denied, so every spelling *except* the family name itself —
    /// which the action seam bounds to `search`/`fetch` — stays on the list.
    #[test]
    fn network_disabled_denies_the_canonical_web_run_surface_and_its_aliases() {
        let authority = ChildAuthority::clamp(
            PermissionCeiling::preset("read_write").expect("preset"),
            full_session(),
        );

        let denied = |name: &str| {
            let lowered = name.to_ascii_lowercase();
            authority.disallowed_tools.iter().any(|rule| {
                let rule = rule.to_ascii_lowercase();
                rule.strip_suffix('*')
                    .map_or(rule == lowered, |prefix| lowered.starts_with(prefix))
            })
        };

        for name in [
            "web.run",
            "web_run",
            "web_search",
            "web.fetch",
            "web_fetch",
            "fetch_url",
            "wait_for_dev_server",
            "browse",
            "browser",
        ] {
            assert!(
                denied(name),
                "{name} must be denied: {:?}",
                authority.disallowed_tools
            );
        }
        // The family name itself is what the read-only search/fetch surface
        // dispatches under; the action seam and the URL-input guard bound it.
        assert!(
            !denied("Web"),
            "the Web family name must survive a network denial: {:?}",
            authority.disallowed_tools
        );
        // The globs must not reach past the browsing family.
        for name in ["read_file", "run_tests", "Git", "grep_files"] {
            assert!(!denied(name), "{name} is not a network surface");
        }
    }

    /// `rlm` reaches the network without ever naming a network tool: `open`
    /// fetches a `url` by calling `FetchUrlTool` in-process, and `eval` runs
    /// Python that owns a socket API. Denying `fetch_url` sees neither call, so
    /// both actions carry their own deny-list entries.
    #[test]
    fn network_disabled_denies_the_in_process_rlm_reach() {
        let authority = ChildAuthority::clamp(
            PermissionCeiling::preset("read_write").expect("preset"),
            full_session(),
        );

        let denied = |name: &str| {
            let lowered = name.to_ascii_lowercase();
            authority.disallowed_tools.iter().any(|rule| {
                let rule = rule.to_ascii_lowercase();
                rule.strip_suffix('*')
                    .map_or(rule == lowered, |prefix| lowered.starts_with(prefix))
            })
        };

        for reaching in ["rlm_open", "rlm_eval"] {
            assert!(
                denied(reaching),
                "{reaching} reaches the network in-process and must be denied: {:?}",
                authority.disallowed_tools
            );
        }
        // The fail-closed narrowing is deliberate but *bounded*: the bounded
        // local metadata actions survive, and so does the family itself, so the
        // per-action seam has something left to permit.
        for kept in ["rlm", "rlm_session_objects", "rlm_configure", "rlm_close"] {
            assert!(
                !denied(kept),
                "{kept} is bounded local metadata and must survive a network denial"
            );
        }
    }

    /// The deny-list sentinel has to actually be on the deny list, or every
    /// posture check derived from it silently reads "network allowed".
    #[test]
    fn the_network_denial_sentinel_is_installed_by_a_network_denial() {
        assert!(
            NETWORK_TOOL_DENYLIST.contains(&NETWORK_DENIAL_SENTINEL),
            "{NETWORK_DENIAL_SENTINEL} must be an explicit entry, not a glob match"
        );
        let authority = ChildAuthority::clamp(
            PermissionCeiling::preset("read_write").expect("preset"),
            full_session(),
        );
        assert!(
            authority
                .disallowed_tools
                .iter()
                .any(|rule| rule == NETWORK_DENIAL_SENTINEL),
            "a network denial must install the sentinel verbatim: {:?}",
            authority.disallowed_tools
        );
        // …and a network-*capable* member must not, or the sentinel would read
        // as denied for everyone.
        let networked = ChildAuthority::clamp(
            PermissionCeiling::preset("full").expect("preset"),
            full_session(),
        );
        assert!(
            !networked
                .disallowed_tools
                .iter()
                .any(|rule| rule == NETWORK_DENIAL_SENTINEL)
        );
    }

    /// Every network-denied preset — read_only/read-only inspection included — leaves the
    /// `Web` family name reachable and seals each of its reaching spellings.
    /// This is the deny-list half of the read-only web-search contract; the
    /// registry-side half (exactly `search`/`fetch`, with URL-addressed calls
    /// refused) is asserted in `subagent/tests.rs`.
    #[test]
    fn every_network_denial_leaves_web_search_reachable_by_family_name() {
        for preset in ["analyst", "read_only", "verifier", "read_write"] {
            let authority = ChildAuthority::clamp(
                PermissionCeiling::preset(preset).expect("preset"),
                full_session(),
            );
            assert!(
                !authority.ceiling.network_tool,
                "{preset} is network-denied"
            );
            assert!(
                !authority.disallowed_tools.iter().any(|rule| rule == "Web"),
                "{preset} must keep the Web family name: {:?}",
                authority.disallowed_tools
            );
            for sealed in [
                "web_*",
                "web.*",
                "web.run",
                "web_run",
                "web_search",
                "web.fetch",
                "web_fetch",
                "fetch_url",
                "wait_for_dev_server",
                "github",
                "mcp*",
            ] {
                assert!(
                    authority.disallowed_tools.iter().any(|rule| rule == sealed),
                    "{preset} must deny {sealed}: {:?}",
                    authority.disallowed_tools
                );
            }
        }
    }

    /// A member saved as `write = false` must not receive a mutating surface —
    /// including the raw shell a `verifier`-shaped ceiling keeps for running
    /// checks. `rm -rf` mutates a workspace exactly as well as `write_file`,
    /// and a receipt that says `write=false` while the child holds `exec_shell`
    /// is not true.
    #[test]
    fn a_read_only_member_gets_a_truthful_non_mutating_tool_contract() {
        let verifier = PermissionCeiling::preset("verifier").expect("preset");
        assert!(!verifier.write);
        assert_eq!(verifier.shell, ShellCeiling::Full);

        let authority = ChildAuthority::clamp(verifier, full_session());
        assert_eq!(authority.write_authority, "read_only");

        let denied = |name: &str| {
            authority.disallowed_tools.iter().any(|rule| {
                rule == name || rule.strip_suffix('*').is_some_and(|p| name.starts_with(p))
            })
        };

        // `rlm_eval` belongs on this list for the same reason `exec_shell` does:
        // the Python it runs writes files. A tool is a mutation primitive
        // because of what it can do, not because of what it is called.
        for mutating in [
            "write_file",
            "edit_file",
            "apply_patch",
            "fim_edit",
            "rlm_eval",
        ] {
            assert!(
                denied(mutating),
                "{mutating} must be denied for a read-only member: {:?}",
                authority.disallowed_tools
            );
        }
        for raw_shell in [
            "Bash",
            "exec_shell",
            "exec_shell_interact",
            "task_shell_start",
            "terminal/run",
        ] {
            assert!(
                denied(raw_shell),
                "{raw_shell} is a general mutation primitive: {:?}",
                authority.disallowed_tools
            );
        }
        // What the member is *for* survives: the bounded verification surface.
        // (`rlm_open` is absent from this list only because the `verifier`
        // preset is also network-denied; the write contract alone keeps it —
        // see `a_write_denial_alone_keeps_local_rlm_loading`.)
        for kept in [
            "Run",
            "run_tests",
            "run_verifiers",
            "read_file",
            "grep_files",
            "rlm",
        ] {
            assert!(!denied(kept), "{kept} must stay available to a verifier");
        }

        // A write-capable member is untouched by this contract.
        let builder = ChildAuthority::clamp(
            PermissionCeiling::preset("read_write").expect("preset"),
            full_session(),
        );
        assert!(builder.ceiling.write);
        for kept in ["write_file", "apply_patch", "exec_shell"] {
            assert!(
                !builder.disallowed_tools.iter().any(|rule| rule == kept),
                "{kept} must stay available to a write-capable member"
            );
        }
    }

    /// The two denials are separate contracts and must not bleed into each
    /// other. A member that may not *write* can still load a large local file
    /// into an RLM kernel and read it — that is analysis, not mutation. Only
    /// `eval` goes, because only `eval` runs code.
    #[test]
    fn a_write_denial_alone_keeps_local_rlm_loading() {
        let member = PermissionCeiling {
            write: false,
            network_tool: true,
            shell: ShellCeiling::ReadOnly,
            delegation_depth: 0,
            tools: true,
        };
        let authority = ChildAuthority::clamp(member, full_session());
        assert!(!authority.ceiling.write);
        assert!(authority.ceiling.network_tool);

        let denied = |name: &str| authority.disallowed_tools.iter().any(|rule| rule == name);

        assert!(denied("rlm_eval"), "eval runs code, so it mutates");
        for kept in ["rlm", "rlm_open", "rlm_session_objects", "rlm_close"] {
            assert!(
                !denied(kept),
                "{kept} loads and inspects; it does not mutate: {:?}",
                authority.disallowed_tools
            );
        }
    }

    /// The parent posture always wins. A saved `full` member inside a
    /// read-only, no-network, no-shell session runs at the session's ceiling.
    #[test]
    fn the_parent_ceiling_wins_over_a_wider_saved_member() {
        let session = PermissionCeiling {
            write: false,
            network_tool: false,
            shell: ShellCeiling::ReadOnly,
            delegation_depth: 0,
            tools: true,
        };
        let member = PermissionCeiling::preset("full").expect("preset");
        assert!(member.write && member.network_tool);

        let authority = ChildAuthority::clamp(member, session);

        assert!(!authority.ceiling.write, "a fleet may not grant write");
        assert!(
            !authority.ceiling.network_tool,
            "a fleet may not grant a network tool"
        );
        assert_eq!(authority.ceiling.shell, ShellCeiling::ReadOnly);
        assert_eq!(authority.ceiling.delegation_depth, 0);
        assert_eq!(authority.write_authority, "read_only");
        assert_eq!(authority.max_depth, 0);
        assert_eq!(authority.posture_role, "scout");
        assert!(!authority.disallowed_tools.is_empty());
    }

    /// A read-only session cannot be widened by a session that *is* permissive
    /// either — clamping is symmetric, and takes the narrower side each way.
    #[test]
    fn clamping_takes_the_narrower_side_of_every_field() {
        let narrow_member = PermissionCeiling {
            write: false,
            network_tool: false,
            shell: ShellCeiling::None,
            delegation_depth: 0,
            tools: true,
        };
        let authority = ChildAuthority::clamp(narrow_member, full_session());

        assert!(!authority.ceiling.write);
        assert_eq!(authority.ceiling.shell, ShellCeiling::None);
        assert_eq!(authority.ceiling.delegation_depth, 0);
    }

    // ── Preflight ──────────────────────────────────────────────────────────

    /// Z.AI GLM routes express only thinking enabled/disabled, so `high` and
    /// `max` must not be reported as two distinct provider-effective tiers.
    #[test]
    fn glm_routes_report_an_enabled_disabled_provider_control() {
        let capability = reasoning_capability_for_route(
            ApiProvider::Zai,
            crate::config::DEFAULT_ZAI_BASE_URL,
            crate::config::ZAI_GLM_5_2_MODEL,
        );

        assert_eq!(
            capability.control,
            ProviderReasoningControl::EnabledDisabled,
            "Z.AI's request shaping emits only thinking enabled/disabled"
        );
        assert!(!capability.supports_native_adaptive());
        assert_eq!(
            capability.provider_effective(ReasoningTier::High),
            ProviderEffectiveReasoning::Enabled
        );
        assert_eq!(
            capability.provider_effective(ReasoningTier::Off),
            ProviderEffectiveReasoning::Disabled
        );
    }

    /// DeepSeek varies `reasoning_effort` per tier, so its tiers are real.
    #[test]
    fn a_route_that_varies_its_wire_value_reports_distinct_tiers() {
        let capability = reasoning_capability_for_route(
            ApiProvider::Deepseek,
            crate::config::DEFAULT_DEEPSEEK_BASE_URL,
            "deepseek-v4-pro",
        );
        assert_eq!(capability.control, ProviderReasoningControl::Tiers);
    }

    /// First-party DeepSeek routes document `reasoning_effort` low/high/max
    /// on the wire (no medium), so `low` is a real tier there. The capability
    /// must report the tier the route *sends*, not the tier the selector
    /// named: low reaches the wire as low, medium rounds up to high because
    /// the dialect has no such value (#52).
    #[test]
    fn a_deepseek_route_reports_low_as_low_and_medium_as_high() {
        let capability = reasoning_capability_for_route(
            ApiProvider::Deepseek,
            crate::config::DEFAULT_DEEPSEEK_BASE_URL,
            "deepseek-v4-pro",
        );

        // Exactly what the request shaping does, read back off the capability.
        for (requested, expected) in [
            (ReasoningTier::Low, ReasoningTier::Low),
            (ReasoningTier::Medium, ReasoningTier::High),
            (ReasoningTier::High, ReasoningTier::High),
            (ReasoningTier::Max, ReasoningTier::Max),
            (ReasoningTier::Off, ReasoningTier::Off),
        ] {
            assert_eq!(
                capability.wire_tier(requested),
                expected,
                "requested {requested:?} must be reported as what the wire carries"
            );
            let (effective, normalized) = capability.normalize(requested);
            assert_eq!(effective, expected);
            assert_eq!(normalized, requested != expected);
        }

        // And the resolver carries that all the way onto the receipt.
        let resolved = codewhale_workflow::resolve_exact_member_reasoning(
            "implementer",
            &codewhale_workflow::FrozenRoute {
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
            },
            RequestedReasoning::Low,
            &capability,
            &RouterAvailability::Absent,
            None,
            None,
        )
        .expect("resolve");
        assert_eq!(resolved.requested(), RequestedReasoning::Low);
        assert_eq!(
            resolved.effective(),
            codewhale_workflow::EffectiveReasoning::Tier(ReasoningTier::Low)
        );
        assert!(!resolved.capability_normalized());
    }

    /// Routes whose dialect has no low tier still collapse low onto high, and
    /// the capability must say so instead of reporting a `low` the wire never
    /// carried. CodeWhale's normalizer keeps the historic low/medium → high
    /// coercion for these DeepSeek-compatible hosted routes because their own
    /// wire contracts are not verified.
    #[test]
    fn a_route_that_collapses_low_onto_high_says_so_instead_of_reporting_low() {
        let capability = reasoning_capability_for_route(
            ApiProvider::Siliconflow,
            crate::config::DEFAULT_SILICONFLOW_BASE_URL,
            "deepseek-ai/DeepSeek-V4-Pro",
        );

        for (requested, expected) in [
            (ReasoningTier::Low, ReasoningTier::High),
            (ReasoningTier::Medium, ReasoningTier::High),
            (ReasoningTier::High, ReasoningTier::High),
            (ReasoningTier::Max, ReasoningTier::Max),
            (ReasoningTier::Off, ReasoningTier::Off),
        ] {
            assert_eq!(
                capability.wire_tier(requested),
                expected,
                "requested {requested:?} must be reported as what the wire carries"
            );
            let (effective, normalized) = capability.normalize(requested);
            assert_eq!(effective, expected);
            assert_eq!(normalized, requested != expected);
        }
    }

    /// Preflight resolves the provider, canonicalizes the model, identifies the
    /// endpoint, and decides credential readiness — all from local config.
    #[test]
    fn preflight_freezes_provider_model_endpoint_and_local_readiness() {
        let _env_lock = crate::test_support::lock_test_env();
        let _key = crate::test_support::EnvVarGuard::set("ZAI_API_KEY", "zai-key");
        let config = Config {
            provider: Some("zai".to_string()),
            ..Default::default()
        };

        let route = preflight_route(
            "implementer",
            "zai",
            crate::config::ZAI_GLM_5_2_MODEL,
            &config,
        )
        .expect("preflight");

        assert_eq!(route.member_id, "implementer");
        assert_eq!(route.provider_kind, "zai");
        assert_eq!(route.wire_model, crate::config::ZAI_GLM_5_2_MODEL);
        assert!(!route.endpoint.host.is_empty());
        assert!(!route.endpoint.host.contains('/'));
        assert_eq!(route.credential, CredentialReadiness::Configured);
        route.require_ready().expect("ready");

        // The receipt and the child spawn read the same canonical wire model.
        assert_eq!(route.frozen().model, route.wire_model);
    }

    /// A keyless local provider is valid, and is decided without a probe.
    #[test]
    fn a_keyless_local_provider_preflights_as_ready() {
        let _env_lock = crate::test_support::lock_test_env();
        let config = Config {
            provider: Some("ollama".to_string()),
            ..Default::default()
        };

        let Ok(route) = preflight_route("worker", "ollama", "qwen3", &config) else {
            // A model id this build does not know is a different failure than
            // the one under test; skip rather than assert on the catalog.
            return;
        };
        assert_eq!(route.credential, CredentialReadiness::KeylessLocal);
        assert!(route.credential.is_ready());
        route.require_ready().expect("keyless local is valid");
        assert!(route.endpoint.local, "a local runtime is marked local");
    }

    #[test]
    fn ollama_cloud_and_custom_remote_preflight_require_route_scoped_credentials() {
        let _env_lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated credential home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _ollama_cloud_key = crate::test_support::EnvVarGuard::remove("OLLAMA_CLOUD_API_KEY");
        let _ollama_key = crate::test_support::EnvVarGuard::remove("OLLAMA_API_KEY");
        let _cli_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");
        let _cli_key = crate::test_support::EnvVarGuard::remove("CODEWHALE_CLI_API_KEY");
        codewhale_secrets::Secrets::auto_detect()
            .set("ollama", "legacy-cloud-key")
            .expect("seed released Ollama Cloud slot");

        let cloud = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                ollama: crate::config::ProviderConfig {
                    base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let cloud_route = preflight_route(
            "cloud-worker",
            "ollama",
            crate::config::DEFAULT_OLLAMA_MODEL,
            &cloud,
        )
        .expect("official Cloud route");
        assert_eq!(cloud_route.provider_id, "ollama-cloud");
        assert_eq!(cloud_route.provider_config_id.as_deref(), Some("ollama"));
        assert_eq!(cloud_route.provider_kind, "ollama-cloud");
        assert_eq!(cloud_route.credential, CredentialReadiness::Configured);
        assert!(!cloud_route.endpoint.local);
        cloud_route.require_ready().expect("Cloud env key is ready");

        let custom_remote = Config {
            provider: Some("ollama".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                ollama: crate::config::ProviderConfig {
                    base_url: Some("https://ollama-gateway.example.test/v1".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let custom_route = preflight_route(
            "custom-worker",
            "ollama",
            crate::config::DEFAULT_OLLAMA_MODEL,
            &custom_remote,
        )
        .expect("custom route still resolves structurally");
        assert!(matches!(
            custom_route.credential,
            CredentialReadiness::Missing { .. }
        ));
        assert!(!custom_route.endpoint.local);
        assert!(custom_route.require_ready().is_err());
    }

    #[tokio::test]
    async fn legacy_ollama_cloud_fleet_start_builds_clients_from_the_frozen_source_route() {
        let _env_lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("isolated credential home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path());
        let _backend = crate::test_support::EnvVarGuard::set("CODEWHALE_SECRET_BACKEND", "file");
        let _cloud_env = crate::test_support::EnvVarGuard::remove("OLLAMA_CLOUD_API_KEY");
        let _official_env = crate::test_support::EnvVarGuard::remove("OLLAMA_API_KEY");
        codewhale_secrets::Secrets::auto_detect()
            .set("ollama", "legacy-cloud-fleet-key")
            .expect("seed released Ollama Cloud slot");

        let config = Config {
            provider: Some("deepseek".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                ollama: crate::config::ProviderConfig {
                    base_url: Some(codewhale_config::provider::OLLAMA_CLOUD_BASE_URL.to_string()),
                    model: Some(crate::config::DEFAULT_OLLAMA_CLOUD_MODEL.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let document = FleetDocument::parse(&format!(
            r#"
name = "glm-pair"
schema = "exact"

[[members]]
id = "cloud-worker"
role = "builder"
provider = "ollama"
model = "{}"
reasoning = "medium"
permissions = "read_only"
"#,
            crate::config::DEFAULT_OLLAMA_CLOUD_MODEL
        ))
        .expect("legacy Cloud fleet parses");

        // `capture` is the real Workflow-start path: it preflights readiness,
        // constructs every worker client, and freezes the run-scoped roster.
        let workflow = ExactFleetWorkflow::capture(
            &document,
            id(),
            "2026-08-14T00:00:00Z",
            Some(&config),
            &[],
        )
        .expect("legacy Cloud fleet starts");
        let route = workflow
            .preflight
            .worker("cloud-worker")
            .expect("preflighted worker");
        assert_eq!(route.provider_id, "ollama-cloud");
        assert_eq!(route.provider_config_id.as_deref(), Some("ollama"));
        assert_eq!(
            workflow
                .roster()
                .get("cloud-worker")
                .and_then(|profile| profile.profile.provider.as_deref()),
            Some("ollama"),
            "the child pin must rebuild the legacy table/slot even though receipts are canonical"
        );

        let binding = workflow
            .bind_member(Some("cloud-worker"), None, full_session())
            .expect("worker binds");
        let launch = workflow
            .route_admitted_task(&binding, "verify the frozen Cloud route")
            .await
            .expect("manual-tier launch needs no provider call");
        assert_eq!(launch.provider, "ollama-cloud");
        assert_eq!(launch.receipt.provider, "ollama-cloud");

        let router_profile = ReasoningRouterProfile::parse(&format!(
            r#"
name = "legacy-cloud-router"
schema = "reasoning_router"
provider = "ollama"
model = "{}"
call_reasoning = "low"
"#,
            crate::config::DEFAULT_OLLAMA_CLOUD_MODEL
        ))
        .expect("legacy Cloud router profile parses");
        let captured =
            CapturedReasoningRouter::from_profile(&router_profile, "workspace".to_string());
        let live = LiveFleetRouter::bind(&captured, &config)
            .expect("legacy Cloud Router binds its source table and secret");
        assert_eq!(live.route.provider_id, "ollama-cloud");
        assert_eq!(live.route.provider_config_id.as_deref(), Some("ollama"));
        assert_eq!(live.client.api_provider(), ApiProvider::OllamaCloud);
        assert_eq!(
            live.client.base_url(),
            codewhale_config::provider::OLLAMA_CLOUD_BASE_URL
        );
    }

    /// A tier label is a selector concept; what a request may carry is a
    /// provider concept. The value placed on a call must come from the route
    /// normalizer the client actually uses, or a Codex-routed Router is called
    /// at the provider default while its receipt claims a tier.
    #[test]
    fn a_call_reasoning_value_is_shaped_by_the_configured_route_not_a_tier_label() {
        // A tiered non-Codex route spells the tiers the ordinary way, after
        // the same route normalization the client performs (first-party
        // DeepSeek keeps a real `low`; medium still rounds up to high).
        for (tier, expected) in [
            (ReasoningTier::Off, "off"),
            (ReasoningTier::High, "high"),
            (ReasoningTier::Max, "max"),
        ] {
            assert_eq!(
                route_reasoning_setting(
                    ApiProvider::Deepseek,
                    crate::config::DEFAULT_DEEPSEEK_BASE_URL,
                    "deepseek-v4-pro",
                    tier,
                ),
                expected,
                "{tier:?} on a deepseek route"
            );
        }

        // Codex is the case a bare tier label gets wrong in both directions:
        // it has no `off`, and its top tier is spelled `xhigh`.
        let codex = |tier| {
            route_reasoning_setting(
                ApiProvider::OpenaiCodex,
                "https://chatgpt.com/backend-api/codex",
                "gpt-5.6-codex",
                tier,
            )
        };
        assert_eq!(codex(ReasoningTier::Max), "xhigh");
        assert_eq!(codex(ReasoningTier::Low), "low");
        assert_ne!(
            codex(ReasoningTier::Off),
            "off",
            "an always-thinking route cannot be asked for `off`; sending the label \
             would take the provider default while the receipt claimed a tier"
        );
    }

    /// An unresolvable provider fails preflight rather than reaching a launch.
    #[test]
    fn an_unresolvable_provider_fails_preflight() {
        let config = Config::default();
        let err = preflight_route("implementer", "not-a-provider", "whatever", &config)
            .expect_err("unresolvable provider");
        assert!(matches!(err, PreflightError::ProviderUnresolved { .. }));
    }

    // ── Receipts ───────────────────────────────────────────────────────────

    /// The receipt is the durable artifact. It must carry every side of the
    /// decision — including which service chose the tier and what that call was
    /// configured to cost — and must store no task text, path, or key.
    #[tokio::test]
    async fn a_launch_receipt_names_the_service_route_and_call_cost_without_content() {
        let workflow = workflow_with(
            Some(StaticFleetRouter::new(r#"{"reasoning":"max"}"#)),
            GLM_FLEET,
        );
        let binding = workflow
            .bind_member(None, Some("builder"), full_session())
            .expect("bind");

        let launch = workflow
            .route_admitted_task(&binding, "refactor /Users/hunter/app with ZAI_API_KEY=zzz")
            .await
            .expect("launch");
        let receipt = &launch.receipt;

        assert_eq!(receipt.fleet, "workspace/glm-pair");
        assert_eq!(receipt.schema_kind, "exact");
        assert_eq!(receipt.member_id, "implementer");
        assert_eq!(receipt.member_role, "builder");
        assert_eq!(receipt.provider, "zai");
        assert_eq!(receipt.model, "glm-5");
        assert_eq!(receipt.requested_reasoning, "auto");
        assert_eq!(receipt.effective_reasoning, "max");
        assert_eq!(receipt.selection_source, "fleet_router");
        assert!(!receipt.content_hash.is_empty());

        // The service is labelled as a service, with its exact route and the
        // configured requested → provider-effective call reasoning.
        let router = receipt.router.as_ref().expect("router identity");
        assert_eq!(router.service_kind, "reasoning_router");
        assert_eq!(router.qualified(), "workspace/luna-low");
        assert_eq!(router.provider, "openai");
        assert_eq!(router.model, "gpt-5.6-luna");
        let call = router.call.as_ref().expect("call disclosure");
        assert_eq!(call.requested, "low");
        assert_eq!(call.effective, "low");
        assert_eq!(call.provider_effective, "low");

        // Cross-provider inference happened (zai worker, openai router) and is
        // disclosed rather than implied away.
        assert!(receipt.cross_provider_inference);
        assert!(
            receipt.transport.contains("different provider"),
            "{}",
            receipt.transport
        );

        // Disclosure without content.
        let disclosure = receipt.routing_summary.as_ref().expect("disclosure");
        assert!(disclosure.transmitted_bytes > 0);
        assert!(disclosure.content_hash.starts_with("sha256:"));
        assert!(disclosure.redacted);

        let json = serde_json::to_string(receipt).expect("serialize");
        for forbidden in ["/Users/", "/home/", ".toml", "api_key", "zzz", "refactor"] {
            assert!(!json.contains(forbidden), "{forbidden} in {json}");
        }

        // The visible line names every side and echoes no content.
        let line = receipt.line();
        for expected in [
            "requested=auto",
            "effective=max",
            "source=fleet_router",
            "reasoning_router:workspace/luna-low",
            "router_call_requested=low",
        ] {
            assert!(line.contains(expected), "{expected} missing from {line}");
        }
        assert!(!line.contains("refactor"), "{line}");
    }

    /// A member's semantic role and its runtime permission posture are separate
    /// facts and the receipt keeps both. An operator who named a member
    /// `auditor` must see `auditor` on the receipt, while the surface actually
    /// granted (`scout`) is disclosed rather than substituted for the name.
    #[tokio::test]
    async fn a_receipt_records_the_posture_without_renaming_the_members_role() {
        const AUDIT_FLEET: &str = r#"
name = "glm-pair"
schema = "exact"

[[members]]
id = "auditor"
role = "auditor"
provider = "zai"
model = "glm-5"
reasoning = "high"
permissions = "read_only"
"#;
        let workflow = workflow_with(None, AUDIT_FLEET);
        let binding = workflow
            .bind_member(None, Some("auditor"), full_session())
            .expect("bind");

        // Enforcement uses the posture; it is not the operator's role name.
        assert_eq!(binding.member_role, "auditor");
        assert_eq!(binding.authority.posture_role, "scout");

        let launch = workflow
            .route_admitted_task(&binding, "review the queue")
            .await
            .expect("launch");
        let receipt = &launch.receipt;

        assert_eq!(receipt.member_role, "auditor");
        assert_eq!(receipt.posture_role.as_deref(), Some("scout"));
        let line = receipt.line();
        assert!(line.contains("(role auditor)"), "{line}");
        assert!(line.contains("posture=scout"), "{line}");
    }
}
