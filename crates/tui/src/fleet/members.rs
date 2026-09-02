//! The fleet as a list of models (design MODEL-ROUTING-CATALOG §10, F1).
//!
//! A person builds their fleet by adding models from the providers they have
//! configured; the operator model later picks sub-agent routes from that list
//! only. The store is the selected Pod file (`fleet/store.rs`): its operator
//! route and every member that pins an exact `provider` + `model`. Nothing
//! here invents a second member store — a fleet model is a Pod member, and
//! the roles a model fills are the member rows that pin it.

use std::path::Path;

use super::store::{
    FleetFile, FleetMember, FleetScope, FleetStoreError, load_fleet_at, resolve_selected_fleet,
    save_fleet, set_selected, slugify,
};
use crate::tools::subagent::public_role_label;

/// Default name for the Pod created by the first `/pod add` or ⇧F on a row in
/// `/models` when no Pod is selected yet.
pub const DEFAULT_FLEET_NAME: &str = "My fleet";

/// One model in the fleet: an exact route plus the roles that pin it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetModel {
    /// Exact provider id (a `[providers.<id>]` key or a built-in id).
    pub provider: String,
    /// Exact model id on that provider's route.
    pub model: String,
    /// Roles whose member rows pin this route; `operator` for the Pod's own
    /// route. Empty when the model was added without a role.
    pub roles: Vec<String>,
    /// The Pod this model belongs to.
    pub fleet: String,
}

impl FleetModel {
    #[must_use]
    pub fn matches(&self, provider: &str, model: &str) -> bool {
        self.provider.eq_ignore_ascii_case(provider.trim())
            && self.model.eq_ignore_ascii_case(model.trim())
    }

    /// `roles` joined for a one-line label, or `member` when none.
    #[must_use]
    pub fn roles_label(&self) -> String {
        if self.roles.is_empty() {
            "member".to_string()
        } else {
            self.roles
                .iter()
                .map(|role| public_role_label(role))
                .collect::<Vec<_>>()
                .join(" · ")
        }
    }
}

/// What a membership change did, for the receipt line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetModelChange {
    Added {
        fleet: String,
        created_fleet: bool,
        roles: Vec<String>,
    },
    Removed {
        fleet: String,
        roles: Vec<String>,
    },
    /// Nothing was written: the route is the Pod's operator route (your
    /// current model while this Pod is selected), which `⇧F` and a
    /// role-less `/pod add` leave alone instead of duplicating.
    Unchanged {
        fleet: String,
        reason: String,
    },
}

/// The selected Pod's models, operator first, then members in file order.
/// Members that inherit the session route (no pin) are not models of their
/// own and are skipped. No selected Pod = an empty fleet; the caller states
/// "your fleet is the session model only".
#[must_use]
pub fn fleet_models(workspace: &Path) -> Vec<FleetModel> {
    let Ok(Some(selected)) = resolve_selected_fleet(workspace) else {
        return Vec::new();
    };
    let Ok((fleet, _scope)) = load_fleet_at(&selected.path) else {
        return Vec::new();
    };
    models_of(&fleet)
}

/// Project a Pod file into its models with roles unioned per exact route.
#[must_use]
pub fn models_of(fleet: &FleetFile) -> Vec<FleetModel> {
    let mut models: Vec<FleetModel> = Vec::new();
    let mut push = |provider: &str, model: &str, role: Option<&str>| {
        let role = role.map(str::trim).filter(|r| !r.is_empty());
        if let Some(existing) = models.iter_mut().find(|m| m.matches(provider, model)) {
            if let Some(role) = role
                && !existing.roles.iter().any(|r| r.eq_ignore_ascii_case(role))
            {
                existing.roles.push(role.to_string());
            }
            return;
        }
        models.push(FleetModel {
            provider: provider.trim().to_string(),
            model: model.trim().to_string(),
            roles: role.map(|r| vec![r.to_string()]).unwrap_or_default(),
            fleet: fleet.name.clone(),
        });
    };
    if let Some(operator) = fleet.operator.as_ref() {
        push(&operator.provider, &operator.model, Some("operator"));
    }
    for member in &fleet.members {
        let (Some(provider), Some(model)) = (member.provider.as_deref(), member.model.as_deref())
        else {
            continue;
        };
        // A member added without a role carries an id derived from the model
        // and an empty role; it is a model in the fleet, not a role.
        let role = (!member.role.trim().is_empty()).then_some(member.role.as_str());
        push(provider, model, role);
    }
    models
}

