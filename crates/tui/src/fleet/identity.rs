//! Effective Fleet roster loading and deterministic member identity.
//!
//! A selected v2 Fleet is the runtime source of truth. Legacy profile layers
//! are consulted only when no Fleet is selected. The selector resolver feeds
//! durable Fleet task dispatch; in-process agent spawns resolve roles only
//! and never consult the roster.

use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use codewhale_config::{
    FleetConfigToml, FleetDelegationHints, FleetLoadout, FleetProfile, FleetProfilePermissions,
    FleetRole, FleetSlot,
};

use super::profile::AgentProfile;
use super::role::public_role_label;
use super::roster::{FleetRoster, ProfileOrigin};
use super::store::{
    FleetFile, FleetScope, MemberCapability, load_fleet_at, resolve_selected_fleet,
};

const MAX_IDENTITY_FIELD_CHARS: usize = 160;

/// Load the one roster the session must display and dispatch against.
///
/// A broken explicit selection becomes a failed roster. It must never be
/// indistinguishable from "no selection", which is the only state allowed to
/// fall back to the legacy profile merge.
#[must_use]
pub fn load_effective_roster(
    fleet_config: &FleetConfigToml,
    workspace: &Path,
    plugins: Option<&crate::plugins::PluginRegistry>,
) -> FleetRoster {
    let selected = match resolve_selected_fleet(workspace) {
        Ok(selected) => selected,
        Err(_) => {
            return FleetRoster::failed(
                "Selected Fleet is missing or unreadable; inspect /fleet and repair or clear the selection.",
            );
        }
    };
    let Some(selected) = selected else {
        return plugins.map_or_else(
            || FleetRoster::load(fleet_config, workspace),
            |plugins| FleetRoster::load_with_plugins(fleet_config, workspace, plugins),
        );
    };
    let (fleet, _) = match load_fleet_at(&selected.path) {
        Ok(loaded) => loaded,
        Err(_) => {
            let name = bounded_fleet_label(&selected.name);
            return FleetRoster::failed(format!(
                "Selected {} Fleet `{name}` is invalid or unreadable; inspect /fleet and repair or clear the selection.",
                selected.scope.label()
            ));
        }
    };
    if fleet.operator.is_none()
        && fleet
            .members
            .iter()
            .all(|member| member.role.trim().is_empty())
    {
        return plugins.map_or_else(
            || FleetRoster::load(fleet_config, workspace),
            |plugins| FleetRoster::load_with_plugins(fleet_config, workspace, plugins),
        );
    }
    roster_from_fleet(&fleet, selected.scope, &selected.path)
}

/// Project a validated v2 Fleet into the existing Agent-profile runtime.
#[must_use]
pub fn roster_from_fleet(fleet: &FleetFile, scope: FleetScope, source: &Path) -> FleetRoster {
    let origin = match scope {
        FleetScope::Personal => ProfileOrigin::Personal,
        FleetScope::Workspace => ProfileOrigin::Workspace,
    };
    let operator = fleet.operator.as_ref();
    FleetRoster::from_members(
        fleet
            .members
            .iter()
            .map(|member| {
                let role = member.role.trim();
                let role = if role.is_empty() {
                    member.id.trim().to_string()
                } else {
                    public_role_label(role)
                };
                // A selected Fleet is one atomic routing policy. An explicit
                // member pair wins; otherwise the Fleet operator pair is the
                // child fallback, and only an operator-less Fleet inherits
                // the current session route. Keep provider/model together so
                // neither half can leak across providers.
                let (provider, model) = match (&member.provider, &member.model) {
                    (Some(provider), Some(model)) => (Some(provider.clone()), Some(model.clone())),
                    (None, None) => operator.map_or((None, None), |operator| {
                        (
                            Some(operator.provider.clone()),
                            Some(operator.model.clone()),
                        )
                    }),
                    // FleetFile::validate rejects partial member pins before
                    // this projection is reachable. Keep the fallback atomic
                    // if an in-process caller supplies an unchecked document.
                    _ => (None, None),
                };
                let reasoning_effort = member
                    .reasoning
                    .clone()
                    .or_else(|| operator.and_then(|operator| operator.reasoning.clone()));
                AgentProfile {
                    id: member.id.trim().to_string(),
                    display_name: member.display_name.clone(),
                    description: None,
                    requires: member.requires.clone(),
                    profile: FleetProfile {
                        slot: FleetSlot::from_name(&role),
                        role: FleetRole {
                            name: role,
                            description: None,
                            instructions: member.instructions.clone(),
                        },
                        loadout: FleetLoadout::Inherit,
                        model,
                        provider,
                        reasoning_effort,
                        permissions: FleetProfilePermissions::default(),
                        delegation: FleetDelegationHints::default(),
                    },
                    source: source.to_path_buf(),
                    origin,
                    plugin_authority: None,
                }
            })
            .collect(),
    )
}

