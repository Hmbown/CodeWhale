//! The fleet as a list of models (design MODEL-ROUTING-CATALOG §10, F1).
//!
//! A person builds their fleet by adding models from the providers they have
//! configured; the operator model later picks sub-agent routes from that list
//! only. The store is the selected fleet file (`fleet/store.rs`): its operator
//! route and every member that pins an exact `provider` + `model`. Nothing
//! here invents a second member store — a fleet model is a fleet member, and
//! the roles a model fills are the member rows that pin it.

use std::path::Path;

use super::role::public_role_label;
use super::store::{
    FleetFile, FleetMember, FleetScope, FleetStoreError, load_fleet_at, load_fleet_in_scope,
    resolve_selected_fleet, save_fleet, set_selected, slugify,
};
use crate::localization::{Locale, MessageId, tr};

/// Default name for the personal fleet created by the first `/fleet add` or
/// ⇧F on a row in `/model` when no fleet is selected yet.
pub const DEFAULT_FLEET_NAME: &str = "My fleet";

/// One model in the fleet: an exact route plus the roles that pin it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetModel {
    /// Exact provider id (a `[providers.<id>]` key or a built-in id).
    pub provider: String,
    /// Exact model id on that provider's route.
    pub model: String,
    /// Roles whose member rows pin this route; `operator` for the fleet's
    /// own route. Empty when the model was added without a role.
    pub roles: Vec<String>,
    /// The fleet this model belongs to.
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

/// Why a membership change wrote nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnchangedReason {
    /// The route is the fleet's operator route (your current model while
    /// this fleet is selected) and has no member row of its own; `⇧F` and a
    /// role-less `/fleet add` leave it alone instead of duplicating it.
    OperatorRoute,
    /// Every requested role already pins the route.
    AlreadyPresent,
}

/// What a membership change did, for the receipt line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetModelChange {
    Added {
        fleet: String,
        /// The default personal fleet was created for this add.
        created_fleet: bool,
        /// The fleet was not selected before this add and is now.
        selected_fleet: bool,
        roles: Vec<String>,
    },
    Removed {
        fleet: String,
        roles: Vec<String>,
    },
    /// Nothing was written.
    Unchanged {
        fleet: String,
        reason: UnchangedReason,
    },
}

/// Why a membership change could not be made. Typed so the surface that
/// shows it picks the person's locale; `Display` is the English form for logs.
#[derive(Debug)]
pub enum FleetModelError {
    /// Provider id or model id was blank.
    NeedsRoute,
    /// No fleet is selected, so there is nothing to remove from.
    NoSelection,
    /// The route is the fleet's operator route, which `/fleet save` changes.
    OperatorRoute { route: String, fleet: String },
    /// No member row pins the route.
    NotInFleet { route: String, fleet: String },
    /// The selected fleet could not be resolved, read, parsed, or written.
    Store(FleetStoreError),
}

impl From<FleetStoreError> for FleetModelError {
    fn from(error: FleetStoreError) -> Self {
        Self::Store(error)
    }
}

impl FleetModelError {
    /// The person-facing explanation in `locale`.
    #[must_use]
    pub fn message(&self, locale: Locale) -> String {
        match self {
            Self::NeedsRoute => tr(locale, MessageId::FleetModelErrorNeedsRoute).into_owned(),
            Self::NoSelection => tr(locale, MessageId::FleetModelErrorNoSelection).into_owned(),
            Self::OperatorRoute { route, fleet } => {
                tr(locale, MessageId::FleetModelErrorOperatorRoute)
                    .replace("{route}", route)
                    .replace("{fleet}", fleet)
            }
            Self::NotInFleet { route, fleet } => tr(locale, MessageId::FleetModelErrorNotInFleet)
                .replace("{route}", route)
                .replace("{fleet}", fleet),
            Self::Store(error) => error.to_string(),
        }
    }
}

impl std::fmt::Display for FleetModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message(Locale::En))
    }
}

impl std::error::Error for FleetModelError {}