/// Add `provider/model` to the selected Pod (creating and selecting a
/// user-global `My fleet` when none is selected), one member row per role.
/// Adding a route that is already present with the same roles is a no-op.
pub fn add_fleet_model(
    workspace: &Path,
    provider: &str,
    model: &str,
    roles: &[String],
) -> Result<FleetModelChange, FleetStoreError> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return Err(FleetStoreError::Invalid(
            "a fleet model needs both a provider id and a model id".to_string(),
        ));
    }
    let (mut fleet, scope, created_fleet) = selected_or_new(workspace)?;
    if roles.iter().all(|r| r.trim().is_empty()) && is_operator_route(&fleet, provider, model) {
        return Ok(FleetModelChange::Unchanged {
            fleet: fleet.name,
            reason: OPERATOR_ROUTE_REASON.to_string(),
        });
    }
    let model_slug = slugify(model);
    let roles: Vec<String> = roles
        .iter()
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .collect();
    let wanted: Vec<Option<&str>> = if roles.is_empty() {
        vec![None]
    } else {
        roles.iter().map(|r| Some(r.as_str())).collect()
    };
    let mut added_any = false;
    for role in wanted {
        let already = fleet.members.iter().any(|m| {
            m.provider
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case(provider))
                && m.model
                    .as_deref()
                    .is_some_and(|id| id.eq_ignore_ascii_case(model))
                && role.is_none_or(|role| m.role.eq_ignore_ascii_case(role))
        });
        if already {
            continue;
        }
        let base = role.map_or_else(|| model_slug.clone(), slugify);
        let id = unique_member_id(&fleet, &base, &model_slug);
        fleet.members.push(FleetMember {
            id,
            display_name: None,
            role: role.unwrap_or_default().to_string(),
            model: Some(model.to_string()),
            provider: Some(provider.to_string()),
            reasoning: None,
            instructions: None,
            requires: Vec::new(),
        });
        added_any = true;
    }
    if !added_any {
        return Ok(FleetModelChange::Unchanged {
            fleet: fleet.name,
            reason: "already in the fleet".to_string(),
        });
    }
    save_fleet(&fleet, scope, workspace)?;
    Ok(FleetModelChange::Added {
        fleet: fleet.name,
        created_fleet,
        roles,
    })
}

/// Idempotently enroll a route the user selected or used (`/model`, config,
/// a subagent run). Never prompts and never fails the caller: a store error is
/// logged and swallowed. `auto` routes and blank ids are skipped.
pub fn auto_enroll_fleet_model(workspace: &Path, provider: &str, model: &str) {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty()
        || model.is_empty()
        || provider.eq_ignore_ascii_case("auto")
        || model.eq_ignore_ascii_case("auto")
    {
        return;
    }
    if let Err(error) = add_fleet_model(workspace, provider, model, &[]) {
        tracing::debug!(
            provider,
            model,
            error = %error,
            "could not auto-enroll model in the fleet"
        );
    }
}

/// Remove every member row pinning `provider/model` from the selected Pod.
/// The operator route is not a member; it is changed with `/pod save`.
pub fn remove_fleet_model(
    workspace: &Path,
    provider: &str,
    model: &str,
) -> Result<FleetModelChange, FleetStoreError> {
    let provider = provider.trim();
    let model = model.trim();
    let Some(selected) = resolve_selected_fleet(workspace)? else {
        return Err(FleetStoreError::Invalid(
            "no Pod is selected; your fleet is the session model only".to_string(),
        ));
    };
    let (mut fleet, scope) = load_fleet_at(&selected.path)?;
    let before = fleet.members.len();
    let mut roles = Vec::new();
    fleet.members.retain(|m| {
        let hit = m
            .provider
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(provider))
            && m.model
                .as_deref()
                .is_some_and(|id| id.eq_ignore_ascii_case(model));
        if hit && !m.role.trim().is_empty() {
            roles.push(m.role.clone());
        }
        !hit
    });
    if fleet.members.len() == before {
        let is_operator = fleet.operator.as_ref().is_some_and(|op| {
            op.provider.eq_ignore_ascii_case(provider) && op.model.eq_ignore_ascii_case(model)
        });
        return Err(FleetStoreError::Invalid(if is_operator {
            format!(
                "{provider}/{model} is the Pod's operator route; change it with /pod save, not remove"
            )
        } else {
            format!("{provider}/{model} is not in the fleet `{}`", fleet.name)
        }));
    }
    save_fleet(&fleet, scope, workspace)?;
    Ok(FleetModelChange::Removed {
        fleet: fleet.name,
        roles,
    })
}