/// Bounded identity row returned by `agent action=roster`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FleetMemberIdentity {
    pub member_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    pub route: String,
    pub origin: String,
}

impl FleetMemberIdentity {
    #[must_use]
    pub fn from_member(member: &AgentProfile) -> Self {
        let provider_id = trimmed_owned(member.profile.provider.as_deref())
            .map(|value| bounded_identity_field(&value));
        let model_id = trimmed_owned(member.profile.model.as_deref())
            .map(|value| bounded_identity_field(&value));
        let route = match (&provider_id, &model_id) {
            (Some(provider), Some(model)) => format!("{provider}/{model}"),
            (None, Some(model)) => model.clone(),
            _ => "inherit".to_string(),
        };
        Self {
            member_id: bounded_identity_field(&member.id),
            display_name: trimmed_owned(member.display_name.as_deref())
                .map(|value| bounded_identity_field(&value)),
            role: bounded_identity_field(&public_role_label(member_role(member))),
            provider_id,
            model_name: friendly_model_name(member).map(|value| bounded_identity_field(&value)),
            model_id,
            requires: member
                .requires
                .iter()
                .take(MemberCapability::VOCABULARY.len())
                .map(|value| bounded_identity_field(value))
                .collect(),
            route,
            origin: member.origin.to_string(),
        }
    }
}

pub(crate) fn bounded_identity_field(value: &str) -> String {
    bounded_visible_text(value, MAX_IDENTITY_FIELD_CHARS)
}

fn bounded_fleet_label(value: &str) -> String {
    crate::safe_label::SafeLabel::phrase(value).to_string()
}

fn bounded_visible_text(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    let mut pending_space = false;
    let mut output_chars = 0usize;
    for ch in value.trim().chars() {
        if ch.is_control() || ch.is_whitespace() {
            pending_space = !output.is_empty();
            continue;
        }
        if pending_space && output_chars < max_chars {
            output.push(' ');
            output_chars += 1;
        }
        pending_space = false;
        if output_chars >= max_chars {
            break;
        }
        output.push(ch);
        output_chars += 1;
    }
    output
}

fn trimmed_owned(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn member_role(member: &AgentProfile) -> &str {
    let role = member.profile.role.name.trim();
    if role.is_empty() {
        member.id.trim()
    } else {
        role
    }
}

/// Offline human label for a member's explicit model pin.
#[must_use]
pub fn friendly_model_name(member: &AgentProfile) -> Option<String> {
    let model = member.profile.model.as_deref()?.trim();
    if model.is_empty() {
        return None;
    }
    let catalog = codewhale_config::catalog::bundled_models_dev_catalog();
    if let Some(provider) = member
        .profile
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        && let Some(offering) = catalog.provider_model(provider, model)
    {
        if let Some(name) = trimmed_owned(offering.name.as_deref()) {
            return Some(name);
        }
        if let Some(base_model) = offering.base_model.as_deref()
            && let Some(name) = catalog
                .model(base_model)
                .and_then(|model| trimmed_owned(model.name.as_deref()))
        {
            return Some(name);
        }
    }
    catalog
        .model(model)
        .and_then(|model| trimmed_owned(model.name.as_deref()))
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetSelectorError {
    #[error("Fleet member selector cannot be blank")]
    Blank,
    #[error(
        "Fleet member selector `{selector}` is ambiguous; choose one member explicitly: {candidates}"
    )]
    Ambiguous {
        selector: String,
        candidates: String,
    },
}

