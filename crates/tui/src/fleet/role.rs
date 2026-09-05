//! Fleet roles — the lightweight role surface for exec and sub-agent spawning.
//!
//! Learned from the OMP sub-agent role system: an agent type is a **name**
//! plus a small static posture (tool allowlist, model, reasoning level), and
//! exec consumes exactly that — never the roster, ledger, or worker
//! machinery. The durable Fleet runs (manager / executor / worker_runtime /
//! exact workflow driver) remain the one consumer of the heavy machinery and
//! resolve roles through this same surface, so both paths share one posture.
//!
//! What lives here (and only here) for spawn-time decisions:
//! - [`FleetRole`]: the closed 8-role set, parsing, and canonical labels.
//! - Per-role posture: [`role_requires_read_only_shell`],
//!   [`effective_runtime_profile_for_role`], [`fleet_effective_permissions`].
//! - The tool deny lists + [`is_posture_denial`] + [`ChildAuthority`]: how a
//!   role posture becomes the concrete child surface (allowlist, deny list,
//!   write authority, delegation budget, fingerprint).
//!
//! Deliberately dependency-light: protocol + workflow ceiling types,
//! [`crate::worker_profile`], std. Nothing from fleet control / store /
//! ledger / executor / worker_runtime / roster / profile / identity /
//! members, and nothing from tools.

use codewhale_protocol::fleet::FleetEffectivePermissions;
use codewhale_workflow::{PermissionCeiling, ShellCeiling};

use crate::worker_profile::{ShellPolicy, ToolScope, WorkerRuntimeProfile};

/// Canonical model-facing Fleet role values, in schema order. This is the
/// closed `enum` advertised on the Agent tool's `type` property. Legacy
/// aliases are accepted only at replay/deserialization boundaries
/// ([`migrate_legacy_role_token`]) and are never advertised to models.
pub(crate) const FLEET_ROLE_SCHEMA_VALUES: [&str; 8] = [
    "general",
    "explore",
    "planner",
    "reviewer",
    "implement",
    "test",
    "advisor",
    "custom",
];

/// Role aliases accepted by `normalize_role_alias`. Kept in sync with the
/// match arms below so every input that `FleetRole::from_str` accepts also
/// resolves to a canonical role (avoids the dual-validation rejection in #2649).
pub(crate) const VALID_ROLE_ALIASES: &str = "general; explore; planner; reviewer; implement; test; advisor; custom \
     (legacy aliases remain accepted: worker; scout; builder; verifier; consultant; default; general-purpose; general_purpose; exploration; explorer; plan; planning; awaiter; review; code-review; code_review; implementer; implementation; verify; verification; validator; tester; oracle)";

/// Canonical Fleet role for a delegated worker, with specialized behavior
/// and tool access per role.
///
/// **Public vocabulary is Fleet roles** (`general`, `explore`, `planner`,
/// `reviewer`, `implement`, `test`, `advisor`, `custom`) and the variants match that
/// vocabulary one-to-one. Serialization, prompts, receipts, and UI always
/// use [`Self::as_str`]. Legacy wire spellings (`worker`, `scout`, `plan`,
/// `review`, `implementer`, …) are accepted only through
/// [`migrate_legacy_role_token`] at deserialization / parse boundaries.
///
/// This is the closed runtime role set. It is distinct from
/// `codewhale_config::FleetRole`, which is the open config-side role
/// *declaration* (free-form name plus instruction overlay) carried by a
/// Fleet profile. The `FleetRole` type name remains a compatibility identifier.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FleetRole {
    /// General-purpose worker - full tool access for multi-step tasks.
    #[default]
    Worker,
    /// Fast exploration - read-only tools for codebase search.
    Scout,
    /// Planning — grounded strategy. Reads the workspace and the web and
    /// may run classifier-bounded shell probes; never mutates.
    Planner,
    /// Code review - read + analysis tools.
    Reviewer,
    /// Implementation — focused on writing / patching code to satisfy
    /// a specific change. Distinct from `Worker` in that the prompt
    /// posture pushes hard on landing the change cleanly with the
    /// minimum surrounding edit (#404).
    Builder,
    /// Verification — focused on running the test suite or other
    /// validation gates and reporting pass/fail with evidence.
    /// Distinct from `Reviewer` in that Reviewer reads code and grades it;
    /// Verifier *runs* tests and reports the outcome (#404).
    Verifier,
    /// Advisory counsel — a strong-model second opinion the operator can ask
    /// for guidance, judgement calls, and design critique (#4752).
    ///
    /// Read-only and shell-less by construction: a Consultant reasons about the
    /// code (and may read the web to ground that counsel) and says what it
    /// thinks. It is distinct from `Reviewer`, which grades a specific change
    /// against a standard, and from `Planner`, which produces a plan to execute.
    /// A Consultant answers "what should we do here, and what are we not seeing".
    Consultant,
    /// Custom tool access defined at spawn time. Inherits the parent's
    /// write/network/shell ceiling and is narrowed by the explicit tool list
    /// or an explicit write_authority, never by a silent lock-down.
    Custom,
}