/// Add when absent, remove when present — the picker's one-key toggle.
pub fn toggle_fleet_model(
    workspace: &Path,
    provider: &str,
    model: &str,
) -> Result<FleetModelChange, FleetStoreError> {
    let models = fleet_models(workspace);
    let Some(present) = models.iter().find(|m| m.matches(provider, model)) else {
        return add_fleet_model(workspace, provider, model, &[]);
    };
    // Present by exact route, whatever roles it fills: never write a
    // duplicate row. The operator route with no member rows of its own is
    // your current model; it cannot be toggled off, only switched.
    let only_operator = present.roles.iter().all(|r| r == "operator");
    if only_operator {
        return Ok(FleetModelChange::Unchanged {
            fleet: present.fleet.clone(),
            reason: OPERATOR_ROUTE_REASON.to_string(),
        });
    }
    remove_fleet_model(workspace, provider, model)
}

const OPERATOR_ROUTE_REASON: &str = "this is your current model (the Pod's operator route); switch models or use /pod save to change it";

fn is_operator_route(fleet: &FleetFile, provider: &str, model: &str) -> bool {
    fleet.operator.as_ref().is_some_and(|op| {
        op.provider.eq_ignore_ascii_case(provider.trim())
            && op.model.eq_ignore_ascii_case(model.trim())
    })
}

/// One-line receipt for a membership change, shared by `/pod add|remove`
/// and the picker's `⇧F`.
#[must_use]
pub fn change_receipt(provider: &str, model: &str, change: &FleetModelChange) -> String {
    match change {
        FleetModelChange::Added {
            fleet,
            created_fleet,
            roles,
        } => {
            let roles = if roles.is_empty() {
                String::new()
            } else {
                format!(" as {}", roles.join(", "))
            };
            let created = if *created_fleet {
                " (new user-global fleet, now selected)"
            } else {
                ""
            };
            format!("Added {provider}/{model}{roles} to the fleet `{fleet}`{created}")
        }
        FleetModelChange::Removed { fleet, roles } => {
            let roles = if roles.is_empty() {
                String::new()
            } else {
                format!(" ({})", roles.join(", "))
            };
            format!("Removed {provider}/{model}{roles} from the fleet `{fleet}`")
        }
        FleetModelChange::Unchanged { fleet, reason } => {
            format!("{provider}/{model} stays in the fleet `{fleet}`: {reason}")
        }
    }
}

fn selected_or_new(workspace: &Path) -> Result<(FleetFile, FleetScope, bool), FleetStoreError> {
    if let Some(selected) = resolve_selected_fleet(workspace)? {
        let (fleet, scope) = load_fleet_at(&selected.path)?;
        return Ok((fleet, scope, false));
    }
    let fleet = FleetFile::new(
        DEFAULT_FLEET_NAME.to_string(),
        Some("Models added from /models and /pod add.".to_string()),
    )?;
    save_fleet(&fleet, FleetScope::Personal, workspace)?;
    set_selected(DEFAULT_FLEET_NAME, FleetScope::Personal, workspace)?;
    Ok((fleet, FleetScope::Personal, true))
}