/// Resolve a member selector deterministically against an already-loaded
/// profile slice.
///
/// Unqualified exact ids win for compatibility. Every other identity class is
/// resolved as a set and succeeds only when it names one distinct member.
/// Fleet task dispatch uses this entry point.
pub fn resolve_member_in_profiles<'a>(
    profiles: &'a [AgentProfile],
    selector: &str,
) -> Result<Option<&'a AgentProfile>, FleetSelectorError> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err(FleetSelectorError::Blank);
    }
    let (kind, value) = selector
        .split_once(':')
        .filter(|(kind, value)| is_selector_kind(kind) && !value.trim().is_empty())
        .map_or((None, selector), |(kind, value)| {
            (Some(kind.to_ascii_lowercase()), value.trim())
        });

    if kind
        .as_deref()
        .is_none_or(|kind| kind == "member" || kind == "id")
        && let Some(member) = profiles
            .iter()
            .find(|member| member.id.eq_ignore_ascii_case(value))
    {
        return Ok(Some(member));
    }

    if kind
        .as_deref()
        .is_none_or(|kind| kind == "member" || kind == "id")
        && let Some(member) = profiles.iter().find(|member| {
            public_role_label(&member.id).eq_ignore_ascii_case(&public_role_label(value))
        })
    {
        return Ok(Some(member));
    }

    let mut candidates = Vec::new();
    for member in profiles {
        let matches = match kind.as_deref() {
            Some("member" | "id") => false,
            Some("name") => matches_display_name(member, value),
            Some("role") => public_role_label(member_role(member))
                .eq_ignore_ascii_case(&public_role_label(value)),
            Some("model") => matches_model(member, value),
            Some("route") => matches_route(member, value),
            Some(_) => false,
            None => {
                matches_display_name(member, value)
                    || public_role_label(member_role(member))
                        .eq_ignore_ascii_case(&public_role_label(value))
                    || matches_model(member, value)
                    || matches_route(member, value)
                    || friendly_model_name(member)
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(value))
            }
        };
        if matches {
            push_unique(&mut candidates, member);
        }
    }

    if candidates.is_empty() && kind.is_none() {
        let alias = match value.to_ascii_lowercase().as_str() {
            "implementer" | "implement" | "implementation" => Some("builder"),
            "release_lead" | "release-lead" | "releaselead" => Some("manager"),
            "explore" | "explorer" | "exploration" => Some("scout"),
            "general" | "default" => Some("worker"),
            _ => None,
        };
        if let Some(member) = alias.and_then(|alias| {
            profiles
                .iter()
                .find(|member| member.id.eq_ignore_ascii_case(alias))
        }) {
            candidates.push(member);
        }
    }

    match candidates.as_slice() {
        [] => Ok(None),
        [member] => Ok(Some(*member)),
        _ => Err(FleetSelectorError::Ambiguous {
            selector: selector.to_string(),
            candidates: candidates
                .iter()
                .take(8)
                .map(|member| FleetMemberIdentity::from_member(member))
                .map(|member| {
                    format!(
                        "{} (role {}, route {})",
                        member.member_id, member.role, member.route
                    )
                })
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

fn is_selector_kind(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "member" | "id" | "name" | "role" | "model" | "route"
    )
}

fn matches_display_name(member: &AgentProfile, value: &str) -> bool {
    member
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .is_some_and(|name| name.eq_ignore_ascii_case(value))
}

fn matches_model(member: &AgentProfile, value: &str) -> bool {
    member
        .profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .is_some_and(|model| model.eq_ignore_ascii_case(value))
        || friendly_model_name(member)
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(value))
}

fn matches_route(member: &AgentProfile, value: &str) -> bool {
    let Some((provider, model)) = value.split_once('/') else {
        return false;
    };
    member
        .profile
        .provider
        .as_deref()
        .map(str::trim)
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(provider.trim()))
        && member
            .profile
            .model
            .as_deref()
            .map(str::trim)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(model.trim()))
}