/// The selected fleet's models, operator first, then members in file order.
/// Members that inherit the session route (no pin) are not models of their
/// own and are skipped.
///
/// `Ok(empty)` means no fleet is selected (or the selected one has no pinned
/// route): the caller states "your fleet is the session model only". `Err`
/// means a fleet *is* selected but cannot be resolved or read — a broken
/// explicit selection, which must never be shown as "no selection".
pub fn fleet_models(workspace: &Path) -> Result<Vec<FleetModel>, FleetStoreError> {
    let Some(selected) = resolve_selected_fleet(workspace)? else {
        return Ok(Vec::new());
    };
    let (fleet, _scope) = load_fleet_at(&selected.path)?;
    Ok(models_of(&fleet))
}

/// Project a fleet file into its models with roles unioned per exact route.
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

fn member_pins(member: &FleetMember, provider: &str, model: &str) -> bool {
    member
        .provider
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case(provider))
        && member
            .model
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(model))
}

/// Add `provider/model` to the selected fleet, one member row per role.
///
/// With no fleet selected, the personal [`DEFAULT_FLEET_NAME`] fleet is used —
/// loaded when it already exists, created otherwise — and selected only once
/// the add has been written. Roles are deduplicated; when every requested
/// role already pins the route nothing is rewritten and the change is
/// [`FleetModelChange::Unchanged`].
pub fn add_fleet_model(
    workspace: &Path,
    provider: &str,
    model: &str,
    roles: &[String],
) -> Result<FleetModelChange, FleetModelError> {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty() || model.is_empty() {
        return Err(FleetModelError::NeedsRoute);
    }
    let target = selected_or_default(workspace)?;
    let SelectedOrDefault {
        mut fleet,
        scope,
        created_fleet,
        needs_select,
    } = target;
    let mut unique: Vec<String> = Vec::with_capacity(roles.len());
    for role in roles.iter().map(|r| r.trim()).filter(|r| !r.is_empty()) {
        if !unique.iter().any(|seen| seen.eq_ignore_ascii_case(role)) {
            unique.push(role.to_string());
        }
    }
    let roles = unique;
    let select = |needs_select: bool| -> Result<(), FleetModelError> {
        if needs_select {
            set_selected(DEFAULT_FLEET_NAME, FleetScope::Personal, workspace)?;
        }
        Ok(())
    };
    if roles.is_empty() && is_operator_route(&fleet, provider, model) {
        select(needs_select)?;
        return Ok(FleetModelChange::Unchanged {
            fleet: fleet.name,
            reason: UnchangedReason::OperatorRoute,
        });
    }
    let model_slug = slugify(model);
    let wanted: Vec<Option<&str>> = if roles.is_empty() {
        vec![None]
    } else {
        roles.iter().map(|r| Some(r.as_str())).collect()
    };
    let mut added_any = false;
    for role in wanted {
        let already = fleet.members.iter().any(|m| {
            member_pins(m, provider, model)
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
        select(needs_select)?;
        return Ok(FleetModelChange::Unchanged {
            fleet: fleet.name,
            reason: UnchangedReason::AlreadyPresent,
        });
    }
    save_fleet(&fleet, scope, workspace)?;
    select(needs_select)?;
    Ok(FleetModelChange::Added {
        fleet: fleet.name,
        created_fleet,
        selected_fleet: needs_select,
        roles,
    })
}

/// Idempotently enroll a route the user selected or used (`/model`, config,
/// a subagent run). Never prompts and never fails the caller: a store error is
/// logged and swallowed. `auto` routes and blank ids are skipped.
///
/// Returns whether the fleet on disk changed, so a caller holding the engine
/// handle can push the refreshed roster (`Op::SetFleetRoster`).
pub fn auto_enroll_fleet_model(workspace: &Path, provider: &str, model: &str) -> bool {
    let provider = provider.trim();
    let model = model.trim();
    if provider.is_empty()
        || model.is_empty()
        || provider.eq_ignore_ascii_case("auto")
        || model.eq_ignore_ascii_case("auto")
    {
        return false;
    }
    match add_fleet_model(workspace, provider, model, &[]) {
        Ok(FleetModelChange::Added { .. }) => true,
        Ok(_) => false,
        Err(error) => {
            tracing::debug!(
                provider,
                model,
                error = %error,
                "could not auto-enroll model in the fleet"
            );
            false
        }
    }
}

/// Remove every member row pinning `provider/model` from the selected fleet.
/// The operator route is not a member; it is changed with `/fleet save`.
pub fn remove_fleet_model(
    workspace: &Path,
    provider: &str,
    model: &str,
) -> Result<FleetModelChange, FleetModelError> {
    let provider = provider.trim();
    let model = model.trim();
    let Some(selected) = resolve_selected_fleet(workspace)? else {
        return Err(FleetModelError::NoSelection);
    };
    let (mut fleet, scope) = load_fleet_at(&selected.path)?;
    let before = fleet.members.len();
    let mut roles = Vec::new();
    fleet.members.retain(|m| {
        let hit = member_pins(m, provider, model);
        if hit && !m.role.trim().is_empty() {
            roles.push(m.role.clone());
        }
        !hit
    });
    if fleet.members.len() == before {
        let route = format!("{provider}/{model}");
        return Err(if is_operator_route(&fleet, provider, model) {
            FleetModelError::OperatorRoute {
                route,
                fleet: fleet.name,
            }
        } else {
            FleetModelError::NotInFleet {
                route,
                fleet: fleet.name,
            }
        });
    }
    save_fleet(&fleet, scope, workspace)?;
    Ok(FleetModelChange::Removed {
        fleet: fleet.name,
        roles,
    })
}

/// Add when absent, remove when present — the picker's one-key toggle.
///
/// Presence is decided by member rows, never by the projected role list: a
/// role-less member row projects to no role at all, so `roles` cannot tell
/// "operator only" from "operator plus a role-less row". Only the operator
/// route with no member row of its own is left alone.
pub fn toggle_fleet_model(
    workspace: &Path,
    provider: &str,
    model: &str,
) -> Result<FleetModelChange, FleetModelError> {
    let provider = provider.trim();
    let model = model.trim();
    let Some(selected) = resolve_selected_fleet(workspace)? else {
        return add_fleet_model(workspace, provider, model, &[]);
    };
    let (fleet, _scope) = load_fleet_at(&selected.path)?;
    if fleet
        .members
        .iter()
        .any(|m| member_pins(m, provider, model))
    {
        return remove_fleet_model(workspace, provider, model);
    }
    if is_operator_route(&fleet, provider, model) {
        return Ok(FleetModelChange::Unchanged {
            fleet: fleet.name,
            reason: UnchangedReason::OperatorRoute,
        });
    }
    add_fleet_model(workspace, provider, model, &[])
}

fn is_operator_route(fleet: &FleetFile, provider: &str, model: &str) -> bool {
    fleet.operator.as_ref().is_some_and(|op| {
        op.provider.eq_ignore_ascii_case(provider.trim())
            && op.model.eq_ignore_ascii_case(model.trim())
    })
}

/// One-line receipt for a membership change, shared by `/fleet add|remove`
/// and the picker's `⇧F`.
#[must_use]
pub fn change_receipt(
    locale: Locale,
    provider: &str,
    model: &str,
    change: &FleetModelChange,
) -> String {
    let route = format!("{}/{}", provider.trim(), model.trim());
    match change {
        FleetModelChange::Added {
            fleet,
            created_fleet,
            selected_fleet,
            roles,
        } => {
            let line = if roles.is_empty() {
                tr(locale, MessageId::FleetModelAdded).replace("{route}", &route)
            } else {
                tr(locale, MessageId::FleetModelAddedAs)
                    .replace("{route}", &route)
                    .replace("{roles}", &roles.join(", "))
            };
            let note = if *created_fleet {
                tr(locale, MessageId::FleetModelAddedCreatedNote)
            } else if *selected_fleet {
                tr(locale, MessageId::FleetModelAddedSelectedNote)
            } else {
                std::borrow::Cow::Borrowed("")
            };
            format!("{}{note}", line.replace("{fleet}", fleet))
        }
        FleetModelChange::Removed { fleet, roles } => {
            let line = if roles.is_empty() {
                tr(locale, MessageId::FleetModelRemoved).replace("{route}", &route)
            } else {
                tr(locale, MessageId::FleetModelRemovedRoles)
                    .replace("{route}", &route)
                    .replace("{roles}", &roles.join(", "))
            };
            line.replace("{fleet}", fleet)
        }
        FleetModelChange::Unchanged { fleet, reason } => {
            let reason = match reason {
                UnchangedReason::OperatorRoute => {
                    tr(locale, MessageId::FleetModelReasonOperatorRoute)
                }
                UnchangedReason::AlreadyPresent => {
                    tr(locale, MessageId::FleetModelReasonAlreadyPresent)
                }
            };
            tr(locale, MessageId::FleetModelUnchanged)
                .replace("{route}", &route)
                .replace("{fleet}", fleet)
                .replace("{reason}", &reason)
        }
    }
}

struct SelectedOrDefault {
    fleet: FleetFile,
    scope: FleetScope,
    /// The default personal fleet did not exist and was built in memory; it
    /// reaches disk only with the first successful add.
    created_fleet: bool,
    /// Select the default personal fleet once the add has succeeded.
    needs_select: bool,
}

/// The selected fleet, or — with nothing selected — the personal default
/// fleet: loaded when it already exists (never overwritten with an empty
/// one), built in memory otherwise. Nothing is written or selected here.
fn selected_or_default(workspace: &Path) -> Result<SelectedOrDefault, FleetStoreError> {
    if let Some(selected) = resolve_selected_fleet(workspace)? {
        let (fleet, scope) = load_fleet_at(&selected.path)?;
        return Ok(SelectedOrDefault {
            fleet,
            scope,
            created_fleet: false,
            needs_select: false,
        });
    }
    match load_fleet_in_scope(DEFAULT_FLEET_NAME, FleetScope::Personal, workspace) {
        Ok((fleet, _path)) => Ok(SelectedOrDefault {
            fleet,
            scope: FleetScope::Personal,
            created_fleet: false,
            needs_select: true,
        }),
        Err(FleetStoreError::NotFound(_)) => Ok(SelectedOrDefault {
            fleet: FleetFile::new(
                DEFAULT_FLEET_NAME.to_string(),
                Some("Models added from /models and /fleet add.".to_string()),
            )?,
            scope: FleetScope::Personal,
            created_fleet: true,
            needs_select: true,
        }),
        Err(error) => Err(error),
    }
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

    /// An isolated `CODEWHALE_HOME` and workspace; the guard must outlive
    /// the test body.
    fn isolated_workspace() -> (
        tempfile::TempDir,
        crate::test_support::EnvVarGuard,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let guard = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        (temp, guard, workspace)
    }

    fn selected_file(workspace: &Path) -> (FleetFile, std::path::PathBuf) {
        let selected = resolve_selected_fleet(workspace)
            .expect("ok")
            .expect("selected");
        let (fleet, _) = load_fleet_at(&selected.path).expect("load");
        (fleet, selected.path)
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
        let (_temp, _home, workspace) = isolated_workspace();

        assert!(fleet_models(&workspace).expect("no selection").is_empty());
        let change =
            add_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash", &[]).expect("add");
        assert_eq!(
            change,
            FleetModelChange::Added {
                fleet: DEFAULT_FLEET_NAME.to_string(),
                created_fleet: true,
                selected_fleet: true,
                roles: Vec::new(),
            }
        );
        let receipt = change_receipt(Locale::En, "openrouter", "z-ai/glm-5.3-flash", &change);
        assert!(
            receipt.contains("new personal fleet, now selected"),
            "{receipt}"
        );
        let models = fleet_models(&workspace).expect("fleet");
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
        let models = fleet_models(&workspace).expect("fleet");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].roles, ["scout"]);

        let change =
            toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash").expect("toggle");
        assert!(
            matches!(change, FleetModelChange::Removed { ref roles, .. } if roles == &["scout"])
        );
        assert!(fleet_models(&workspace).expect("fleet").is_empty());

        let err = remove_fleet_model(&workspace, "openrouter", "nope").expect_err("absent");
        assert!(
            matches!(err, FleetModelError::NotInFleet { ref route, .. } if route == "openrouter/nope"),
            "{err:?}"
        );
        assert!(
            err.message(Locale::En).contains("is not in the fleet"),
            "{err}"
        );
    }

    /// Review finding on #5815: a role-less member row projects to no role,
    /// so an `all(== "operator")` test on the projected roles was vacuously
    /// true and ⇧F could never remove the common role-less model.
    #[test]
    fn toggle_removes_a_role_less_member_row() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();
        add_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash", &[]).expect("add");

        let change =
            toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash").expect("toggle");
        assert!(
            matches!(change, FleetModelChange::Removed { ref roles, .. } if roles.is_empty()),
            "a role-less model must toggle off, got {change:?}"
        );
        let (on_disk, _) = selected_file(&workspace);
        assert!(on_disk.members.is_empty(), "{:?}", on_disk.members);
    }

    /// Review finding on #5815: with nothing selected, an existing personal
    /// `My fleet` must be loaded and extended, never overwritten with an
    /// empty file, and selected only once the add has landed.
    #[test]
    fn add_with_no_selection_reuses_an_existing_personal_default_fleet() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();
        let mut existing = fleet_with(None, &[("scout", "scout", "z-ai/glm-5.3-flash")]);
        existing.name = DEFAULT_FLEET_NAME.to_string();
        save_fleet(&existing, FleetScope::Personal, &workspace).expect("save");
        assert!(resolve_selected_fleet(&workspace).expect("ok").is_none());

        // A rejected add selects nothing.
        let err = add_fleet_model(&workspace, "", "x", &[]).expect_err("blank provider");
        assert!(matches!(err, FleetModelError::NeedsRoute), "{err:?}");
        assert!(resolve_selected_fleet(&workspace).expect("ok").is_none());

        let change = add_fleet_model(&workspace, "openrouter", "deepseek/deepseek-v4-flash", &[])
            .expect("add");
        assert_eq!(
            change,
            FleetModelChange::Added {
                fleet: DEFAULT_FLEET_NAME.to_string(),
                created_fleet: false,
                selected_fleet: true,
                roles: Vec::new(),
            }
        );
        let receipt = change_receipt(
            Locale::En,
            "openrouter",
            "deepseek/deepseek-v4-flash",
            &change,
        );
        assert!(receipt.ends_with("(now selected)"), "{receipt}");
        let (on_disk, _) = selected_file(&workspace);
        let ids: Vec<_> = on_disk.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["scout", "deepseekdeepseek-v4-flash"]);
    }

    /// Review finding on #5815: requested roles are deduplicated and a
    /// fully-present request rewrites nothing.
    #[test]
    fn add_dedupes_roles_and_is_a_no_op_when_every_role_is_present() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();
        let change = add_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3-flash",
            &[
                "scout".to_string(),
                "scout".to_string(),
                "Scout".to_string(),
            ],
        )
        .expect("add");
        assert!(
            matches!(change, FleetModelChange::Added { ref roles, .. } if roles == &["scout"]),
            "{change:?}"
        );
        let (on_disk, path) = selected_file(&workspace);
        assert_eq!(on_disk.members.len(), 1);
        let before = std::fs::read(&path).expect("read");
        let before_modified = std::fs::metadata(&path).expect("meta").modified().ok();

        let change = add_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3-flash",
            &["scout".to_string()],
        )
        .expect("add again");
        assert_eq!(
            change,
            FleetModelChange::Unchanged {
                fleet: DEFAULT_FLEET_NAME.to_string(),
                reason: UnchangedReason::AlreadyPresent,
            }
        );
        assert_eq!(std::fs::read(&path).expect("read"), before);
        assert_eq!(
            std::fs::metadata(&path).expect("meta").modified().ok(),
            before_modified,
            "a no-op add must not rewrite the file"
        );
    }

    /// Review finding on #5815: a selected fleet that cannot be read is an
    /// error, not "no fleet selected".
    #[test]
    fn fleet_models_reports_a_broken_selection_as_an_error() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();
        add_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash", &[]).expect("add");
        let (_, path) = selected_file(&workspace);

        std::fs::write(&path, "this is not = [toml").expect("corrupt");
        let err = fleet_models(&workspace).expect_err("parse error");
        assert!(matches!(err, FleetStoreError::Parse { .. }), "{err:?}");

        std::fs::remove_file(&path).expect("remove");
        let err = fleet_models(&workspace).expect_err("dangling selection");
        assert!(matches!(err, FleetStoreError::NotFound(_)), "{err:?}");
        // The picker's toggle carries the same error instead of adding to a
        // fleet nobody can read.
        let err = toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash")
            .expect_err("toggle on a broken selection");
        assert!(matches!(err, FleetModelError::Store(_)), "{err:?}");
    }

    #[test]
    fn auto_enroll_is_idempotent_and_skips_auto_routes() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();

        assert!(auto_enroll_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3-flash"
        ));
        assert!(!auto_enroll_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3-flash"
        ));
        assert_eq!(fleet_models(&workspace).expect("fleet").len(), 1);

        remove_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3-flash").expect("remove");
        assert!(!auto_enroll_fleet_model(&workspace, "auto", "auto"));
        assert!(fleet_models(&workspace).expect("fleet").is_empty());
    }

    #[test]
    fn toggling_the_operator_route_is_a_no_op_and_never_writes_a_duplicate_row() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();
        let mut fleet = fleet_with(Some(("openrouter", "z-ai/glm-5.3")), &[]);
        fleet.name = "Ops".to_string();
        save_fleet(&fleet, FleetScope::Personal, &workspace).expect("save");
        set_selected("Ops", FleetScope::Personal, &workspace).expect("select");

        let change = toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3").expect("toggle");
        assert_eq!(
            change,
            FleetModelChange::Unchanged {
                fleet: "Ops".to_string(),
                reason: UnchangedReason::OperatorRoute,
            }
        );
        let receipt = change_receipt(Locale::En, "openrouter", "z-ai/glm-5.3", &change);
        assert!(receipt.contains("stays in the fleet"), "{receipt}");
        assert!(receipt.contains("current model"), "{receipt}");
        // A role-less /fleet add of the same route is the same no-op.
        let change = add_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3", &[]).expect("add");
        assert!(
            matches!(change, FleetModelChange::Unchanged { .. }),
            "{change:?}"
        );
        let (on_disk, _) = selected_file(&workspace);
        assert!(
            on_disk.members.is_empty(),
            "no duplicate member row: {:?}",
            on_disk.members
        );
        let err = remove_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3")
            .expect_err("operator route");
        assert!(
            matches!(err, FleetModelError::OperatorRoute { .. }),
            "{err:?}"
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
        let models = fleet_models(&workspace).expect("fleet");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].roles, ["operator", "planner"]);
        let change = toggle_fleet_model(&workspace, "openrouter", "z-ai/glm-5.3").expect("toggle");
        assert!(
            matches!(change, FleetModelChange::Removed { ref roles, .. } if roles == &["planner"]),
            "{change:?}"
        );
        assert_eq!(
            fleet_models(&workspace).expect("fleet")[0].roles,
            ["operator"]
        );
    }

    #[test]
    fn member_ids_stay_unique_when_a_role_is_reused_on_two_models() {
        let _lock = crate::test_support::lock_test_env();
        let (_temp, _home, workspace) = isolated_workspace();
        add_fleet_model(&workspace, "openrouter", "a/one", &["scout".to_string()]).expect("one");
        add_fleet_model(&workspace, "openrouter", "a/two", &["scout".to_string()]).expect("two");
        let (fleet, _) = selected_file(&workspace);
        let ids: Vec<_> = fleet.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["scout", "scout-atwo"]);
    }
}
