//! Fleet-party visibility for the model-facing spawn surface.
//!
//! The "party" is the set of reusable agent profiles under
//! `.codewhale/agents/` (loaded by
//! [`crate::fleet::profile::load_workspace_agent_profiles`]). Party exists ⇒
//! advertise it (a compact roster on the `agent` tool description) and honor
//! `profile: "<id>"` on spawn calls, resolving role, ranked model
//! preference, and instruction overlay. Zero profiles ⇒ behavior unchanged.
//!
//! Profile permissions are deliberately NOT applied here: the fleet floor
//! stays. The profile loader itself rejects permission expansion
//! (`reject_permission_expansion`), and the sub-agent registry posture /
//! approval gate is unaffected by anything a profile declares.

use std::path::Path;

use codewhale_config::FleetLoadout;
use codewhale_protocol::fleet::FleetTaskWorkerProfile;

use super::{SpawnRequest, SubAgentModelStrength};
use crate::config::ApiProvider;
use crate::fleet::profile::{AgentProfile, load_workspace_agent_profiles};
use crate::fleet::worker_runtime::{fleet_role_to_agent_type, select_fleet_model};
use crate::tools::spec::ToolError;

/// Roster ceiling: keep the tool description compact under large parties.
const FLEET_ROSTER_MAX_MEMBERS: usize = 8;
/// Character bound for the per-spawn profile instruction overlay so a huge
/// profile file cannot balloon every child prompt.
const PROFILE_OVERLAY_MAX_CHARS: usize = 4_000;

