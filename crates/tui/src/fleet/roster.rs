//! Fleet roster — the persistent, inspectable party of named agent roles.
//!
//! The roster merges three layers into one config-backed lineup shared by
//! model-spawned sub-agents and fleet dispatch (#fleet-roster cutover
//! (v0.8.67)):
//!
//! - built-in members (the default party, always available),
//! - `[fleet.profiles]` entries from config.toml,
//! - workspace `.codewhale/agents/*.toml` profile files.
//!
//! Precedence is Workspace > Config > BuiltIn, merged by id. Loading never
//! fails the session: an unreadable workspace profile dir degrades to the
//! built-in + config layers with a log line.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use codewhale_config::{
    FleetConfigToml, FleetDelegationHints, FleetLoadout, FleetProfile, FleetProfilePermissions,
    FleetRole, FleetSlot,
};

use super::profile::{AgentProfile, load_workspace_agent_profiles};

/// Which layer a roster member came from. Higher layers override lower ones
/// by id (Workspace > Config > BuiltIn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    BuiltIn,
    Config,
    Workspace,
}

impl std::fmt::Display for ProfileOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BuiltIn => "built-in",
            Self::Config => "config",
            Self::Workspace => "project",
        })
    }
}

/// The merged fleet roster. Think RPG saved party / K8s runconfig: a stable,
/// named lineup of agent roles the session can inspect and dispatch against.
#[derive(Debug, Clone)]
pub struct FleetRoster {
    members: Vec<AgentProfile>,
}

impl FleetRoster {
    /// Roster containing only the built-in party. Used as the runtime default
    /// before config/workspace layers are wired in.
    #[must_use]
    pub fn built_ins_only() -> Self {
        Self {
            members: Self::built_in_members(),
        }
    }

    /// Load and merge the full roster for a workspace.
    ///
    /// Config members come from `[fleet.profiles]` (id = map key). Workspace
    /// members come from `.codewhale/agents/*.toml`; a load failure there is
    /// logged and skipped so a broken profile file cannot take down the
    /// session — the roster degrades to built-ins + config.
    #[must_use]
    pub fn load(fleet_config: &FleetConfigToml, workspace: &Path) -> Self {
        let mut built_ins = Self::built_in_members();
        let mut extras: Vec<AgentProfile> = Vec::new();

        for (id, profile) in &fleet_config.profiles {
            let member = AgentProfile {
                id: id.clone(),
                display_name: None,
                description: profile.role.description.clone(),
                profile: profile.clone(),
                source: PathBuf::from("config.toml"),
                origin: ProfileOrigin::Config,
            };
            merge_member(&mut built_ins, &mut extras, member);
        }

        match load_workspace_agent_profiles(workspace) {
            Ok(profiles) => {
                for member in profiles {
                    merge_member(&mut built_ins, &mut extras, member);
                }
            }
            Err(err) => {
                tracing::warn!(
                    workspace = %workspace.display(),
                    "fleet roster: skipping workspace agent profiles: {err:#}"
                );
            }
        }

        // Built-ins keep their canonical slot order (overrides included);
        // config/workspace-only extras follow alphabetically.
        extras.sort_by_key(|a| a.id.to_lowercase());
        let mut members = built_ins;
        members.extend(extras);
        Self { members }
    }

    /// The default party. Built-ins carry no permission grants (permissions
    /// stay at the [`FleetProfilePermissions::default`] floor); behavior comes
    /// from the role posture / system prompts plus the role `instructions`
    /// below, which encode the operation hierarchy: the **operator** (the
    /// session's `/model` selection) runs the operation and assigns managers
    /// to workflows; a **manager** is the middle manager of one workflow.
    #[must_use]
    pub fn built_in_members() -> Vec<AgentProfile> {
        [
            (
                "manager",
                FleetSlot::Manager,
                FleetLoadout::Inherit,
                "Middle manager for one workflow: decomposes it into bounded tasks, dispatches workers, integrates results, and reports to the operator.",
                Some(
                    "You lead exactly one workflow. Decompose it into bounded tasks, dispatch them to the right roles, keep work-in-progress small, integrate the results, and report a concise receipt (what was done, evidence, gaps) upward. Do not take on work outside your workflow.",
                ),
            ),
            (
                "operator",
                FleetSlot::Operator,
                FleetLoadout::Inherit,
                "The helm of the operation — the session's /model selection. Assigns managers to workflows, routes work between them, arbitrates conflicts, and reviews what comes back.",
                Some(
                    "You run the operation, not individual workflows. Assign a manager per workflow, route work and context between them, arbitrate conflicts and priorities, review the receipts that come back, and decide what runs next. Delegate execution; keep judgment.",
                ),
            ),
            (
                "scout",
                FleetSlot::Scout,
                FleetLoadout::Fast,
                "Fast read-only reconnaissance: find files, map code, gather evidence.",
                None,
            ),
            (
                "builder",
                FleetSlot::Implementer,
                FleetLoadout::Inherit,
                "Writes code: implements bounded tasks with write and shell access.",
                None,
            ),
            (
                "reviewer",
                FleetSlot::Reviewer,
                FleetLoadout::Inherit,
                "Adversarial code review: assumes the change is broken and tries to prove it — regressions, missing tests, unhandled cases. Read-only.",
                Some(
                    "Be adversarial: assume the change is wrong until the evidence proves otherwise. Actively try to refute the claims made about the work — hunt regressions, missing tests, unhandled edge cases, and quiet behavior changes. Report severity-scored findings with file:line evidence; if nothing survives your attack, say so plainly. Never patch.",
                ),
            ),
            (
                "verifier",
                FleetSlot::Verifier,
                FleetLoadout::Inherit,
                "Runs builds and tests to verify claims; reports evidence, does not patch.",
                None,
            ),
            (
                "synthesizer",
                FleetSlot::Summarizer,
                FleetLoadout::Fast,
                "Read-only synthesis: merge findings into one coherent report.",
                None,
            ),
            (
                "general",
                FleetSlot::General,
                FleetLoadout::Inherit,
                "General-purpose worker with full capabilities.",
                None,
            ),
        ]
        .into_iter()
        .map(|(id, slot, loadout, description, instructions)| AgentProfile {
            id: id.to_string(),
            display_name: None,
            description: Some(description.to_string()),
            profile: FleetProfile {
                slot,
                role: FleetRole {
                    name: id.to_string(),
                    description: Some(description.to_string()),
                    instructions: instructions.map(str::to_string),
                },
                loadout,
                model: None,
                provider: None,
                reasoning_effort: None,
                permissions: FleetProfilePermissions::default(),
                delegation: FleetDelegationHints::default(),
            },
            source: PathBuf::from("built-in"),
            origin: ProfileOrigin::BuiltIn,
        })
        .collect()
    }

