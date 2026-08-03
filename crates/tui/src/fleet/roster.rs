//! Fleet roster — the persistent, inspectable party of named agent roles.
//!
//! The roster merges four layers into one config-backed lineup shared by
//! model-spawned sub-agents and fleet dispatch (#fleet-roster cutover
//! (v0.8.67)):
//!
//! - built-in members (the default party, always available),
//! - `[fleet.profiles]` entries from config.toml,
//! - personal `$CODEWHALE_HOME/agents/*.toml` profile files,
//! - workspace `.codewhale/agents/*.toml` profile files.
//!
//! Precedence is Workspace > Personal > Config > BuiltIn, merged by id. Loading never
//! fails the session: an unreadable workspace profile dir degrades to the
//! built-in + config layers with a log line.
//!
//! Two guardrails (#5098):
//!
//! - Shadowing is recorded, not silent: when a higher layer displaces a
//!   lower-precedence file for the same id, the roster keeps a
//!   [`ShadowedProfile`] receipt (logged at load, badged in the roster view)
//!   so an edit in the losing layer is visibly ignored rather than dropped.
//! - Project-scope profiles (`.codewhale/agents/*.toml`) join the roster only
//!   when project-level config is trusted for the launch; `--no-project-config`
//!   opts the whole layer out, same as `.codewhale/config.toml` (#485).

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use codewhale_config::{
    FleetConfigToml, FleetDelegationHints, FleetLoadout, FleetProfile, FleetProfilePermissions,
    FleetRole, FleetSlot,
};

use super::profile::{
    AgentProfile, load_agent_profiles_from_dir_tolerant, load_workspace_agent_profiles_tolerant,
    personal_agent_profile_dir,
};

/// Which layer a roster member came from. Higher layers override lower ones
/// by id (Workspace > Personal > Config > BuiltIn).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    BuiltIn,
    Config,
    Personal,
    Workspace,
}

impl std::fmt::Display for ProfileOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BuiltIn => "built-in",
            Self::Config => "config",
            Self::Personal => "personal",
            Self::Workspace => "project",
        })
    }
}

/// The merged fleet roster. Think RPG saved party / K8s runconfig: a stable,
/// named lineup of agent roles the session can inspect and dispatch against.
#[derive(Debug, Clone)]
pub struct FleetRoster {
    members: Vec<AgentProfile>,
    /// Lower-precedence profiles displaced by a higher layer for the same id
    /// (#5098). Shadowing is normal precedence, but it must be VISIBLE: a
    /// personal edit that loses to a stale project copy otherwise changes
    /// nothing anywhere with no signal why.
    shadowed: Vec<ShadowedProfile>,
}

/// A lower-precedence profile displaced by a higher layer for the same id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShadowedProfile {
    pub id: String,
    pub shadowed_origin: ProfileOrigin,
    pub shadowed_source: PathBuf,
    pub winner_origin: ProfileOrigin,
    pub winner_source: PathBuf,
}

/// Process-launch decision: whether project-scope agent profiles
/// (`.codewhale/agents/*.toml`) may join the dispatch roster (#5098). Set
/// once from `--no-project-config` at launch so every roster re-read (spawn
/// refresh, dispatch, views) honors the same trust decision other
/// project-level config already has (#485). Defaults to enabled, matching
/// project config itself.
static PROJECT_AGENT_PROFILES_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