impl serde::Serialize for FleetRole {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for FleetRole {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::from_str(&raw)
            .ok_or_else(|| serde::de::Error::unknown_variant(&raw, &FLEET_ROLE_SCHEMA_VALUES))
    }
}

/// Explicit boundary migration for pre-Fleet serialized role tokens.
///
/// Call this only at load / parse edges. Runtime code must use Fleet role
/// names via [`FleetRole::as_str`]. Returns `None` for tokens that are
/// already canonical or unknown — callers should prefer [`FleetRole::from_str`]
/// for full acceptance (canonical + legacy).
#[must_use]
pub fn migrate_legacy_role_token(token: &str) -> Option<&'static str> {
    match token.trim().to_ascii_lowercase().as_str() {
        "worker" | "general-purpose" | "general_purpose" | "default" => Some("general"),
        "scout" | "exploration" | "explorer" => Some("explore"),
        "plan" | "planning" | "awaiter" => Some("planner"),
        "review" | "code-review" | "code_review" => Some("reviewer"),
        "builder" | "implementer" | "implementation" => Some("implement"),
        "verifier" | "verify" | "verification" | "validator" | "tester" => Some("test"),
        "consultant" | "oracle" => Some("advisor"),
        _ => None,
    }
}

impl FleetRole {
    /// Parse a Fleet role from user input or a serialized boundary.
    ///
    /// Accepts Fleet role names and, at this parse boundary only, legacy
    /// aliases (`scout` → explore, `plan` → planner, …).
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        let normalized = s.trim().to_ascii_lowercase();
        // Boundary migration first, then canonical Fleet names.
        let token = migrate_legacy_role_token(&normalized).unwrap_or(normalized.as_str());
        match token {
            "general" => Some(Self::Worker),
            "explore" => Some(Self::Scout),
            "planner" => Some(Self::Planner),
            "reviewer" => Some(Self::Reviewer),
            "implement" => Some(Self::Builder),
            "test" => Some(Self::Verifier),
            "advisor" => Some(Self::Consultant),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// Canonical Fleet role label for runtime, schemas, prompts, receipts, UI.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Worker => "general",
            Self::Scout => "explore",
            Self::Planner => "planner",
            Self::Reviewer => "reviewer",
            Self::Builder => "implement",
            Self::Verifier => "test",
            Self::Consultant => "advisor",
            Self::Custom => "custom",
        }
    }

    /// One-line model-facing description of what this role does. Backs the
    /// role catalog returned by the agent roster action.
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::Worker => "General-purpose worker with full tool access for multi-step tasks.",
            Self::Scout => "Fast read-only exploration for codebase search and analysis.",
            Self::Planner => {
                "Grounded strategy: reads the workspace and the web, runs read-only probes, never mutates."
            }
            Self::Reviewer => "Reads and grades code against a standard.",
            Self::Builder => {
                "Lands focused code changes cleanly with the minimum surrounding edit."
            }
            Self::Verifier => {
                "Runs the test suite and validation gates, reports pass/fail with evidence."
            }
            Self::Consultant => {
                "Read-only high-reasoning counsel for judgement calls and design critique."
            }
            Self::Custom => "Custom tool access defined at spawn time by the parent's posture.",
        }
    }

    /// All canonical roles in schema order, for catalog responses.
    #[must_use]
    pub fn all() -> [Self; 8] {
        [
            Self::Worker,
            Self::Scout,
            Self::Planner,
            Self::Reviewer,
            Self::Builder,
            Self::Verifier,
            Self::Consultant,
            Self::Custom,
        ]
    }
}