fn unique_member_id(fleet: &FleetFile, base: &str, model_slug: &str) -> String {
    let taken = |id: &str| fleet.members.iter().any(|m| m.id.eq_ignore_ascii_case(id));
    if !taken(base) {
        return base.to_string();
    }
    let with_model = format!("{base}-{model_slug}");
    if !taken(&with_model) {
        return with_model;
    }
    (2..)
        .map(|n| format!("{with_model}-{n}"))
        .find(|candidate| !taken(candidate))
        .expect("an unbounded counter yields a free id")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::store::FleetOperator;
    use crate::fleet::store::{save_fleet, set_selected};

    fn fleet_with(operator: Option<(&str, &str)>, members: &[(&str, &str, &str)]) -> FleetFile {
        let mut fleet = FleetFile::new("Test".to_string(), None).expect("valid");
        fleet.operator = operator.map(|(p, m)| FleetOperator {
            provider: p.to_string(),
            model: m.to_string(),
            reasoning: None,
        });
        for (id, role, model) in members {
            fleet.members.push(FleetMember {
                id: (*id).to_string(),
                display_name: None,
                role: (*role).to_string(),
                model: Some((*model).to_string()),
                provider: Some("openrouter".to_string()),
                reasoning: None,
                instructions: None,
                requires: Vec::new(),
            });
        }
        fleet
    }

    #[test]
    fn models_of_unions_roles_per_exact_route_and_puts_the_operator_first() {
        let fleet = fleet_with(
            Some(("openrouter", "z-ai/glm-5.3")),
            &[
                ("scout", "scout", "z-ai/glm-5.3-flash"),
                ("reviewer", "reviewer", "deepseek/deepseek-v4-flash"),
                ("verifier", "verifier", "z-ai/glm-5.3-flash"),
                ("planner", "planner", "z-ai/glm-5.3"),
            ],
        );
        let models = models_of(&fleet);
        let ids: Vec<_> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(
            ids,
            [
                "z-ai/glm-5.3",
                "z-ai/glm-5.3-flash",
                "deepseek/deepseek-v4-flash"
            ]
        );
        assert_eq!(models[0].roles, ["operator", "planner"]);
        assert_eq!(models[1].roles, ["scout", "verifier"]);
        assert_eq!(models[1].roles_label(), "explore · test");
    }

    #[test]
    fn inheriting_members_are_not_models_of_their_own() {
        let mut fleet = fleet_with(None, &[]);
        fleet.members.push(FleetMember {
            id: "builder".to_string(),
            display_name: None,
            role: "builder".to_string(),
            model: None,
            provider: None,
            reasoning: None,
            instructions: None,
            requires: Vec::new(),
        });
        assert!(models_of(&fleet).is_empty());
    }

    #[test]
    fn add_creates_and_selects_a_default_fleet_then_toggle_removes() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");

        assert!(fleet_models(&workspace).is_empty());
        let change =
            add_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash", &[]).expect("add");
        assert_eq!(
            change,
            FleetModelChange::Added {
                fleet: DEFAULT_FLEET_NAME.to_string(),
                created_fleet: true,
                roles: Vec::new(),
            }
        );
        let models = fleet_models(&workspace);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].model, "z-ai/glm-5.3-flash");
        assert!(models[0].roles.is_empty());

        // A second add with a role attaches the role instead of duplicating.
        add_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3-flash",
            &["scout".to_string()],
        )
        .expect("add role");
        let models = fleet_models(&workspace);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].roles, ["scout"]);

        let change =
            toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash").expect("toggle");
        assert!(
            matches!(change, FleetModelChange::Removed { ref roles, .. } if roles == &["scout"])
        );
        assert!(fleet_models(&workspace).is_empty());

        let err = remove_fleet_model(&workspace, "openrouter", "nope").expect_err("absent");
        assert!(err.to_string().contains("not in the fleet"), "{err}");
    }

    #[test]
    fn auto_enroll_is_idempotent_and_skips_auto_routes() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");

        auto_enroll_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash");
        auto_enroll_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash");
        assert_eq!(fleet_models(&workspace).len(), 1);

        remove_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash").expect("remove");
        auto_enroll_fleet_model(&workspace, "auto", "auto");
        assert!(fleet_models(&workspace).is_empty());
    }

    #[test]
    fn toggling_the_operator_route_is_a_no_op_and_never_writes_a_duplicate_row() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut fleet = fleet_with(Some(("openrouter", "z-ai/glm-5.3")), &[]);
        fleet.name = "Ops".to_string();
        save_fleet(&fleet, FleetScope::Personal, &workspace).expect("save");
        set_selected("Ops", FleetScope::Personal, &workspace).expect("select");

        let change = toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3").expect("toggle");
        assert!(
            matches!(change, FleetModelChange::Unchanged { ref fleet, ref reason } if fleet == "Ops" && reason.contains("current model")),
            "{change:?}"
        );
        assert!(
            change_receipt("openrouter", "z-ai/glm-5.3", &change).contains("stays in the fleet"),
        );
        // A role-less /pod add of the same route is the same no-op.
        let change = add_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3", &[]).expect("add");
        assert!(
            matches!(change, FleetModelChange::Unchanged { .. }),
            "{change:?}"
        );
        let selected = resolve_selected_fleet(&workspace)
            .expect("ok")
            .expect("selected");
        let (on_disk, _) = load_fleet_at(&selected.path).expect("load");
        assert!(
            on_disk.members.is_empty(),
            "no duplicate member row: {:?}",
            on_disk.members
        );

        // With a role, the operator route may also fill that role; toggling
        // then removes the role rows and keeps the operator.
        add_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3",
            &["planner".to_string()],
        )
        .expect("add role");
        let models = fleet_models(&workspace);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].roles, ["operator", "planner"]);
        let change = toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3").expect("toggle");
        assert!(
            matches!(change, FleetModelChange::Removed { ref roles, .. } if roles == &["planner"]),
            "{change:?}"
        );
        assert_eq!(fleet_models(&workspace)[0].roles, ["operator"]);
    }

    #[test]
    fn member_ids_stay_unique_when_a_role_is_reused_on_two_models() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        add_fleet_model(&workspace, "openrouter", "a/one", &["scout".to_string()]).expect("one");
        add_fleet_model(&workspace, "openrouter", "a/two", &["scout".to_string()]).expect("two");
        let selected = resolve_selected_fleet(&workspace)
            .expect("ok")
            .expect("selected");
        let (fleet, _) = load_fleet_at(&selected.path).expect("load");
        let ids: Vec<_> = fleet.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["scout", "scout-atwo"]);
    }
}
