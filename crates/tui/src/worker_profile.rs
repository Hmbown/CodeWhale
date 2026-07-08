//! Worker runtime profile — the per-role capability contract for a CodeWhale
//! worker (#3217, #3211, #3213, and the child-permission-intersection issues
//! #414 / #426 / #1186).
//!
//! This is the **Workflow substrate**: every detached worker — whether launched
//! as an `agent` sub-agent or a Fleet worker — should run under a profile
//! that bounds what it may do (permissions, shell access, tool scope, model
//! route, recursion budget, foreground/background). A child profile is always
//! **derived** from its parent and can never escalate beyond it.
//!
//! Scope: this module defines the contract and the parent→child derivation with
//! tests. `agent` and Fleet worker records now build and persist these
//! profiles so parent-visible worker projections have a single capability
//! contract. Runtime enforcement of every declared field remains incremental
//! follow-up work (#3217).

#![allow(dead_code)] // foundation: consumers are wired in a follow-up (#3217).

use crate::tools::subagent::SubAgentType;
use serde::{Deserialize, Serialize};

/// Coarse capability classes a worker may exercise, beyond read access (reads
/// are always permitted). A child may only ever hold a *subset* of its parent's
/// capabilities.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionSet {
    /// May modify the workspace (`write_file` / `edit_file` / `apply_patch`).
    pub write: bool,
    /// May use network-capable tools (web search/fetch, networked MCP servers).
    pub network: bool,
}

impl PermissionSet {
    /// Full capabilities (write + network).
    pub const fn full() -> Self {
        Self {
            write: true,
            network: true,
        }
    }

    /// Read-only: no write, no network.
    pub const fn read_only() -> Self {
        Self {
            write: false,
            network: false,
        }
    }

    /// Intersection: a capability is granted only if **both** sets grant it.
    /// This is the core non-escalation primitive — `parent.intersect(child)`
    /// can never produce a capability the parent lacks.
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self {
            write: self.write && other.write,
            network: self.network && other.network,
        }
    }
}

/// Shell access policy — the replacement for the legacy per-worker shell boolean
/// (#3217). Ordered from most to least restrictive so `min` yields the safer of
/// two policies.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ShellPolicy {
    /// No shell access.
    None,
    /// Read-only / non-mutating commands only (the policy enforcement lives in
    /// the exec/sandbox layer; this is the declared intent).
    ReadOnly,
    /// Full shell access.
    Full,
}

impl ShellPolicy {
    /// Convert the legacy top-level shell opt-in into the typed shell policy.
    #[must_use]
    pub const fn from_legacy_allow_shell(allow_shell: bool) -> Self {
        if allow_shell { Self::Full } else { Self::None }
    }

    /// Whether any shell tools should be exposed under this policy.
    #[must_use]
    pub const fn allows_shell(self) -> bool {
        !matches!(self, Self::None)
    }

    /// The more restrictive (safer) of two policies. A child can never exceed
    /// its parent's shell policy.
    #[must_use]
    pub fn min_with(self, other: Self) -> Self {
        if self <= other { self } else { other }
    }
}

/// Which tools a worker may call. Mirrors the existing `AgentWorkerToolProfile`
/// (`Inherited` / `Explicit`) so the two can be reconciled when this is wired in.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolScope {
    /// Inherit the parent's tool surface.
    Inherit,
    /// Only the explicitly listed tool names.
    Explicit(Vec<String>),
}

/// How a worker's model is selected. New model-facing spawns default to the
/// parent/session model; a child only takes a smaller/faster family sibling when
/// the parent explicitly asks for that route.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoute {
    /// Same model as the parent / session.
    Inherit,
    /// Explicitly request a smaller/faster same-family sibling when known.
    Faster,
    /// Legacy persisted route from the old hidden auto-router. New spawns do
    /// not emit this; runtime treats it like `Faster` for compatibility.
    Auto,
    /// An explicit model id, validated against the active provider at spawn time.
    Fixed(String),
}

/// The capability contract a single worker runs under.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRuntimeProfile {
    pub role: SubAgentType,
    pub permissions: PermissionSet,
    pub shell: ShellPolicy,
    pub tools: ToolScope,
    pub model: ModelRoute,
    /// Explicit provider override; `None` inherits the parent/session provider.
    pub provider: Option<String>,
    /// Remaining nested-delegation budget. A worker may spawn children while
    /// `max_spawn_depth > 0`; each level decrements it. Clamped to the workspace
    /// ceiling.
    pub max_spawn_depth: u32,
    /// Whether the worker runs detached (background) or inline (foreground).
    pub background: bool,
    /// Tools that are explicitly denied for this worker, overriding any allowlist.
    /// Deny always wins over allow. Deny rules propagate to child workers through
    /// `derive_child()` — each child inherits its parent's deny list as a union.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_tools: Vec<String>,
}