fn push_unique<'a>(members: &mut Vec<&'a AgentProfile>, member: &'a AgentProfile) {
    if !members
        .iter()
        .any(|existing| existing.id.eq_ignore_ascii_case(&member.id))
    {
        members.push(member);
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::{FLEET_SCHEMA_KIND, FLEET_SCHEMA_REVISION, FleetMember};
    use super::*;
    use std::path::PathBuf;

    fn member(
        id: &str,
        display_name: Option<&str>,
        role: &str,
        provider: Option<&str>,
        model: Option<&str>,
    ) -> AgentProfile {
        AgentProfile {
            id: id.to_string(),
            display_name: display_name.map(str::to_string),
            description: None,
            requires: Vec::new(),
            profile: FleetProfile {
                slot: FleetSlot::from_name(role),
                role: FleetRole {
                    name: role.to_string(),
                    description: None,
                    instructions: None,
                },
                loadout: FleetLoadout::Inherit,
                model: model.map(str::to_string),
                provider: provider.map(str::to_string),
                reasoning_effort: None,
                permissions: FleetProfilePermissions::default(),
                delegation: FleetDelegationHints::default(),
            },
            source: PathBuf::from("test"),
            origin: ProfileOrigin::Workspace,
            plugin_authority: None,
        }
    }

    fn stored_member(id: &str, display_name: Option<&str>, role: &str) -> FleetMember {
        FleetMember {
            id: id.to_string(),
            display_name: display_name.map(str::to_string),
            role: role.to_string(),
            model: None,
            provider: None,
            reasoning: None,
            instructions: None,
            requires: Vec::new(),
        }
    }

    #[test]
    fn selected_v2_members_project_exact_identity_and_route() {
        let fleet = FleetFile {
            schema: FLEET_SCHEMA_KIND.to_string(),
            schema_revision: FLEET_SCHEMA_REVISION,
            name: "Launch".to_string(),
            description: None,
            operator: None,
            members: vec![FleetMember {
                id: "Scout-One".to_string(),
                display_name: Some("Flash Scout".to_string()),
                role: "scout".to_string(),
                model: Some("deepseek-v4-flash".to_string()),
                provider: Some("deepseek".to_string()),
                reasoning: Some("low".to_string()),
                instructions: Some("Inspect only.".to_string()),
                requires: vec!["vision".to_string()],
            }],
        };
        let roster = roster_from_fleet(
            &fleet,
            FleetScope::Workspace,
            Path::new(".codewhale/fleets/launch.toml"),
        );
        let projected = roster.get("scout-one").expect("case-insensitive id");
        assert_eq!(projected.display_name.as_deref(), Some("Flash Scout"));
        assert_eq!(projected.profile.role.name, "explore");
        assert_eq!(projected.profile.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            projected.profile.model.as_deref(),
            Some("deepseek-v4-flash")
        );
        assert_eq!(projected.profile.reasoning_effort.as_deref(), Some("low"));
        assert_eq!(
            projected.profile.role.instructions.as_deref(),
            Some("Inspect only.")
        );
        assert_eq!(projected.requires, vec!["vision".to_string()]);
    }

    #[test]
    fn selected_v2_member_route_precedence_is_member_then_operator_then_session() {
        let fleet = FleetFile {
            schema: FLEET_SCHEMA_KIND.to_string(),
            schema_revision: FLEET_SCHEMA_REVISION,
            name: "Launch".to_string(),
            description: None,
            operator: Some(super::super::store::FleetOperator {
                provider: "deepseek".to_string(),
                model: "deepseek-v4-flash".to_string(),
                reasoning: Some("medium".to_string()),
            }),
            members: vec![
                FleetMember {
                    id: "inherited".to_string(),
                    display_name: None,
                    role: "scout".to_string(),
                    model: None,
                    provider: None,
                    reasoning: None,
                    instructions: None,
                    requires: Vec::new(),
                },
                FleetMember {
                    id: "pinned".to_string(),
                    display_name: None,
                    role: "reviewer".to_string(),
                    model: Some("gpt-5.6".to_string()),
                    provider: Some("openrouter".to_string()),
                    reasoning: Some("high".to_string()),
                    instructions: None,
                    requires: Vec::new(),
                },
            ],
        };
        let roster = roster_from_fleet(
            &fleet,
            FleetScope::Personal,
            Path::new("fleets/launch.toml"),
        );

        let inherited = roster.get("inherited").expect("inherited member");
        assert_eq!(inherited.profile.provider.as_deref(), Some("deepseek"));
        assert_eq!(
            inherited.profile.model.as_deref(),
            Some("deepseek-v4-flash")
        );
        assert_eq!(
            inherited.profile.reasoning_effort.as_deref(),
            Some("medium")
        );

        let pinned = roster.get("pinned").expect("pinned member");
        assert_eq!(pinned.profile.provider.as_deref(), Some("openrouter"));
        assert_eq!(pinned.profile.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(pinned.profile.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn selector_accepts_id_display_role_model_name_and_route() {
        let roster = FleetRoster::from_members(vec![member(
            "flash-scout",
            Some("Scout One"),
            "scout",
            Some("deepseek"),
            Some("deepseek-v4-flash"),
        )]);
        for selector in [
            "FLASH-SCOUT",
            "Scout One",
            "role:scout",
            "deepseek-v4-flash",
            "DeepSeek V4 Flash",
            "route:deepseek/deepseek-v4-flash",
        ] {
            assert_eq!(
                resolve_member_in_profiles(roster.members(), selector)
                    .expect("valid selector")
                    .map(|member| member.id.as_str()),
                Some("flash-scout"),
                "selector {selector}"
            );
        }
    }

    #[test]
    fn canonical_role_selectors_resolve_legacy_builtin_member_ids() {
        let roster = FleetRoster::built_ins_only();

        assert_eq!(
            resolve_member_in_profiles(roster.members(), "explore")
                .expect("canonical role selector")
                .map(|member| member.id.as_str()),
            Some("scout")
        );
        assert_eq!(
            resolve_member_in_profiles(roster.members(), "advisor")
                .expect("canonical role selector")
                .map(|member| member.id.as_str()),
            Some("consultant")
        );
    }

    #[test]
    fn selected_v2_friendly_name_selector_is_deterministic() {
        let fleet = FleetFile {
            schema: FLEET_SCHEMA_KIND.to_string(),
            schema_revision: FLEET_SCHEMA_REVISION,
            name: "Named members".to_string(),
            description: None,
            operator: None,
            members: vec![
                stored_member("release-lead", Some("Release Lead"), "manager"),
                stored_member("scout-a", Some("Flash Scout"), "scout"),
                stored_member("scout-b", Some("Flash Scout"), "reviewer"),
            ],
        };
        let roster = roster_from_fleet(
            &fleet,
            FleetScope::Workspace,
            Path::new(".codewhale/fleets/named-members.toml"),
        );

        for selector in ["Release Lead", "name:Release Lead"] {
            assert_eq!(
                resolve_member_in_profiles(roster.members(), selector)
                    .expect("unique friendly name")
                    .map(|member| member.id.as_str()),
                Some("release-lead"),
                "selector {selector}"
            );
        }
        let error = resolve_member_in_profiles(roster.members(), "name:Flash Scout")
            .expect_err("duplicate friendly name must be ambiguous");
        let FleetSelectorError::Ambiguous { candidates, .. } = error else {
            panic!("expected ambiguity");
        };
        assert!(candidates.contains("scout-a"), "{candidates}");
        assert!(candidates.contains("scout-b"), "{candidates}");

        assert_eq!(
            roster.members()[0].display_name.as_deref(),
            Some("Release Lead")
        );
    }

    #[test]
    fn duplicate_model_label_is_ambiguous_and_member_id_still_wins() {
        let roster = FleetRoster::from_members(vec![
            member(
                "scout-a",
                None,
                "scout",
                Some("deepseek"),
                Some("deepseek-v4-flash"),
            ),
            member(
                "scout-b",
                None,
                "reviewer",
                Some("deepseek"),
                Some("deepseek-v4-flash"),
            ),
        ]);
        assert!(matches!(
            resolve_member_in_profiles(roster.members(), "DeepSeek V4 Flash"),
            Err(FleetSelectorError::Ambiguous { .. })
        ));
        assert_eq!(
            resolve_member_in_profiles(roster.members(), "SCOUT-A")
                .expect("id")
                .map(|member| member.id.as_str()),
            Some("scout-a")
        );
    }

    #[test]
    fn invalid_selected_fleet_becomes_visible_failure_without_legacy_members() {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let fleets = workspace.path().join(".codewhale/fleets");
        std::fs::create_dir_all(&fleets).unwrap();
        std::fs::write(fleets.join("selected"), "Broken\n").unwrap();
        std::fs::write(
            fleets.join("broken.toml"),
            "schema = \"fleet\"\nschema_revision = 2\nname = \"Broken\"\n[[members]]\nid = \"scout\"\nprovider = \"deepseek\"\n",
        )
        .unwrap();

        let roster = load_effective_roster(&FleetConfigToml::default(), workspace.path(), None);
        assert!(roster.members().is_empty());
        let error = roster.load_error().expect("visible selected-Fleet error");
        assert!(error.contains("Selected folder Fleet `Broken`"), "{error}");
        assert!(!error.contains(&workspace.path().display().to_string()));
        assert!(!error.contains("must pin both provider and model"));
        assert!(!error.contains("provider ="));
        assert!(error.chars().count() <= 200, "{error}");
    }

    #[test]
    fn selected_fleet_error_label_redacts_opaque_or_quoted_names() {
        let raw = "sk-live-abcdef0123456789abcdef 'quoted'\n/Users/operator/private";
        let label = bounded_fleet_label(raw);
        assert!(label.starts_with("sha256:"), "{label}");
        assert!(!label.contains("sk-live"));
        assert!(!label.contains("quoted"));
        assert!(!label.contains("/Users"));
        assert!(!label.contains('\n'));
        assert!(label.chars().count() <= 32, "{label}");
    }

    #[test]
    fn member_identity_fields_stay_bounded() {
        let live = member("member-zero", None, "worker", None, None);
        let identity = FleetMemberIdentity::from_member(&live);
        assert_eq!(identity.member_id, "member-zero");
        assert_eq!(identity.route, "inherit");
    }
}