/// Content fingerprint of the workspace party dir (`.codewhale/agents`).
///
/// Hashes the sorted `*.toml` file names plus their raw bytes, so profile
/// creation, edit, and removal all move the fingerprint while an unchanged
/// party stays stable. Long-lived registries (the sub-agent step loop; the
/// parent registry only lives one turn) compare this against the value
/// captured when the `agent` tool composed its roster and regenerate the
/// description ONLY on a change — an unchanged party must keep the serialized
/// tool catalog byte-identical for prefix-cache stability. A missing or empty
/// dir hashes to the same stable "no party" value.
pub(super) fn fleet_party_fingerprint(workspace: &Path) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let dir = workspace.join(crate::fleet::profile::WORKSPACE_AGENT_PROFILE_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        // No party dir ⇒ the stable no-party fingerprint (no writes).
        return hasher.finish();
    };
    let mut paths: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
        .collect();
    paths.sort();
    for path in paths {
        if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
            name.hash(&mut hasher);
        }
        match std::fs::read(&path) {
            Ok(bytes) => bytes.hash(&mut hasher),
            // Unreadable file: the name alone perturbs the fingerprint; the
            // loader independently degrades to "no party" on its own errors.
            Err(_) => 0u8.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Load the workspace party; loader errors degrade to "no party" with a
/// warning instead of failing the spawn surface.
pub(super) fn load_fleet_party(workspace: &Path) -> Vec<AgentProfile> {
    match load_workspace_agent_profiles(workspace) {
        Ok(profiles) => profiles,
        Err(err) => {
            tracing::warn!(
                target: "subagent",
                ?err,
                "failed to load workspace fleet party profiles"
            );
            Vec::new()
        }
    }
}

/// Compact roster advertised on the `agent` tool description when the party
/// is non-empty, e.g.:
/// `Fleet party (use profile:"<id>"): reviewer — role reviewer, class heavy,
/// model deepseek-v4-pro; scout — role scout, class fast`.
/// Bounded to [`FLEET_ROSTER_MAX_MEMBERS`] members plus a `+N more` marker.
pub(super) fn fleet_party_roster(party: &[AgentProfile]) -> Option<String> {
    if party.is_empty() {
        return None;
    }
    let mut entries = Vec::with_capacity(party.len().min(FLEET_ROSTER_MAX_MEMBERS));
    for profile in party.iter().take(FLEET_ROSTER_MAX_MEMBERS) {
        let mut entry = format!("{} — role {}", profile.id, profile.profile.role.name);
        if profile.profile.loadout != FleetLoadout::Inherit {
            entry.push_str(", class ");
            entry.push_str(profile.profile.loadout.as_str());
        }
        if let Some(model) = profile
            .profile
            .models
            .first()
            .map(String::as_str)
            .or(profile.profile.model.as_deref())
        {
            entry.push_str(", model ");
            entry.push_str(model);
        }
        entries.push(entry);
    }
    let mut roster = format!("Fleet party (use profile:\"<id>\"): {}", entries.join("; "));
    if party.len() > FLEET_ROSTER_MAX_MEMBERS {
        roster.push_str(&format!(
            "; +{} more",
            party.len() - FLEET_ROSTER_MAX_MEMBERS
        ));
    }
    Some(roster)
}

/// Outcome of applying a fleet profile to a spawn request.
#[derive(Debug)]
pub(super) struct FleetProfileApplication {
    pub profile_id: String,
    /// Degradation notice from ranked-model resolution (skipped pin /
    /// class-route fallback). Callers surface it as an `Event::Status`
    /// warning on the stream.
    pub notice: Option<String>,
}

/// Resolve `request.profile` against the party and fill the request in
/// place. Explicit fields on the call keep precedence:
/// - role/type: applied only when the call had no `type`/`role`;
/// - model: ranked profile pins resolve through [`select_fleet_model`] only
///   when the call pinned no `model`; a fully-degraded pin (`"auto"`) leaves
///   the model unset and, for a `fast` class, routes `model_strength` to
///   `faster` when the call left it open;
/// - instructions: appended to the child objective as a bounded overlay
///   (mirrors `fleet_task_prompt_with_profile`'s profile tail).
///
/// Unknown ids are a catchable invalid-input error listing available ids.
pub(super) fn apply_fleet_profile_to_spawn(
    request: &mut SpawnRequest,
    party: &[AgentProfile],
    run_model: &str,
    provider: ApiProvider,
) -> Result<Option<FleetProfileApplication>, ToolError> {
    let Some(profile_id) = request.profile.as_deref() else {
        return Ok(None);
    };
    let Some(profile) = party.iter().find(|profile| profile.id == profile_id) else {
        let available = if party.is_empty() {
            "none — the workspace has no .codewhale/agents profiles (run /fleet setup)".to_string()
        } else {
            party
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(ToolError::invalid_input(format!(
            "Unknown fleet profile '{profile_id}'. Available profiles: {available}"
        )));
    };

    // Role: profile fills only what the call left open.
    let role_name = profile.profile.role.name.trim();
    if !request.type_explicit && !role_name.is_empty() {
        request.agent_type = fleet_role_to_agent_type(Some(role_name));
        if request.assignment.role.is_none() {
            request.assignment.role = Some(role_name.to_string());
        }
    }

    // Model: ranked pins against the active provider; explicit call model wins.
    let mut notice = None;
    if request.model.is_none() {
        let profile_has_model_preference =
            !profile.profile.models.is_empty() || profile.profile.model.is_some();
        if profile_has_model_preference {
            let selection = select_fleet_model(
                run_model,
                Some(provider),
                None::<&FleetTaskWorkerProfile>,
                Some(profile),
            );
            notice = selection.notice;
            if !selection.model.eq_ignore_ascii_case("auto") {
                request.model = Some(selection.model);
            }
            // `"auto"` means every pin was unusable: fall through to the
            // class/loadout route instead of sending a known-bad id to the
            // wire.
        }
        // Class/loadout route: when no usable pin decided the model, a `fast`
        // profile routes to the faster sibling — the same route the fleet
        // worker runtime derives from `FleetLoadout::Fast` — whether the
        // profile had no pins at all or all pins degraded.
        if request.model.is_none()
            && !request.model_strength_explicit
            && profile.profile.loadout == FleetLoadout::Fast
        {
            request.model_strength = SubAgentModelStrength::Faster;
        }
    }

    // Instruction overlay on the child objective (bounded).
    if let Some(overlay) = profile_prompt_overlay(profile) {
        request.prompt.push_str(&overlay);
    }

    Ok(Some(FleetProfileApplication {
        profile_id: profile.id.clone(),
        notice,
    }))
}

/// Compose the profile tail appended to the child objective. Mirrors the
/// profile block of `fleet_task_prompt_with_profile` in
/// `crate::fleet::worker_runtime` (owned by the fleet lane, hence mirrored):
/// profile id (+ display name), description, then `[instructions].text`,
/// bounded to [`PROFILE_OVERLAY_MAX_CHARS`].
fn profile_prompt_overlay(profile: &AgentProfile) -> Option<String> {
    let mut overlay = String::new();
    overlay.push_str("\n\nFleet profile: ");
    overlay.push_str(&profile.id);
    if let Some(display_name) = profile.display_name.as_deref() {
        overlay.push_str(" (");
        overlay.push_str(display_name);
        overlay.push(')');
    }
    if let Some(description) = profile.description.as_deref() {
        overlay.push_str("\nProfile description:\n");
        overlay.push_str(description);
    }
    if let Some(instructions) = profile.profile.role.instructions.as_deref() {
        overlay.push_str("\nProfile instructions:\n");
        overlay.push_str(instructions);
    }
    if overlay.chars().count() > PROFILE_OVERLAY_MAX_CHARS {
        let bounded: String = overlay.chars().take(PROFILE_OVERLAY_MAX_CHARS).collect();
        overlay = format!("{bounded}\n… [profile overlay truncated]");
    }
    Some(overlay)
}

#[cfg(test)]
mod tests;