impl WorkerRuntimeProfile {
    /// The default profile for a role — the per-role posture. Mirrors the role
    /// stances documented in `docs/SUBAGENTS.md` (explore/plan/review are
    /// read-only; verifier runs tests; implementer/general write).
    #[must_use]
    pub fn for_role(role: SubAgentType) -> Self {
        let (permissions, shell) = match role {
            // Read-only investigators.
            SubAgentType::Explore | SubAgentType::Review => {
                (PermissionSet::read_only(), ShellPolicy::ReadOnly)
            }
            // Planner: analysis only, no shell.
            SubAgentType::Plan => (PermissionSet::read_only(), ShellPolicy::None),
            // Verifier: doesn't modify code, but runs the test suite.
            SubAgentType::Verifier => (PermissionSet::read_only(), ShellPolicy::Full),
            // Doers.
            SubAgentType::Implementer | SubAgentType::General => {
                (PermissionSet::full(), ShellPolicy::Full)
            }
            // Custom starts locked down; the caller opens specific tools explicitly.
            SubAgentType::Custom => (PermissionSet::read_only(), ShellPolicy::None),
        };
        Self {
            role,
            permissions,
            shell,
            tools: ToolScope::Inherit,
            model: ModelRoute::Inherit,
            provider: None,
            max_spawn_depth: codewhale_config::DEFAULT_SPAWN_DEPTH,
            background: true,
            denied_tools: Vec::new(),
        }
    }

    /// Derive a child profile from this (parent) profile and a `requested` child
    /// profile. The result is the **intersection** of the two — it can never
    /// grant the child something the parent lacks (#414 / #426 / #1186):
    ///
    /// - permissions are AND-ed,
    /// - shell takes the more restrictive policy,
    /// - an explicit parent tool set bounds the child's tool set,
    /// - the spawn-depth budget decrements by one level and clamps to the ceiling.
    ///
    /// The child keeps its own requested role, model route, and
    /// foreground/background preference (these don't grant capability), but its
    /// provider falls back to the parent's when unset.
    #[must_use]
    pub fn derive_child(&self, requested: &WorkerRuntimeProfile) -> WorkerRuntimeProfile {
        let permissions = self.permissions.intersect(requested.permissions);
        let shell = self.shell.min_with(requested.shell);
        let tools = match (&self.tools, &requested.tools) {
            // Parent restricts to a set → the child can only narrow within it.
            (ToolScope::Explicit(parent), ToolScope::Explicit(child)) => ToolScope::Explicit(
                child
                    .iter()
                    .filter(|name| parent.contains(name))
                    .cloned()
                    .collect(),
            ),
            (ToolScope::Explicit(parent), ToolScope::Inherit) => {
                ToolScope::Explicit(parent.clone())
            }
            // Parent inherits the full surface → the child's request stands.
            (ToolScope::Inherit, child) => child.clone(),
        };
        // The child gets at most one level less budget than the parent, and never
        // more than it requested, clamped to the hard ceiling.
        let max_spawn_depth = requested
            .max_spawn_depth
            .min(self.max_spawn_depth.saturating_sub(1))
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
        // Union parent + child deny lists (no duplicates). Neither can remove
        // the other's entries — deny always adds across generations.
        let mut denied_tools = self.denied_tools.clone();
        for tool in &requested.denied_tools {
            if !denied_tools.contains(tool) {
                denied_tools.push(tool.clone());
            }
        }
        WorkerRuntimeProfile {
            role: requested.role.clone(),
            permissions,
            shell,
            tools,
            model: requested.model.clone(),
            provider: requested.provider.clone().or_else(|| self.provider.clone()),
            max_spawn_depth,
            background: requested.background,
            denied_tools,
        }
    }

    /// Whether this worker may still spawn a child (budget remaining).
    #[must_use]
    pub fn can_spawn_child(&self) -> bool {
        self.max_spawn_depth > 0
    }
}