    /// Look up a member by id (trimmed, case-insensitive).
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&AgentProfile> {
        let id = id.trim();
        self.members
            .iter()
            .find(|member| member.id.trim().eq_ignore_ascii_case(id))
    }

    /// All members in stable order: built-in canonical order first (an
    /// overridden built-in keeps its slot but shows its overriding origin),
    /// then extra config/workspace-only members alphabetically.
    #[must_use]
    pub fn members(&self) -> &[AgentProfile] {
        &self.members
    }

    /// Per-member explicit model pins, keyed by lowercased member id.
    /// Feeds the sub-agent `role_models` lookup; explicit `[subagents]`
    /// overrides are merged on top by the engine and win.
    #[must_use]
    pub fn model_overrides(&self) -> HashMap<String, String> {
        self.members
            .iter()
            .filter_map(|member| {
                let model = member.profile.model.as_deref()?.trim();
                (!model.is_empty()).then(|| (member.id.to_lowercase(), model.to_string()))
            })
            .collect()
    }
}

/// Overlay `member` onto the roster layers: replace an existing member with
/// the same id (case-insensitive) in place, otherwise collect it as an extra.
fn merge_member(
    built_ins: &mut [AgentProfile],
    extras: &mut Vec<AgentProfile>,
    member: AgentProfile,
) {
    let matches =
        |existing: &AgentProfile| existing.id.trim().eq_ignore_ascii_case(member.id.trim());
    if let Some(slot) = built_ins.iter_mut().find(|existing| matches(existing)) {
        *slot = member;
    } else if let Some(slot) = extras.iter_mut().find(|existing| matches(existing)) {
        *slot = member;
    } else {
        extras.push(member);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn config_with_profiles(profiles: BTreeMap<String, FleetProfile>) -> FleetConfigToml {
        FleetConfigToml {
            profiles,
            ..FleetConfigToml::default()
        }
    }

    fn config_profile(role: &str, model: Option<&str>) -> FleetProfile {
        FleetProfile {
            slot: FleetSlot::from_name(role),
            role: FleetRole {
                name: role.to_string(),
                description: Some(format!("{role} from config")),
                instructions: None,
            },
            loadout: FleetLoadout::Inherit,
            model: model.map(str::to_string),
            provider: None,
            reasoning_effort: None,
            permissions: FleetProfilePermissions::default(),
            delegation: FleetDelegationHints::default(),
        }
    }

    fn write_workspace_profile(workspace: &Path, filename: &str, contents: &str) {
        let dir = workspace.join(super::super::profile::WORKSPACE_AGENT_PROFILE_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(filename), contents).unwrap();
    }

    #[test]
    fn built_in_party_is_complete_with_floor_permissions() {
        let members = FleetRoster::built_in_members();
        let ids: Vec<&str> = members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "manager",
                "operator",
                "scout",
                "builder",
                "reviewer",
                "verifier",
                "synthesizer",
                "general"
            ]
        );
        for member in &members {
            assert_eq!(member.origin, ProfileOrigin::BuiltIn, "{}", member.id);
            assert_eq!(
                member.profile.permissions,
                FleetProfilePermissions::default(),
                "built-in {} must stay at the permission floor",
                member.id
            );
            assert_eq!(
                member.profile.delegation,
                FleetDelegationHints::default(),
                "{}",
                member.id
            );
            assert!(member.profile.model.is_none(), "{}", member.id);
            // The coordination hierarchy (operator/manager) and the
            // adversarial reviewer carry role doctrine; the remaining
            // built-ins get behavior from posture / system prompts alone.
            let carries_doctrine =
                matches!(member.id.as_str(), "manager" | "operator" | "reviewer");
            assert_eq!(
                member.profile.role.instructions.is_some(),
                carries_doctrine,
                "built-in {} instructions presence",
                member.id
            );
            assert!(member.description.is_some(), "{}", member.id);
        }
        assert_eq!(members[0].profile.slot, FleetSlot::Manager);
        assert_eq!(members[1].profile.slot, FleetSlot::Operator);
        assert_eq!(members[2].profile.loadout, FleetLoadout::Fast);
        assert_eq!(members[6].profile.slot, FleetSlot::Summarizer);
        assert_eq!(members[6].profile.loadout, FleetLoadout::Fast);
    }

    #[test]
    fn config_member_overrides_built_in_and_extras_sort_alphabetically() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_profiles(BTreeMap::from([
            (
                "reviewer".to_string(),
                config_profile("reviewer", Some("deepseek-v4-pro")),
            ),
            ("zeta".to_string(), config_profile("scout", None)),
            ("alpha".to_string(), config_profile("builder", None)),
        ]));

        let roster = FleetRoster::load(&config, tmp.path());

        let ids: Vec<&str> = roster.members().iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "manager",
                "operator",
                "scout",
                "builder",
                "reviewer",
                "verifier",
                "synthesizer",
                "general",
                "alpha",
                "zeta"
            ],
            "overridden built-in keeps its slot; extras follow alphabetically"
        );
        let reviewer = roster.get("reviewer").unwrap();
        assert_eq!(reviewer.origin, ProfileOrigin::Config);
        assert_eq!(reviewer.profile.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(reviewer.source, PathBuf::from("config.toml"));
    }

    #[test]
    fn workspace_member_wins_over_config_and_built_in() {
        let tmp = TempDir::new().unwrap();
        write_workspace_profile(
            tmp.path(),
            "reviewer.toml",
            "id = \"reviewer\"\nrole_hint = \"reviewer\"\nmodel = \"glm-5.2\"\n",
        );
        let config = config_with_profiles(BTreeMap::from([(
            "reviewer".to_string(),
            config_profile("reviewer", Some("deepseek-v4-pro")),
        )]));

        let roster = FleetRoster::load(&config, tmp.path());

        let reviewer = roster.get("reviewer").unwrap();
        assert_eq!(reviewer.origin, ProfileOrigin::Workspace);
        assert_eq!(reviewer.profile.model.as_deref(), Some("glm-5.2"));
        // Precedence must not duplicate the member.
        assert_eq!(
            roster
                .members()
                .iter()
                .filter(|m| m.id == "reviewer")
                .count(),
            1
        );
    }

    #[test]
    fn broken_workspace_dir_degrades_to_built_ins_and_config() {
        let tmp = TempDir::new().unwrap();
        // A malformed provider token is still a load failure (#4093 / #3965):
        // profile pins may name built-ins or simple custom ids like
        // `lm-studio`, but whitespace/punctuation is rejected so a broken
        // workspace dir still degrades to built-ins + config.
        write_workspace_profile(
            tmp.path(),
            "broken.toml",
            "provider = \"not a real provider\"\n",
        );
        let config = config_with_profiles(BTreeMap::from([(
            "extra".to_string(),
            config_profile("scout", None),
        )]));

        let roster = FleetRoster::load(&config, tmp.path());

        assert!(roster.get("extra").is_some());
        assert_eq!(
            roster.members().len(),
            FleetRoster::built_in_members().len() + 1
        );
    }

    #[test]
    fn model_overrides_use_lowercased_ids_and_only_explicit_models() {
        let tmp = TempDir::new().unwrap();
        let config = config_with_profiles(BTreeMap::from([
            (
                "Reviewer".to_string(),
                config_profile("reviewer", Some("deepseek-v4-pro")),
            ),
            ("scout".to_string(), config_profile("scout", None)),
        ]));

        let roster = FleetRoster::load(&config, tmp.path());
        let overrides = roster.model_overrides();

        assert_eq!(
            overrides,
            HashMap::from([("reviewer".to_string(), "deepseek-v4-pro".to_string())]),
            "only members with explicit models are pinned, keyed lowercased"
        );
    }

    #[test]
    fn get_is_trimmed_and_case_insensitive() {
        let roster = FleetRoster::built_ins_only();
        assert!(roster.get("  Reviewer ").is_some());
        assert!(roster.get("SYNTHESIZER").is_some());
        assert!(roster.get("nonexistent").is_none());
    }

    #[test]
    fn origin_labels_are_stable() {
        assert_eq!(ProfileOrigin::BuiltIn.to_string(), "built-in");
        assert_eq!(ProfileOrigin::Config.to_string(), "config");
        assert_eq!(ProfileOrigin::Workspace.to_string(), "project");
    }
}