/// Public label for any role token (canonical or legacy alias).
/// Canonical/legacy tokens collapse to the advertised name;
/// anything else passes through trimmed.
#[must_use]
pub fn public_role_label(token: &str) -> String {
    FleetRole::from_str(token).map_or_else(
        || token.trim().to_string(),
        |role| role.as_str().to_string(),
    )
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

/// A Runtime role policy intersected with the live parent and translated into
/// the concrete knobs a child spawn actually carries.
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
    /// Intersect one Runtime-requested posture with the live parent posture.
    ///
    /// Every field takes the more restrictive side, so a child can never widen
    /// live authority.
    #[must_use]
    pub(crate) fn clamp(requested: PermissionCeiling, session: PermissionCeiling) -> Self {
        let ceiling = requested.clamp_to(session);

        // `tools = false` is total: an empty allowlist leaves the child with no
        // model-visible tools and nothing it is permitted to call.
        let allowed_tools = (!ceiling.tools).then(Vec::new);

        // The deny list expresses the effective Runtime posture. The spawn
        // registry separately unions it with inherited parent restrictions, so
        // a descendant can never drop something an ancestor
        // imposed.
        let mut disallowed_tools = Vec::new();
        if !ceiling.network_tool {
            disallowed_tools.extend(NETWORK_TOOL_DENYLIST.iter().map(|name| (*name).to_string()));
        }
        // Raw shell requires the ceiling to *say* `shell = "full"`. Any narrower
        // shell posture — `none` or `read_only` — loses the raw command surface
        // outright. `from_runtime_role` may subsequently
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
            // shell is already gone by the rule above; a Runtime scout/reviewer
            // may regain only canonical Bash in `from_runtime_role`, behind its
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

    /// Derive authority exclusively from Runtime policy after Fleet identity
    /// selection. Free-form semantic roles map to Runtime `custom`; neither
    /// the Fleet definition nor its legacy `permissions` key participates.
    #[must_use]
    pub(crate) fn from_runtime_role(role: &str, session: PermissionCeiling) -> Self {
        let runtime_role = runtime_role_for_member(role);
        let requested = runtime_permission_ceiling(&runtime_role);
        let mut authority = Self::clamp(requested, session);
        authority.posture_role = runtime_role.as_str();

        if matches!(
            runtime_role,
            FleetRole::Scout | FleetRole::Reviewer | FleetRole::Planner
        ) && authority.ceiling.shell != ShellCeiling::None
        {
            // Runtime's Scout/Reviewer policy permits classifier-bounded Bash
            // inspection. Keep the canonical entry while all other shell and
            // execution aliases remain denied.
            authority
                .disallowed_tools
                .retain(|name| !name.eq_ignore_ascii_case("Bash"));
        }
        authority
    }
}

/// Map the Fleet's open semantic role label onto Runtime's closed role policy.
/// Unknown labels remain useful identity (`auditor`, `research-lead`, …) but
/// execute under Runtime `custom`, whose capabilities still intersect with the
/// live parent.
pub(crate) fn runtime_role_for_member(role: &str) -> FleetRole {
    FleetRole::from_str(role).unwrap_or(FleetRole::Custom)
}

fn runtime_permission_ceiling(role: &FleetRole) -> PermissionCeiling {
    let profile = WorkerRuntimeProfile::for_role(role.clone());
    let shell = match profile.shell {
        ShellPolicy::None => ShellCeiling::None,
        ShellPolicy::ReadOnly => ShellCeiling::ReadOnly,
        ShellPolicy::Full => ShellCeiling::Full,
    };
    let tools = match profile.tools {
        ToolScope::Inherit => true,
        ToolScope::Explicit(ref tools) => !tools.is_empty(),
    };
    PermissionCeiling {
        write: profile.permissions.write,
        network_tool: profile.permissions.network,
        shell,
        delegation_depth: profile.max_spawn_depth,
        tools,
    }
}

/// Intersect a worker shell policy with the session's legacy shell opt-in.
#[must_use]
pub(crate) fn session_shell_ceiling(shell: ShellPolicy, allow_shell: bool) -> ShellCeiling {
    match shell {
        ShellPolicy::None => ShellCeiling::None,
        ShellPolicy::ReadOnly => ShellCeiling::ReadOnly,
        ShellPolicy::Full if allow_shell => ShellCeiling::Full,
        ShellPolicy::Full => ShellCeiling::None,
    }
}

/// Map a permission ceiling onto the canonical posture role that governs the
/// child's tool surface.
#[must_use]
pub(crate) fn posture_role_for(ceiling: PermissionCeiling) -> &'static str {
    if !ceiling.tools {
        // No tools at all; the narrowest posture, and the allowlist is empty
        // anyway.
        return "explore";
    }
    if ceiling.write {
        return "implement";
    }
    match ceiling.shell {
        ShellCeiling::None | ShellCeiling::ReadOnly => "explore",
        ShellCeiling::Full => "test",
    }
}