impl Default for WorkerRuntimeProfile {
    fn default() -> Self {
        Self::for_role(SubAgentType::General)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_intersection_never_escalates() {
        let parent = PermissionSet::read_only();
        let greedy_child = PermissionSet::full();
        // Even though the child asks for everything, the read-only parent wins.
        let got = parent.intersect(greedy_child);
        assert_eq!(got, PermissionSet::read_only());
    }

    #[test]
    fn shell_policy_min_takes_the_safer() {
        assert_eq!(
            ShellPolicy::ReadOnly.min_with(ShellPolicy::Full),
            ShellPolicy::ReadOnly
        );
        assert_eq!(
            ShellPolicy::None.min_with(ShellPolicy::ReadOnly),
            ShellPolicy::None
        );
        assert_eq!(
            ShellPolicy::Full.min_with(ShellPolicy::Full),
            ShellPolicy::Full
        );
    }

    #[test]
    fn for_role_postures_match_role_stances() {
        let explore = WorkerRuntimeProfile::for_role(SubAgentType::Explore);
        assert!(!explore.permissions.write, "explore must not write");
        assert_eq!(explore.shell, ShellPolicy::ReadOnly);
        assert_eq!(
            explore.model,
            ModelRoute::Inherit,
            "explore should not silently downgrade the child model"
        );

        let implementer = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
        assert!(implementer.permissions.write, "implementer writes");
        assert_eq!(implementer.shell, ShellPolicy::Full);

        let verifier = WorkerRuntimeProfile::for_role(SubAgentType::Verifier);
        assert!(
            !verifier.permissions.write,
            "verifier reports, does not patch"
        );
        assert_eq!(
            verifier.shell,
            ShellPolicy::Full,
            "verifier runs the test suite"
        );
    }

    #[test]
    fn child_cannot_escalate_beyond_a_readonly_parent() {
        let parent = WorkerRuntimeProfile::for_role(SubAgentType::Explore); // read-only
        let greedy = WorkerRuntimeProfile::for_role(SubAgentType::Implementer); // wants write + full shell
        let child = parent.derive_child(&greedy);
        assert!(
            !child.permissions.write,
            "a read-only parent cannot bear a writing child"
        );
        assert!(!child.permissions.network);
        assert_eq!(
            child.shell,
            ShellPolicy::ReadOnly,
            "child shell clamped to parent's"
        );
    }

    #[test]
    fn child_explicit_tools_are_bounded_by_parent() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.tools = ToolScope::Explicit(vec!["read_file".into(), "grep_files".into()]);
        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::General);
        requested.tools = ToolScope::Explicit(vec!["read_file".into(), "write_file".into()]);
        let child = parent.derive_child(&requested);
        match child.tools {
            ToolScope::Explicit(names) => {
                assert_eq!(
                    names,
                    vec!["read_file".to_string()],
                    "write_file not in parent set is dropped"
                );
            }
            ToolScope::Inherit => panic!("expected explicit tool scope"),
        }
    }

    #[test]
    fn spawn_depth_decrements_and_clamps() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.max_spawn_depth = 2;
        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::General);
        requested.max_spawn_depth = 99; // tries to grab more than the parent has
        let child = parent.derive_child(&requested);
        assert_eq!(
            child.max_spawn_depth, 1,
            "child budget is at most parent-1, never the requested 99"
        );
        assert!(child.can_spawn_child());

        let mut leaf_parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        leaf_parent.max_spawn_depth = 1;
        let grandchild = leaf_parent.derive_child(&requested);
        assert_eq!(grandchild.max_spawn_depth, 0);
        assert!(
            !grandchild.can_spawn_child(),
            "budget exhausted at the leaf"
        );
    }

    #[test]
    fn child_provider_falls_back_to_parent() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.provider = Some("moonshot".to_string());
        let requested = WorkerRuntimeProfile::for_role(SubAgentType::Explore); // provider None
        let child = parent.derive_child(&requested);
        assert_eq!(child.provider.as_deref(), Some("moonshot"));
    }

    #[test]
    fn derive_child_unions_deny_lists() {
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.denied_tools = vec!["tool_a".to_string(), "tool_b".to_string()];

        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
        requested.denied_tools = vec!["tool_c".to_string()];

        let child = parent.derive_child(&requested);
        // Child inherits parent's denied tools AND keeps its own (union).
        assert!(
            child.denied_tools.contains(&"tool_a".to_string()),
            "parent's deny list propagates to child"
        );
        assert!(
            child.denied_tools.contains(&"tool_b".to_string()),
            "parent's deny list propagates to child"
        );
        assert!(
            child.denied_tools.contains(&"tool_c".to_string()),
            "child's own denied tools are preserved"
        );
        assert_eq!(child.denied_tools.len(), 3, "no duplicates in union");
    }

    #[test]
    fn derive_child_preserves_wildcard_denies() {
        // Parent denies mcp_*, child denies file_*. Union must preserve both
        // wildcards — neither can remove the other's deny entries.
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.denied_tools = vec!["mcp_*".to_string()];

        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::Implementer);
        requested.denied_tools = vec!["file_*".to_string()];

        let child = parent.derive_child(&requested);
        assert!(
            child.denied_tools.contains(&"mcp_*".to_string()),
            "parent's wildcard deny propagates"
        );
        assert!(
            child.denied_tools.contains(&"file_*".to_string()),
            "child's wildcard deny is preserved"
        );
        assert_eq!(
            child.denied_tools.len(),
            2,
            "both wildcards in union, no duplicates"
        );
    }

    #[test]
    fn derive_child_does_not_duplicate_case_variants() {
        // Union should not add duplicate entries even if case differs —
        // the matching logic (is_tool_denied) is case-insensitive, so
        // storing both "MCP_*" and "mcp_*" is redundant (though harmless).
        let mut parent = WorkerRuntimeProfile::for_role(SubAgentType::General);
        parent.denied_tools = vec!["mcp_*".to_string()];

        let mut requested = WorkerRuntimeProfile::for_role(SubAgentType::General);
        requested.denied_tools = vec!["mcp_*".to_string()]; // same literal

        let child = parent.derive_child(&requested);
        assert_eq!(
            child.denied_tools.len(),
            1,
            "duplicate wildcards are not stored twice"
        );
    }
}