/// Record the launch-time trust decision for project-scope agent profiles.
pub fn set_project_agent_profiles_enabled(enabled: bool) {
    PROJECT_AGENT_PROFILES_ENABLED.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether project-scope agent profiles join the roster in this process.
#[must_use]
pub fn project_agent_profiles_enabled() -> bool {
    PROJECT_AGENT_PROFILES_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

impl FleetRoster {
    /// Roster containing only the built-in party. Used as the runtime default
    /// before config/workspace layers are wired in.
    #[must_use]
    pub fn built_ins_only() -> Self {
        Self {
            members: Self::built_in_members(),
            shadowed: Vec::new(),
        }
    }

    /// A roster built from an explicit member list.
    ///
    /// Used for run-scoped rosters that are not a merge of the config layers —
    /// notably an exact named Fleet, whose members are frozen at Workflow
    /// start and must not pick up built-in or workspace profiles by name.
    #[must_use]
    pub fn from_members(members: Vec<AgentProfile>) -> Self {
        Self {
            members,
            shadowed: Vec::new(),
        }
    }

    /// Load and merge the full roster for a workspace.
    ///
    /// Config members come from `[fleet.profiles]` (id = map key). Personal
    /// members come from `$CODEWHALE_HOME/agents/*.toml`, and workspace members
    /// come from `.codewhale/agents/*.toml`. A load failure is logged and
    /// skipped so one broken profile layer cannot take down the session.
    #[must_use]
    pub fn load(fleet_config: &FleetConfigToml, workspace: &Path) -> Self {
        let personal_dir = personal_agent_profile_dir().ok();
        Self::load_with_personal_dir(
            fleet_config,
            workspace,
            personal_dir.as_deref(),
            project_agent_profiles_enabled(),
        )
    }

    fn load_with_personal_dir(
        fleet_config: &FleetConfigToml,
        workspace: &Path,
        personal_dir: Option<&Path>,
        include_workspace_profiles: bool,
    ) -> Self {
        let mut built_ins = Self::built_in_members();
        let mut extras: Vec<AgentProfile> = Vec::new();
        let mut shadowed: Vec<ShadowedProfile> = Vec::new();

        for (id, profile) in &fleet_config.profiles {
            let mut profile = profile.clone();
            profile.role.name = super::profile::canonical_public_role_name(&profile.role.name);
            profile.slot = FleetSlot::from_name(&profile.role.name);
            let member = AgentProfile {
                id: id.clone(),
                display_name: None,
                description: profile.role.description.clone(),
                profile,
                source: PathBuf::from("config.toml"),
                origin: ProfileOrigin::Config,
            };
            record_shadow(
                merge_member(&mut built_ins, &mut extras, member),
                &mut shadowed,
            );
        }

        if let Some(personal_dir) = personal_dir {
            match load_agent_profiles_from_dir_tolerant(personal_dir, ProfileOrigin::Personal) {
                Ok((profiles, issues)) => {
                    for issue in issues {
                        tracing::warn!(
                            "fleet roster: skipping invalid personal agent profile: {issue}"
                        );
                    }
                    for member in profiles {
                        record_shadow(
                            merge_member(&mut built_ins, &mut extras, member),
                            &mut shadowed,
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!("fleet roster: skipping personal agent profiles: {err:#}");
                }
            }
        }

        // #5098: project-scope profiles join the dispatch roster only when the
        // launch trusted project-level config (`--no-project-config` opts the
        // whole layer out, same as `.codewhale/config.toml`).
        if include_workspace_profiles {
            match load_workspace_agent_profiles_tolerant(workspace) {
                Ok((profiles, issues)) => {
                    for issue in issues {
                        tracing::warn!(
                            workspace = %workspace.display(),
                            "fleet roster: skipping invalid workspace agent profile: {issue}"
                        );
                    }
                    for member in profiles {
                        record_shadow(
                            merge_member(&mut built_ins, &mut extras, member),
                            &mut shadowed,
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        workspace = %workspace.display(),
                        "fleet roster: skipping workspace agent profiles: {err:#}"
                    );
                }
            }
        }

        for shadow in &shadowed {
            // Overriding a built-in is the intended customization path —
            // keep it quiet. A file layer (config/personal) losing to another
            // file layer is the #5098 footgun: the edit changes nothing
            // anywhere and must be visible.
            if shadow.shadowed_origin == ProfileOrigin::BuiltIn {
                tracing::debug!(
                    "fleet roster: '{}' {} copy at {} overrides the built-in default",
                    shadow.id,
                    shadow.winner_origin,
                    shadow.winner_source.display()
                );
            } else {
                tracing::warn!(
                    "fleet roster: '{}' {} copy at {} shadows the {} copy at {} (ignored)",
                    shadow.id,
                    shadow.winner_origin,
                    shadow.winner_source.display(),
                    shadow.shadowed_origin,
                    shadow.shadowed_source.display()
                );
            }
        }

        // Built-ins keep their canonical slot order (overrides included);
        // config/workspace-only extras follow alphabetically.
        extras.sort_by_key(|a| a.id.to_lowercase());
        let mut members = built_ins;
        members.extend(extras);
        Self { members, shadowed }
    }

    /// The default party. Built-ins carry no permission grants (permissions
    /// stay at the [`FleetProfilePermissions::default`] floor); behavior comes
    /// from the role posture / system prompts plus the role `instructions`
    /// below, which encode the coordination hierarchy: the **operator** (the
    /// session's `/model` selection) directs the work and assigns managers
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
                "The helm of the session — the session's /model selection. Assigns managers to Workflows, routes work between them, arbitrates conflicts, and reviews what comes back.",
                Some(
                    "You direct the overall work, not individual Workflow steps. Assign a manager per Workflow, route work and context between them, arbitrate conflicts and priorities, review the receipts that come back, and decide what runs next. Delegate execution; keep judgment.",
                ),
            ),
            (
                "scout",
                FleetSlot::Scout,
                FleetLoadout::Inherit,
                "Read-only reconnaissance: find files, map code, gather evidence.",
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
                "consultant",
                FleetSlot::Custom("consultant".to_string()),
                FleetLoadout::Inherit,
                "Short-lived, high-reasoning, read-only counsel for difficult decisions and overlooked risks.",
                Some(
                    "Give the operator a direct second opinion grounded in what you can read. Surface the decisive tradeoff, overlooked failure mode, and your recommendation. Advise only: do not edit files or run commands.",
                ),
            ),
            (
                "synthesizer",
                FleetSlot::Summarizer,
                FleetLoadout::Inherit,
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
                reasoning_effort: (id == "consultant").then(|| "high".to_string()),
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
    /// Lower-precedence profiles displaced by higher layers (#5098). Empty
    /// for `built_ins_only` / `from_members` rosters.
    #[must_use]
    pub fn shadowed(&self) -> &[ShadowedProfile] {
        &self.shadowed
    }

    /// Shadow records for one member id (trimmed, case-insensitive).
    pub fn shadowed_for<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a ShadowedProfile> {
        let id = id.trim().to_lowercase();
        self.shadowed
            .iter()
            .filter(move |shadow| shadow.id.trim().eq_ignore_ascii_case(&id))
    }
}

/// Fold a displaced layer (if any) into the shadow log.
fn record_shadow(displaced: Option<ShadowedProfile>, shadowed: &mut Vec<ShadowedProfile>) {
    if let Some(shadow) = displaced {
        shadowed.push(shadow);
    }
}

/// Overlay `member` onto the roster layers: replace an existing member with
/// the same id (case-insensitive) in place, otherwise collect it as an extra.
/// Returns a shadow record when a lower-precedence layer was displaced so the
/// load can log it and the roster can surface it (#5098).
fn merge_member(
    built_ins: &mut [AgentProfile],
    extras: &mut Vec<AgentProfile>,
    member: AgentProfile,
) -> Option<ShadowedProfile> {
    let matches =
        |existing: &AgentProfile| existing.id.trim().eq_ignore_ascii_case(member.id.trim());
    let slot = built_ins
        .iter_mut()
        .find(|existing| matches(existing))
        .or_else(|| extras.iter_mut().find(|existing| matches(existing)));
    match slot {
        Some(existing) => {
            let shadow = ShadowedProfile {
                id: existing.id.clone(),
                shadowed_origin: existing.origin,
                shadowed_source: existing.source.clone(),
                winner_origin: member.origin,
                winner_source: member.source.clone(),
            };
            *existing = member;
            Some(shadow)
        }
        None => {
            extras.push(member);
            None
        }
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
                "consultant",
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
            assert_eq!(
                member.profile.reasoning_effort.as_deref(),
                (member.id == "consultant").then_some("high"),
                "built-in {} reasoning",
                member.id
            );
            // The coordination hierarchy (operator/manager) and the
            // adversarial reviewer carry role doctrine; the remaining
            // built-ins get behavior from posture / system prompts alone.
            let carries_doctrine = matches!(
                member.id.as_str(),
                "manager" | "operator" | "reviewer" | "consultant"
            );
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
        assert_eq!(members[2].profile.loadout, FleetLoadout::Inherit);
        assert_eq!(members[6].profile.slot.as_str(), "consultant");
        assert_eq!(members[7].profile.slot, FleetSlot::Summarizer);
        assert_eq!(members[7].profile.loadout, FleetLoadout::Inherit);
    }

    #[test]
    fn config_member_overrides_built_in_and_extras_sort_alphabetically() {
        let _env_lock = crate::test_support::lock_test_env();
        let home = TempDir::new().unwrap();
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let tmp = TempDir::new().unwrap();
        let config = config_with_profiles(BTreeMap::from([
            (
                "reviewer".to_string(),
                config_profile("reviewer", Some("deepseek-v4-pro")),
            ),
            ("zeta".to_string(), config_profile("scout", None)),
            ("alpha".to_string(), config_profile("builder", None)),
        ]));

        // Isolate from ambient personal agent profiles on developer machines.
        let roster = FleetRoster::load_with_personal_dir(&config, tmp.path(), None, true);

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
                "consultant",
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
    fn personal_member_applies_across_projects_but_project_still_wins() {
        let tmp = TempDir::new().unwrap();
        let personal_dir = tmp.path().join("personal-agents");
        std::fs::create_dir_all(&personal_dir).unwrap();
        std::fs::write(
            personal_dir.join("reviewer.toml"),
            "id = \"reviewer\"\nrole_hint = \"reviewer\"\nmodel = \"deepseek-v4-flash\"\n",
        )
        .unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let personal = FleetRoster::load_with_personal_dir(
            &FleetConfigToml::default(),
            &workspace,
            Some(&personal_dir),
            true,
        );
        let reviewer = personal.get("reviewer").unwrap();
        assert_eq!(reviewer.origin, ProfileOrigin::Personal);
        assert_eq!(reviewer.profile.model.as_deref(), Some("deepseek-v4-flash"));

        write_workspace_profile(
            &workspace,
            "reviewer.toml",
            "id = \"reviewer\"\nrole_hint = \"reviewer\"\nmodel = \"glm-5.2\"\n",
        );
        let project = FleetRoster::load_with_personal_dir(
            &FleetConfigToml::default(),
            &workspace,
            Some(&personal_dir),
            true,
        );
        let reviewer = project.get("reviewer").unwrap();
        assert_eq!(reviewer.origin, ProfileOrigin::Workspace);
        assert_eq!(reviewer.profile.model.as_deref(), Some("glm-5.2"));
    }

    #[test]
    fn personal_setup_target_round_trips_through_the_runtime_roster() {
        let _env_lock = crate::test_support::lock_test_env();
        let home = TempDir::new().unwrap();
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let workspace = TempDir::new().unwrap();
        let personal_dir = super::super::profile::agent_profile_dir_for_scope(
            super::super::profile::FleetProfileScope::Personal,
            workspace.path(),
        )
        .expect("personal profile directory");
        assert_eq!(personal_dir, home.path().join("agents"));

        let target = personal_dir.join("reviewer.toml");
        let mut transaction = codewhale_config::persistence::SetupTransaction::new();
        transaction.stage(
            target.clone(),
            b"id = \"reviewer\"\nrole_hint = \"reviewer\"\nprovider = \"deepseek\"\nmodel = \"deepseek-v4-flash\"\n"
                .to_vec(),
        );
        transaction.commit().expect("atomic personal save");
        assert!(target.is_file(), "save must land under CODEWHALE_HOME");

        let roster = FleetRoster::load(&FleetConfigToml::default(), workspace.path());
        let reviewer = roster
            .get("reviewer")
            .expect("saved personal profile must be loaded");
        assert_eq!(reviewer.origin, ProfileOrigin::Personal);
        assert_eq!(reviewer.source, target);
        assert_eq!(reviewer.profile.provider.as_deref(), Some("deepseek"));
        assert_eq!(reviewer.profile.model.as_deref(), Some("deepseek-v4-flash"));
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
    fn invalid_legacy_profile_does_not_hide_valid_scout_neighbor() {
        let _env_lock = crate::test_support::lock_test_env();
        let home = TempDir::new().unwrap();
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let tmp = TempDir::new().unwrap();
        write_workspace_profile(
            tmp.path(),
            "reviewer.toml",
            "id = \"reviewer\"\nmodel_class_hint = \"heavy\"\n",
        );
        write_workspace_profile(
            tmp.path(),
            "scout.toml",
            "id = \"scout\"\nrole_hint = \"scout\"\nprovider = \"deepseek\"\nmodel = \"deepseek-v4-flash\"\n",
        );

        // Isolate from ambient personal agent profiles on developer machines.
        let roster = FleetRoster::load_with_personal_dir(
            &FleetConfigToml::default(),
            tmp.path(),
            None,
            true,
        );

        let scout = roster.get("scout").expect("valid scout remains visible");
        assert_eq!(scout.origin, ProfileOrigin::Workspace);
        assert_eq!(scout.profile.provider.as_deref(), Some("deepseek"));
        assert_eq!(scout.profile.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(
            roster.get("reviewer").unwrap().origin,
            ProfileOrigin::BuiltIn,
            "invalid legacy override must fall back to the safe built-in"
        );
    }

    #[test]
    fn model_overrides_use_lowercased_ids_and_only_explicit_models() {
        let _env_lock = crate::test_support::lock_test_env();
        let home = TempDir::new().unwrap();
        let _codewhale_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        // Isolate personal `$CODEWHALE_HOME/agents` so ambient developer
        // profiles cannot pin built-ins like manager during unit tests.
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
        assert_eq!(ProfileOrigin::Personal.to_string(), "personal");
        assert_eq!(ProfileOrigin::Workspace.to_string(), "project");
    }
}

#[cfg(test)]
mod shadow_and_trust_tests {
    use super::*;
    use tempfile::TempDir;

    fn write_profile(dir: &Path, filename: &str, contents: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(filename), contents).unwrap();
    }

    #[test]
    fn workspace_shadow_of_personal_file_is_recorded_and_reported() {
        // #5098: editing the personal builder.toml changed nothing because a
        // project copy silently shadowed it. The roster must report that the
        // shadowed personal file exists and is ignored.
        let tmp = TempDir::new().unwrap();
        let personal_dir = tmp.path().join("personal");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        write_profile(
            &personal_dir,
            "builder.toml",
            "id = \"builder\"\nrole_hint = \"builder\"\nmodel = \"deepseek-v4-flash\"\n",
        );
        write_profile(
            &workspace.join(".codewhale").join("agents"),
            "builder.toml",
            "id = \"builder\"\nrole_hint = \"builder\"\nmodel = \"deepseek-v4-pro\"\n",
        );

        let roster = FleetRoster::load_with_personal_dir(
            &FleetConfigToml::default(),
            &workspace,
            Some(&personal_dir),
            true,
        );

        let builder = roster.get("builder").expect("builder member");
        assert_eq!(builder.origin, ProfileOrigin::Workspace);
        let shadows: Vec<_> = roster.shadowed_for("builder").collect();
        // The chain is built-in → personal → workspace; both displacements
        // are recorded, and the file-on-file one names the ignored personal
        // copy explicitly.
        assert_eq!(shadows.len(), 2, "full shadow chain: {shadows:?}");
        let shadow = shadows
            .iter()
            .find(|shadow| shadow.shadowed_origin == ProfileOrigin::Personal)
            .expect("personal file shadow is recorded");
        assert!(shadow.shadowed_source.ends_with("builder.toml"));
        assert_eq!(shadow.winner_origin, ProfileOrigin::Workspace);
        assert!(
            shadows
                .iter()
                .any(|shadow| shadow.shadowed_origin == ProfileOrigin::BuiltIn),
            "the built-in displacement is recorded too: {shadows:?}"
        );
        assert!(
            roster.shadowed().iter().any(|s| s.id == "builder"),
            "roster-level shadow log carries the record"
        );
    }

    #[test]
    fn project_scope_profiles_are_skipped_when_the_layer_is_not_trusted() {
        // #5098: `load_workspace_agent_profiles_tolerant` applied no trust
        // check — a cloned repo's .codewhale/agents/*.toml silently joined
        // the dispatch roster. With project config disabled
        // (`--no-project-config`), the whole layer stays out.
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        write_profile(
            &workspace.join(".codewhale").join("agents"),
            "builder.toml",
            "id = \"builder\"\nrole_hint = \"builder\"\nmodel = \"gpt-5.6-luna\"\n",
        );

        let gated = FleetRoster::load_with_personal_dir(
            &FleetConfigToml::default(),
            &workspace,
            None,
            false,
        );
        let builder = gated.get("builder").expect("built-in builder remains");
        assert_eq!(
            builder.origin,
            ProfileOrigin::BuiltIn,
            "untrusted project profile must not join the roster"
        );
        assert_ne!(
            builder.profile.model.as_deref(),
            Some("gpt-5.6-luna"),
            "foreign project pin must not reach dispatch"
        );

        let trusted = FleetRoster::load_with_personal_dir(
            &FleetConfigToml::default(),
            &workspace,
            None,
            true,
        );
        assert_eq!(
            trusted.get("builder").expect("builder").origin,
            ProfileOrigin::Workspace,
            "trusted project profile wins as before"
        );
    }
}