/// Whether a Fleet role is never allowed a mutating shell, whatever its
/// requested runtime profile says. Spawn narrows the child to a read-only
/// shell for these roles, and every receipt must report that same posture.
#[must_use]
pub(crate) fn role_requires_read_only_shell(role: &FleetRole) -> bool {
    matches!(
        role,
        FleetRole::Scout | FleetRole::Reviewer | FleetRole::Planner
    )
}

/// The runtime profile a worker of `role` actually runs under: the requested
/// profile with the shell narrowed for read-only roles. Receipts and headers
/// derive from this, never from the requested profile alone (#5542 review).
#[must_use]
pub(crate) fn effective_runtime_profile_for_role(
    role: &FleetRole,
    requested: &WorkerRuntimeProfile,
) -> WorkerRuntimeProfile {
    let mut effective = requested.clone();
    if role_requires_read_only_shell(role) && effective.shell.allows_shell() {
        effective.shell = ShellPolicy::ReadOnly;
    }
    effective
}

fn shell_policy_label(shell: ShellPolicy) -> &'static str {
    match shell {
        ShellPolicy::None => "none",
        ShellPolicy::ReadOnly => "read_only",
        ShellPolicy::Full => "full",
    }
}

fn tool_scope_label(tools: &ToolScope) -> &'static str {
    match tools {
        ToolScope::Inherit => "inherit",
        ToolScope::Explicit(_) => "explicit",
    }
}

/// Effective non-secret runtime permissions for a worker of `role` running
/// under `requested`. This is the single posture truth for both in-process
/// sub-agent snapshots and durable Fleet receipts: the requested profile with
/// the shell narrowed for read-only roles. `profile_id` / `profile_origin`
/// identify a saved Fleet member when one selected the role; `None` for
/// direct role dispatches.
#[must_use]
pub(crate) fn fleet_effective_permissions(
    role: &FleetRole,
    requested: &WorkerRuntimeProfile,
    profile_id: Option<&str>,
    profile_origin: Option<&str>,
) -> FleetEffectivePermissions {
    let profile = effective_runtime_profile_for_role(role, requested);
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
        profile_id: profile_id.map(str::to_string),
        profile_origin: profile_origin.map(str::to_string),
        source: "worker_runtime_profile".to_string(),
    }
}
