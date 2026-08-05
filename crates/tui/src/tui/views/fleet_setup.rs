//! `/fleet setup` — a progressive "set up your agent team" flow.
//!
//! Replaces the old six-column config matrix (#3791). Fleet is presented as an
//! agent team: the shortest valid path is role → provider/model → save/apply.
//! The review step shows resolved provider, model, auth/readiness, profile
//! availability, and overwrite consequences once before anything is written. Thinking defaults to
//! inherit and can be adjusted on the review step without an extra wizard
//! screen. "Save profile" persists the exact rendered TOML bytes.
//!
//! NOTE (audit #7 / #3167): the role/model taxonomy and copy below are
//! intentionally English for now; #3167 reworks this into an interactive
//! provider/model picker that will churn most of this text. The command entry
//! (`CmdFleetDescription`) is already localized.

use std::borrow::Cow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::fleet::profile::FleetProfileScope;
use crate::localization::{MessageId, tr};
use crate::palette;
use crate::tui::app::App;
use crate::tui::menu_style;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer_with_gutter, render_modal_surface, truncate_view_text,
};

const PROFILE_DIR: &str = ".codewhale/agents";

/// A selectable choice in a wizard step: a short identifier `label`, a one-line
/// `summary`, and a longer `description` shown (wrapped) in the detail pane.
#[derive(Clone)]
struct Choice {
    label: Cow<'static, str>,
    summary: Cow<'static, str>,
    description: Cow<'static, str>,
}

const CHOICE_LIST_WIDTH: u16 = 22;
const CHOICE_DETAIL_MIN_WIDTH: u16 = 58;
const CHOICE_TWO_COLUMN_MIN_WIDTH: u16 = CHOICE_LIST_WIDTH + CHOICE_DETAIL_MIN_WIDTH;

/// Agent-team roles. `label` doubles as the profile `role_hint` and file stem,
/// so these strings are part of the generated-profile contract.
const ROLES: [Choice; 9] = [
    Choice {
        label: Cow::Borrowed("manager"),
        summary: Cow::Borrowed("Plan & split queued work"),
        description: Cow::Borrowed(
            "Coordinates the Fleet run: plans the work, splits it into bounded tasks, and dispatches workers.",
        ),
    },
    Choice {
        label: Cow::Borrowed("scout"),
        summary: Cow::Borrowed("Read-first research"),
        description: Cow::Borrowed(
            "Research and repo reconnaissance. Reads and summarizes before anything is written.",
        ),
    },
    Choice {
        label: Cow::Borrowed("builder"),
        summary: Cow::Borrowed("Implements bounded changes"),
        description: Cow::Borrowed(
            "Implements changes strictly inside its assigned task scope; writes only what the slice needs.",
        ),
    },
    Choice {
        label: Cow::Borrowed("reviewer"),
        summary: Cow::Borrowed("Read-only review"),
        description: Cow::Borrowed(
            "Checks regressions, tests, and diffs. Read-only — it never writes.",
        ),
    },
    Choice {
        label: Cow::Borrowed("verifier"),
        summary: Cow::Borrowed("Runs focused validation"),
        description: Cow::Borrowed(
            "Runs targeted validation and reports receipts back to the orchestrator.",
        ),
    },
    Choice {
        label: Cow::Borrowed("consultant"),
        summary: Cow::Borrowed("Read-only second opinion"),
        description: Cow::Borrowed(
            "Short-lived, high-reasoning counsel for difficult decisions and overlooked risks. Read-only and shell-less.",
        ),
    },
    Choice {
        label: Cow::Borrowed("synthesizer"),
        summary: Cow::Borrowed("Reduce receipts to handoff"),
        description: Cow::Borrowed(
            "Turns worker receipts into bounded handoff state instead of raw transcript replay.",
        ),
    },
    Choice {
        label: Cow::Borrowed("general"),
        summary: Cow::Borrowed("General-purpose worker"),
        description: Cow::Borrowed(
            "A flexible worker with no specialized posture — use it when the task doesn't fit a named role.",
        ),
    },
    Choice {
        label: Cow::Borrowed("custom"),
        summary: Cow::Borrowed("Author a profile by hand"),
        description: Cow::Borrowed(
            "Define the posture yourself in a workspace agent TOML profile under .codewhale/agents/.",
        ),
    },
];

/// The `inherit` row shown first in the Model step (#3167). Concrete provider
/// models follow it, built per-run from EVERY configured provider's catalog
/// (#4093), so the user picks a real route — including cross-provider ones —
/// instead of an abstract class or only the active provider's models.
const MODEL_INHERIT: Choice = Choice {
    label: Cow::Borrowed("inherit"),
    summary: Cow::Borrowed("Same model as now"),
    description: Cow::Borrowed(
        "Use the operator's current route — provider, model, and reasoning included. Recommended default.",
    ),
};

const THINKING_CHOICES: &[Choice] = &[
    Choice {
        label: Cow::Borrowed("inherit"),
        summary: Cow::Borrowed("Same thinking as now"),
        description: Cow::Borrowed(
            "Reuse the operator's current reasoning setting for this worker. Recommended default.",
        ),
    },
    Choice {
        label: Cow::Borrowed("off"),
        summary: Cow::Borrowed("No extra thinking"),
        description: Cow::Borrowed(
            "Use for narrow lookups or mechanical work where speed matters.",
        ),
    },
    Choice {
        label: Cow::Borrowed("low"),
        summary: Cow::Borrowed("Small thinking budget"),
        description: Cow::Borrowed(
            "Use for bounded checks that still benefit from light reasoning.",
        ),
    },
    Choice {
        label: Cow::Borrowed("medium"),
        summary: Cow::Borrowed("Balanced thinking budget"),
        description: Cow::Borrowed("Use for normal implementation and review work."),
    },
    Choice {
        label: Cow::Borrowed("high"),
        summary: Cow::Borrowed("Deep thinking budget"),
        description: Cow::Borrowed("Use for harder design, debugging, and integration tasks."),
    },
    Choice {
        label: Cow::Borrowed("max"),
        summary: Cow::Borrowed("Maximum thinking budget"),
        description: Cow::Borrowed("Use for hard release, security, and root-cause work."),
    },
    Choice {
        label: Cow::Borrowed("auto"),
        summary: Cow::Borrowed("Let Codewhale choose"),
        description: Cow::Borrowed("Choose a thinking tier from the worker prompt at runtime."),
    },
];

#[derive(Debug, Clone)]
pub struct FleetSetupSnapshot {
    workspace: PathBuf,
    locale: crate::localization::Locale,
    /// Whether the active provider has a key or local runtime — gates the
    /// model-draft offer, mirroring the constitution card's `provider_ready`.
    provider_ready: bool,
    provider: String,
    model: String,
    reasoning: String,
    subagents_enabled: bool,
    max_subagents: usize,
    launch_concurrency: usize,
    max_admitted: usize,
    subagent_spawn_depth: u32,
    fleet_spawn_depth: u32,
    api_timeout_secs: u64,
    heartbeat_timeout_secs: u64,
    /// Lowercased roster member ids with their origin labels (built-in /
    /// config / project), so the wizard can say when a chosen role would
    /// override an existing roster member.
    roster_members: Vec<(String, String)>,
    /// `(exact provider id, model id, readiness label, selectable)` routes for a worker,
    /// drawn from ALL configured providers — not only the active one (#4093).
    /// Shown after `inherit` in the Model step so a Fleet worker can be pinned
    /// to a route independent of the parent/current provider. The provider id
    /// is a canonical built-in id or the exact named custom table key, not a
    /// display label — see [`cross_provider_model_routes`].
    available_models: Vec<(
        String,
        String,
        crate::provider_readiness::ResolvedProviderReadiness,
    )>,
}

impl FleetSetupSnapshot {
    #[must_use]
    pub fn from_app(app: &App, config: &Config) -> Self {
        let provider = app.effective_route_identity_display().0;
        let model = if app.auto_model {
            app.last_effective_model
                .as_deref()
                .map(|effective| format!("auto -> {effective}"))
                .unwrap_or_else(|| "auto".to_string())
        } else {
            app.model.clone()
        };
        let fleet_spawn_depth = config
            .fleet
            .as_ref()
            .map(|fleet| fleet.exec.max_spawn_depth)
            .unwrap_or_else(|| codewhale_config::FleetExecConfig::default().max_spawn_depth)
            .min(codewhale_config::MAX_SPAWN_DEPTH_CEILING);
        let roster_members =
            crate::fleet::roster::FleetRoster::load(&config.fleet_config(), &app.workspace)
                .members()
                .iter()
                .map(|member| (member.id.to_lowercase(), member.origin.to_string()))
                .collect();
        let active_route_readiness = crate::provider_readiness::resolve_for_model(
            config,
            app.api_provider,
            if app.auto_model { "auto" } else { &app.model },
            &app.provider_health,
        );

        Self {
            workspace: app.workspace.clone(),
            locale: app.ui_locale,
            provider_ready: active_route_readiness.can_attempt(),
            provider,
            model,
            reasoning: app.reasoning_effort_display_label(),
            subagents_enabled: config.subagents_enabled_for_provider(app.api_provider),
            max_subagents: config.max_subagents_for_provider(app.api_provider),
            launch_concurrency: config.launch_concurrency_for_provider(app.api_provider),
            max_admitted: config.max_admitted_subagents_for_provider(app.api_provider),
            subagent_spawn_depth: config.subagent_max_spawn_depth_for_provider(app.api_provider),
            fleet_spawn_depth,
            api_timeout_secs: config.subagent_api_timeout_secs_for_provider(app.api_provider),
            heartbeat_timeout_secs: config
                .subagent_heartbeat_timeout_secs_for_provider(app.api_provider),
            roster_members,
            available_models: cross_provider_model_routes(
                config,
                app.api_provider,
                &app.provider_health,
            ),
        }
    }
}

/// Build the `(canonical provider id, model id)` pairs selectable for a worker
/// from EVERY configured provider — not only the active one (#4093). Fleet
/// workers can be pinned to a route independent of the parent/current provider,
/// so the Model step must offer the same cross-provider catalog the model
/// picker does, instead of the active provider's models alone.
///
/// The provider id here is the exact non-secret configured route key. Built-ins
/// use their canonical id; named custom routes keep their table key so saved
/// Fleet profiles can rebuild the same child client.
/// Callers derive a human-readable label from it for UI text.
pub(super) fn cross_provider_model_routes(
    config: &Config,
    active: crate::config::ApiProvider,
    health: &crate::provider_readiness::ProviderReadinessSnapshot,
) -> Vec<(
    String,
    String,
    crate::provider_readiness::ResolvedProviderReadiness,
)> {
    let mut routes = Vec::new();
    let configured = crate::provider_lake::configured_providers(config, active);
    let legacy_custom_configured = configured.contains(&crate::config::ApiProvider::Custom);
    for provider in configured
        .into_iter()
        .filter(|provider| *provider != crate::config::ApiProvider::Custom)
    {
        append_provider_model_routes(
            &mut routes,
            config,
            active,
            provider,
            provider.as_str(),
            health,
        );
    }

    // `ApiProvider::Custom` is an enum class, not a route identity. Enumerate
    // every named custom table so a Fleet on custom A can still pin a worker
    // to custom B and persist B's exact client route.
    let mut custom_names = config
        .providers
        .as_ref()
        .map(|providers| providers.custom.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    custom_names.sort();
    if custom_names.is_empty() && legacy_custom_configured {
        append_provider_model_routes(
            &mut routes,
            config,
            active,
            crate::config::ApiProvider::Custom,
            crate::config::ApiProvider::Custom.as_str(),
            health,
        );
    }
    for name in custom_names {
        let mut named_config = config.clone();
        named_config.provider = Some(name.clone());
        append_provider_model_routes(
            &mut routes,
            &named_config,
            active,
            crate::config::ApiProvider::Custom,
            &name,
            health,
        );
    }
    routes
}

fn append_provider_model_routes(
    routes: &mut Vec<(
        String,
        String,
        crate::provider_readiness::ResolvedProviderReadiness,
    )>,
    config: &Config,
    active: crate::config::ApiProvider,
    provider: crate::config::ApiProvider,
    provider_id: &str,
    health: &crate::provider_readiness::ProviderReadinessSnapshot,
) {
    // The bundled lake is only the baseline. A user may pin a valid
    // provider-specific preview or private deployment outside that catalog.
    let mut models = Vec::new();
    if let Some(model) = config
        .provider_config_for(provider)
        .and_then(|entry| entry.model.as_deref())
    {
        push_unique_model(&mut models, model);
    }
    if provider == active {
        let active_model = config.default_model();
        if !active_model.trim().eq_ignore_ascii_case("auto") {
            push_unique_model(&mut models, &active_model);
        }
    }
    for model in crate::provider_lake::models_for_provider(config, active, provider) {
        push_unique_model(&mut models, &model);
    }

    for model in models {
        let readiness =
            crate::provider_readiness::resolve_for_model(config, provider, &model, health);
        routes.push((provider_id.to_string(), model, readiness));
    }
}

fn push_unique_model(models: &mut Vec<String>, model: &str) {
    let model = model.trim();
    if !model.is_empty()
        && !models
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(model))
    {
        models.push(model.to_string());
    }
}

/// Human-readable label for a built-in provider id, falling back to an exact
/// named custom id verbatim.
pub(super) fn provider_display_label(provider_id: &str) -> String {
    crate::config::ApiProvider::parse(provider_id)
        .filter(|provider| provider.as_str() == provider_id)
        .map(|provider| provider.display_name().to_string())
        .unwrap_or_else(|| provider_id.to_string())
}

/// Which focused screen of the wizard is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Pick the team role.
    Role,
    /// Pick the model-routing class.
    Model,
    /// Review the full posture and save.
    Review,
}

/// Per-row Fleet Model step interaction state.
///
/// Replaces the old `model_selectable: Vec<bool>` so a dormant external-consent
/// route can require explicit activation (#v092-fleet-routes-fix) while
/// genuinely unconfigured routes stay blocked with a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FleetModelRowState {
    Ready,
    NeedsActivation,
    Blocked { reason: String },
}

impl FleetModelRowState {
    fn from_readiness(readiness: &crate::provider_readiness::ResolvedProviderReadiness) -> Self {
        if readiness.requires_explicit_activation() {
            return Self::NeedsActivation;
        }
        if let Some(reason) = readiness.blocked_reason() {
            return Self::Blocked {
                reason: reason.into_owned(),
            };
        }
        if readiness.can_attempt() {
            return Self::Ready;
        }
        Self::Blocked {
            reason: readiness
                .blocked_reason()
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_else(|| readiness.label().into_owned()),
        }
    }
}

pub struct FleetSetupView {
    snapshot: FleetSetupSnapshot,
    step: Step,
    role_idx: usize,
    model_idx: usize,
    thinking_idx: usize,
    profile_scope: FleetProfileScope,
    /// Cached `profile_file_status` for the Review step.
    ///
    /// `render_review` used to recompute this on every paint — `exists()` +
    /// `is_dir()` + a full `read_dir` extension count, inside the draw closure
    /// (#3908). Recomputed on entry to Review and when the scope toggles,
    /// which are the only inputs it depends on.
    profile_status: Option<(String, String)>,
    review_scroll: usize,
    /// A model-drafted profile awaiting save (already sanitized and
    /// bounded by the untrusted gate). Cleared when the selection changes so
    /// a stale draft can never be saved against fresh answers.
    model_draft: Option<Box<crate::fleet::profile::FleetProfileDraft>>,
    /// Exact rendered TOML preview for `model_draft` (header comment + the
    /// deterministic bytes saving would persist). Rendered inline on the
    /// Review step — never in a separate pager (#4093): a standalone pager
    /// view owns its own `g`/`G` scroll bindings, which silently swallowed
    /// the save keypress and left users unable to save without first
    /// pressing Esc. Keeping the preview and the save control in the same
    /// view means the footer's `g`/Enter hints are never a lie.
    model_draft_preview: Option<String>,
    /// Model-step rows: `inherit` followed by one row per concrete model from
    /// every configured provider (#4093).
    model_choices: Vec<Choice>,
    /// `(provider, model)` aligned with `model_choices`. Index 0 is `inherit`
    /// (the active route); later rows pin a concrete, possibly cross-provider
    /// route. Drives the review/copy so a pinned route names its own provider.
    model_routes: Vec<(String, String)>,
    /// Interaction state for each aligned Model row. Distinguishes ready rows,
    /// dormant external-consent rows that need explicit activation, and
    /// genuinely blocked rows with a short reason.
    model_row_states: Vec<FleetModelRowState>,
    /// Typed filter for the Model step (#4639): substring match over
    /// provider and model id, so provider-heavy catalogs (e.g. OpenRouter)
    /// stay navigable without a provider→model drill-down.
    model_query: String,
    /// Whether the Model step's filter input is capturing keystrokes (`/`
    /// toggles it; Enter keeps the filter, Esc clears it).
    model_filter_active: bool,
    /// Selectable rows registered by the latest render. Keeping mouse geometry
    /// in the view gives the Fleet walkthrough the same row ownership as its
    /// keyboard path without coupling the host to this modal's layout.
    row_hitboxes: RefCell<Vec<(Rect, usize)>>,
}

impl FleetSetupView {
    /// Refresh row states from a freshly built snapshot while preserving the
    /// user's current selection position and draft state. Used after the host
    /// validates a dormant external-consent route so the same row becomes
    /// Ready without closing and reopening the modal.
    pub fn refresh_from_snapshot(&mut self, snapshot: FleetSetupSnapshot) {
        let old_step = self.step;
        let old_role_idx = self.role_idx;
        let old_model_idx = self.model_idx;
        let old_thinking_idx = self.thinking_idx;
        let old_profile_scope = self.profile_scope;
        let old_model_query = self.model_query.clone();
        let old_model_filter_active = self.model_filter_active;
        let old_review_scroll = self.review_scroll;
        let old_profile_status = self.profile_status.clone();
        let old_model_draft = self.model_draft.clone();
        let old_model_draft_preview = self.model_draft_preview.clone();

        *self = Self::from_snapshot(snapshot);

        self.step = old_step;
        self.role_idx = old_role_idx;
        self.model_idx = old_model_idx.min(self.filtered_model_indices().len().saturating_sub(1));
        self.thinking_idx = old_thinking_idx;
        self.profile_scope = old_profile_scope;
        self.model_query = old_model_query;
        self.model_filter_active = old_model_filter_active;
        self.review_scroll = old_review_scroll;
        self.profile_status = old_profile_status;
        self.model_draft = old_model_draft;
        self.model_draft_preview = old_model_draft_preview;
    }

    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        Self::from_snapshot(FleetSetupSnapshot::from_app(app, config))
    }

    /// Open setup for a role the operator already selected in `/fleet`.
    /// Unknown/custom roster roles map to the explicit custom authoring row;
    /// Left or Esc still exposes Role so the carried choice is never sticky.
    #[must_use]
    pub fn new_for_role(app: &App, config: &Config, role: &str) -> Self {
        Self::from_snapshot_for_role(FleetSetupSnapshot::from_app(app, config), role)
    }

    fn from_snapshot_for_role(snapshot: FleetSetupSnapshot, role: &str) -> Self {
        let mut view = Self::from_snapshot(snapshot);
        view.role_idx = ROLES
            .iter()
            .position(|choice| choice.label.eq_ignore_ascii_case(role.trim()))
            .unwrap_or(ROLES.len() - 1);
        view.step = Step::Model;
        view
    }

    fn from_snapshot(snapshot: FleetSetupSnapshot) -> Self {
        let mut model_choices = vec![MODEL_INHERIT];
        // `inherit` (index 0) maps to the active route; every later row pins a
        // concrete (provider, model) drawn from all configured providers.
        let mut model_routes = vec![(snapshot.provider.clone(), snapshot.model.clone())];
        let mut model_row_states = vec![FleetModelRowState::Ready];
        for (provider, model, readiness) in &snapshot.available_models {
            let provider_label = provider_display_label(provider);
            let readiness_summary = readiness.detail().map_or_else(
                || readiness.label().into_owned(),
                |detail| format!("{}: {detail}", readiness.label()),
            );
            // Capability badges from the existing catalog/registry owners
            // (#5038): shown in the word-wrapped detail pane so the picker
            // list stays narrow-terminal friendly. Unknown models honestly
            // omit the sentence instead of blocking selection.
            let capability_note = crate::fleet::capability_badges::resolve_route_capability_badges(
                Some(provider),
                model,
            )
            .map(|badges| format!(" Capabilities: {}.", badges.summary()))
            .unwrap_or_default();
            model_choices.push(Choice {
                label: Cow::Owned(model.clone()),
                summary: Cow::Owned(format!(
                    "Pin this model ({provider_label}) · {readiness_summary}"
                )),
                description: Cow::Owned(format!(
                    "Route this worker to {model} on {provider_label} instead of inheriting the session route.{capability_note}"
                )),
            });
            // Canonical provider id (not the display label above) — this is
            // what gets persisted into the saved profile (#4093).
            model_routes.push((provider.clone(), model.clone()));
            model_row_states.push(FleetModelRowState::from_readiness(readiness));
        }
        Self {
            snapshot,
            step: Step::Role,
            role_idx: 0,
            model_idx: 0,
            thinking_idx: 0,
            // Profiles authored for a person should follow that person across
            // repositories by default. Project scope remains one `s` away and
            // keeps higher roster precedence when explicitly selected.
            profile_scope: FleetProfileScope::Personal,
            profile_status: None,
            review_scroll: 0,
            model_draft: None,
            model_draft_preview: None,
            model_choices,
            model_routes,
            model_row_states,
            model_query: String::new(),
            model_filter_active: false,
            row_hitboxes: RefCell::new(Vec::new()),
        }
    }

    /// Install a sanitized, bounded model draft. The exact TOML preview
    /// (returned here for the caller's status message) renders inline on the
    /// Review step — not in a separate pager — so the footer's `g`/Enter
    /// ratify hints stay true the instant the draft lands (#4093).
    pub fn install_model_draft(
        &mut self,
        mut draft: Box<crate::fleet::profile::FleetProfileDraft>,
        model_label: String,
        picked_route: Option<(String, String)>,
        reasoning_effort: Option<String>,
    ) -> (String, String) {
        // Re-inject the route the operator picked at `m`-press time (#4093). A
        // model draft comes from `from_untrusted_json`, which hard-sets
        // `provider: None` and echoes whatever `model` the model happened to
        // emit — so ratifying it verbatim would drop a concrete cross-provider
        // pick and persist the ambiguous, provider-scoped profile #4093 exists
        // to prevent. Pinning BOTH fields from the CARRIED route keeps the route
        // the user actually chose (the model only authored the prose), and is
        // immune to the selection changing while the async draft is in flight.
        // `inherit` (a `None` route) leaves `model`/`provider` untouched,
        // matching the deterministic Enter path.
        if let Some((provider, model)) = picked_route {
            draft.model = Some(model);
            draft.provider = Some(provider);
        }
        draft.reasoning_effort = reasoning_effort;
        let (title, header) = (
            tr(self.snapshot.locale, MessageId::FleetDraftTitle)
                .replace("{model_label}", &model_label),
            tr(self.snapshot.locale, MessageId::FleetDraftHeader)
                .replace("{name}", &draft.file_name())
                .replace("{model_label}", &model_label),
        );
        let content = format!(
            "{}{}",
            self.scope_preview_header(header),
            draft.render_toml()
        );
        self.model_draft = Some(draft);
        self.model_draft_preview = Some(content.clone());
        self.review_scroll = 0;
        (title, content)
    }

    /// The planner role chosen (drives the profile file name and `role_hint`).
    fn selected_role(&self) -> String {
        ROLES[self.role_idx.min(ROLES.len() - 1)].label.to_string()
    }

    /// Copy note when the chosen role would override an existing roster
    /// member of the same id (e.g. "overrides built-in reviewer"). A saved
    /// profile shadows lower roster layers rather than adding a new member.
    fn roster_override_note(&self) -> Option<String> {
        let role = self.selected_role().to_lowercase();
        self.snapshot
            .roster_members
            .iter()
            .find(|(id, _)| *id == role)
            .map(|(id, origin)| {
                if self.profile_scope == FleetProfileScope::Personal && origin == "project" {
                    format!(
                        "The project '{id}' profile remains higher precedence; this personal profile applies elsewhere."
                    )
                } else if self.profile_scope == FleetProfileScope::Personal {
                    format!("Overrides {origin} '{id}' unless a project profile exists.")
                } else {
                    format!("Overrides the {origin} '{id}' roster member.")
                }
            })
    }

    /// The concrete model chosen for this worker, written to the profile
    /// `model` field. `None` means `inherit` (reuse the session route).
    fn selected_model(&self) -> Option<String> {
        self.selected_route().map(|(_, model)| model)
    }

    /// The concrete `(provider, model)` chosen for this worker — a pinned route
    /// independent of the parent/current provider (#4093) — or `None` when
    /// `inherit` is selected (reuse the session route).
    fn selected_route(&self) -> Option<(String, String)> {
        let real_idx = self.real_model_idx();
        if real_idx == 0 {
            return None;
        }
        self.model_routes.get(real_idx).cloned()
    }

    /// Indices into `model_choices` visible under the current typed filter
    /// (#4639). Empty query shows every row; otherwise substring match over
    /// provider id/label and model id.
    fn filtered_model_indices(&self) -> Vec<usize> {
        let query = self.model_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return (0..self.model_choices.len()).collect();
        }
        (0..self.model_choices.len())
            .filter(|idx| {
                let (provider, model) = &self.model_routes[*idx];
                model.to_ascii_lowercase().contains(&query)
                    || provider.to_ascii_lowercase().contains(&query)
                    || provider_display_label(provider)
                        .to_ascii_lowercase()
                        .contains(&query)
                    || (*idx == 0 && "inherit same current".contains(&query))
            })
            .collect()
    }

    /// Map the filtered highlight position back to the real `model_choices`
    /// index. Selection, persistence, and hitboxes all use the real index.
    fn real_model_idx(&self) -> usize {
        let filtered = self.filtered_model_indices();
        if filtered.is_empty() {
            return 0;
        }
        filtered[self.model_idx.min(filtered.len() - 1)]
    }

    fn selected_reasoning_effort(&self) -> Option<String> {
        if self.thinking_idx == 0 {
            return None;
        }
        THINKING_CHOICES
            .get(self.thinking_idx)
            .map(|choice| choice.label.to_string())
    }

    fn selected_thinking_label(&self) -> String {
        self.selected_reasoning_effort()
            .unwrap_or_else(|| format!("inherit ({})", self.snapshot.reasoning))
    }

    fn scope_preview_header(&self, header: String) -> String {
        header.replacen(PROFILE_DIR, self.profile_scope.display_dir(), 1)
    }

    /// Number of selectable rows on the current step (0 on the review step).
    fn step_len(&self) -> usize {
        match self.step {
            Step::Role => ROLES.len(),
            Step::Model => self.filtered_model_indices().len(),
            Step::Review => 0,
        }
    }

    fn move_up(&mut self) {
        match self.step {
            Step::Role => {
                self.role_idx =
                    crate::tui::list_nav::wrap_index(self.role_idx, self.step_len(), -1);
                self.discard_model_draft();
            }
            Step::Model => {
                self.model_idx =
                    crate::tui::list_nav::wrap_index(self.model_idx, self.step_len(), -1);
                self.discard_model_draft();
            }
            Step::Review => self.review_scroll = self.review_scroll.saturating_sub(1),
        }
    }

    /// A draft is only valid for the answers it was requested against.
    fn discard_model_draft(&mut self) {
        self.model_draft = None;
        self.model_draft_preview = None;
    }

    fn move_down(&mut self) {
        match self.step {
            Step::Role => {
                self.role_idx = crate::tui::list_nav::wrap_index(self.role_idx, self.step_len(), 1);
                self.discard_model_draft();
            }
            Step::Model => {
                self.model_idx =
                    crate::tui::list_nav::wrap_index(self.model_idx, self.step_len(), 1);
                self.discard_model_draft();
            }
            Step::Review => self.review_scroll = self.review_scroll.saturating_add(1),
        }
    }

    /// Re-stat the profile directory. Called on the two transitions that can
    /// change the answer — entering Review, and toggling project/user scope —
    /// so the Review step never touches the filesystem while painting.
    fn refresh_profile_status(&mut self) {
        self.profile_status = Some(profile_file_status(
            self.profile_scope,
            &self.snapshot.workspace,
        ));
    }

    /// starter profile TOML the next save keypress would persist.
    fn advance(&mut self) -> ViewAction {
        match self.step {
            Step::Role => {
                self.step = Step::Model;
                ViewAction::None
            }
            Step::Model => {
                let idx = self.real_model_idx();
                match self.model_row_states.get(idx) {
                    Some(FleetModelRowState::Ready) => {
                        // Shortest valid path: role → model → review/save.
                        // Thinking defaults to inherit; adjust on review with `t`.
                        self.step = Step::Review;
                        self.review_scroll = 0;
                        self.refresh_profile_status();
                    }
                    Some(FleetModelRowState::NeedsActivation) => {
                        // Dormant external-consent route: explicit human
                        // selection must mint the read capability and validate
                        // only this exact provider/model. Hand off to the host
                        // so rendering stays I/O-free.
                        if let Some((provider_id, model)) = self.model_routes.get(idx)
                            && let Some(provider) = crate::config::ApiProvider::parse(provider_id)
                            && crate::tui::provider_picker::external_consent_target_for_provider(
                                provider,
                            )
                            .is_some()
                        {
                            return ViewAction::Emit(
                                ViewEvent::FleetSetupExternalConsentActivationRequested {
                                    provider_id: provider_id.clone(),
                                    model: model.clone(),
                                },
                            );
                        }
                    }
                    Some(FleetModelRowState::Blocked { .. }) => {
                        // The summary line already shows the reason; stay on
                        // the Model step so the user can pick a different route.
                    }
                    None => {}
                }
                ViewAction::None
            }
            Step::Review => self.commit_starter_profile_action(),
        }
    }

    /// Step back toward the first screen. Returns `None` at the first step (the
    /// host closes the modal via Esc instead).
    fn back(&mut self) -> ViewAction {
        match self.step {
            Step::Role => ViewAction::None,
            Step::Model => {
                self.step = Step::Role;
                ViewAction::None
            }
            Step::Review => {
                self.step = Step::Model;
                ViewAction::None
            }
        }
    }

    /// Persist the deterministic starter profile directly from the Review
    /// summary. Unlike a model-authored draft, every field is derived from the
    /// structured choices already visible on this screen, so a second TOML
    /// ratification state adds no trust boundary.
    fn commit_starter_profile_action(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested {
            draft: self.starter_profile_draft(),
            scope: self.profile_scope,
        })
    }

    /// Build a deterministic starter profile for the current role/model
    /// selection. The same save event persists this as model-drafted profiles,
    /// so duplicate-id checks and atomic writes stay in one host path.
    ///
    /// `provider` is seeded from whatever the user actually picked in the
    /// Model step (#4093) — a concrete route names its own provider
    /// explicitly, so the saved profile is never ambiguously scoped to
    /// whatever provider happens to be active at launch time. `inherit`
    /// carries no provider, matching its `model: None`.
    fn starter_profile_draft(&self) -> Box<crate::fleet::profile::FleetProfileDraft> {
        let role = &ROLES[self.role_idx.min(ROLES.len() - 1)];
        let route = self.selected_route();
        Box::new(crate::fleet::profile::FleetProfileDraft {
            id: profile_file_stem(&role.label),
            display_name: Some(role.label.to_string()),
            description: Some(format!("{} - {}", role.summary, role.description)),
            role_hint: role.label.to_string(),
            model_class_hint: None,
            model: route.as_ref().map(|(_, model)| model.clone()),
            provider: route.map(|(provider, _)| provider),
            reasoning_effort: self.selected_reasoning_effort(),
            instructions: Some(format!(
                "Role: {}. Work only within the assigned Fleet slice. Report concise evidence and stop when the assignment is complete. Do not widen permissions, trust, route configuration, or topology.",
                role.label
            )),
        })
    }

    /// The action hints for the current step's footer (wrapped by the shared
    /// footer renderer so they can never run off the modal edge).
    fn footer_hints(&self) -> Vec<ActionHint> {
        let mut hints = Vec::new();
        match self.step {
            Step::Role => {
                hints.push(ActionHint::new("↑/↓", "choose"));
                hints.push(ActionHint::new("Enter", "next"));
            }
            Step::Model => {
                hints.push(ActionHint::new("↑/↓", "choose"));
                hints.push(ActionHint::new("/", "filter"));
                hints.push(ActionHint::new("Enter", "next"));
                hints.push(ActionHint::new("←", "back"));
            }
            Step::Review => {
                hints.push(ActionHint::new("↑/↓", "scroll"));
                hints.push(ActionHint::new("s", "save location"));
                hints.push(ActionHint::new("t", "thinking"));
                if self.model_draft.is_some() {
                    hints.push(ActionHint::new("Enter", "Save profile"));
                    hints.push(ActionHint::new("g", "Save profile"));
                    hints.push(ActionHint::new("m", "redraft"));
                } else {
                    hints.push(ActionHint::new("Enter/g", "save"));
                    if self.snapshot.provider_ready {
                        hints.push(ActionHint::new("m", "model draft"));
                    }
                }
                hints.push(ActionHint::new("←", "back"));
            }
        }
        hints.push(ActionHint::new("Esc", "cancel"));
        hints
    }
}

impl ModalView for FleetSetupView {
    fn kind(&self) -> ModalKind {
        ModalKind::FleetSetup
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_up(),
            MouseEventKind::ScrollDown => self.move_down(),
            MouseEventKind::Down(MouseButton::Left) => {
                let row = self.row_hitboxes.borrow().iter().find_map(|(rect, row)| {
                    rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                        .then_some(*row)
                });
                if let Some(row) = row {
                    match self.step {
                        Step::Role => self.role_idx = row.min(ROLES.len().saturating_sub(1)),
                        Step::Model => {
                            self.model_idx = row.min(self.step_len().saturating_sub(1));
                        }
                        Step::Review => {}
                    }
                    self.discard_model_draft();
                }
            }
            _ => {}
        }
        ViewAction::None
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        // Model-step filter input captures keystrokes while active (#4639).
        if self.step == Step::Model && self.model_filter_active {
            match key.code {
                KeyCode::Enter => {
                    self.model_filter_active = false;
                }
                KeyCode::Esc => {
                    self.model_filter_active = false;
                    self.model_query.clear();
                    self.model_idx = 0;
                }
                KeyCode::Backspace => {
                    self.model_query.pop();
                    self.model_idx = 0;
                }
                KeyCode::Up => {
                    self.move_up();
                }
                KeyCode::Down => {
                    self.move_down();
                }
                KeyCode::Char(ch)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    self.model_query.push(ch);
                    self.model_idx = 0;
                }
                _ => {}
            }
            return ViewAction::None;
        }
        match key.code {
            KeyCode::Esc if self.step != Step::Role => self.back(),
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Char('/') if self.step == Step::Model => {
                self.model_filter_active = true;
                ViewAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ViewAction::None
            }
            KeyCode::Char('s') if self.step == Step::Review => {
                self.profile_scope = self.profile_scope.toggled();
                self.discard_model_draft();
                self.review_scroll = 0;
                self.refresh_profile_status();
                ViewAction::None
            }
            KeyCode::Char('t') if self.step == Step::Review => {
                self.thinking_idx = (self.thinking_idx + 1) % THINKING_CHOICES.len();
                self.discard_model_draft();
                ViewAction::None
            }
            KeyCode::Char('m') if self.step == Step::Review && self.snapshot.provider_ready => {
                let route = self.selected_route();
                ViewAction::Emit(ViewEvent::FleetProfileModelDraftRequested {
                    role: self.selected_role(),
                    model: route
                        .as_ref()
                        .map(|(_, model)| model.clone())
                        .unwrap_or_else(|| "inherit".to_string()),
                    // Carry the picked provider so the redrafted profile keeps
                    // the cross-provider route (#4093). `install_model_draft`
                    // re-injects it authoritatively from the wizard's current
                    // selection, but the event stays self-describing.
                    provider: route.map(|(provider, _)| provider),
                    reasoning_effort: self.selected_reasoning_effort(),
                    locale: self.snapshot.locale,
                })
            }
            KeyCode::Char('g') if self.step == Step::Review => {
                self.model_draft.clone().map_or_else(
                    || self.commit_starter_profile_action(),
                    |draft| {
                        ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested {
                            draft,
                            scope: self.profile_scope,
                        })
                    },
                )
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
                if self.step == Step::Review && self.model_draft.is_some() =>
            {
                // A save-ready draft is on screen; Enter should save it,
                // not silently start the manual profile-prompt flow and drop
                // the draft.
                match self.model_draft.clone() {
                    Some(draft) => {
                        ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested {
                            draft,
                            scope: self.profile_scope,
                        })
                    }
                    None => ViewAction::None,
                }
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.advance(),
            KeyCode::Left | KeyCode::Char('h') => self.back(),
            KeyCode::Home => {
                self.review_scroll = 0;
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.review_scroll = self.review_scroll.saturating_sub(8);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.review_scroll = self.review_scroll.saturating_add(8);
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.row_hitboxes.borrow_mut().clear();
        // Choice steps have a bounded list/detail body and should not expand
        // into a tall empty card on roomy terminals. Review is proof-dense and
        // scrollable, so it keeps the extra row budgeted for the footer gutter.
        let preferred_height = match self.step {
            Step::Role => 21,
            Step::Model => 22,
            Step::Review => 31,
        };
        let popup_area = centered_modal_area(area, 96, preferred_height, 60, 16);
        render_modal_surface(area, popup_area, buf);

        let step_no = match self.step {
            Step::Role => 1,
            Step::Model => 2,
            Step::Review => 3,
        };
        let block = Block::default()
            .title(Line::from(Span::styled(
                " Fleet setup — your agent team ",
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(
                Line::from(Span::styled(
                    format!(" Step {step_no}/3 "),
                    Style::default().fg(palette::TEXT_MUTED),
                ))
                .alignment(ratatui::layout::Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let hints = self.footer_hints();
        let content = render_modal_footer_with_gutter(inner, buf, &hints);

        // Header (intro + breadcrumb) above the step body.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(content);
        self.render_header(chunks[0], buf);

        match self.step {
            Step::Role => {
                let mut context = vec![
                    "Fleet runs sub-agents that delegate work. Pick the role this".to_string(),
                    "team member should play. It becomes the profile role_hint.".to_string(),
                ];
                if let Some(note) = self.roster_override_note() {
                    context.push(note);
                }
                render_choice_step(chunks[1], buf, &ROLES, self.role_idx, &context);
                register_choice_hitboxes(chunks[1], ROLES.len(), self.role_idx, &self.row_hitboxes);
            }
            Step::Model => {
                let filtered = self.filtered_model_indices();
                let filtered_choices: Vec<Choice> = filtered
                    .iter()
                    .map(|idx| self.model_choices[*idx].clone())
                    .collect();
                let selected = self.model_idx.min(filtered.len().saturating_sub(1));
                let filter_line = if self.model_filter_active {
                    format!("Filter: {}▏ (Enter keep · Esc clear)", self.model_query)
                } else if !self.model_query.trim().is_empty() {
                    format!(
                        "Filter: {} ({} of {} rows · / edit)",
                        self.model_query,
                        filtered.len(),
                        self.model_choices.len()
                    )
                } else {
                    format!(
                        "Type / to filter {} routes by provider or model",
                        self.model_choices.len()
                    )
                };
                render_choice_step(
                    chunks[1],
                    buf,
                    &filtered_choices,
                    selected,
                    &[
                        filter_line,
                        format!(
                            "Current route: {} / {}  ·  reasoning {}",
                            self.snapshot.provider, self.snapshot.model, self.snapshot.reasoning
                        ),
                        match self.selected_model() {
                            Some(model) => format!("This worker will run on {model}."),
                            None => "This worker inherits your current route.".to_string(),
                        },
                    ],
                );
                register_choice_hitboxes(
                    chunks[1],
                    filtered_choices.len(),
                    selected,
                    &self.row_hitboxes,
                );
            }
            Step::Review => self.render_review(chunks[1], buf),
        }
    }
}

impl FleetSetupView {
    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let (title, subtitle) = match self.step {
            Step::Role => (
                "Choose a team role",
                "Each Fleet member plays one role in the delegation.",
            ),
            Step::Model => (
                "Choose a model",
                "Pick this worker's model, or inherit your current route.",
            ),
            Step::Review if self.model_draft.is_some() => (
                "Save profile",
                "Exact TOML shown below. Press Enter or g to save, m to redraft.",
            ),
            Step::Review => (
                "Review & save",
                "Confirm provider, model, readiness, profile availability, and overwrite, then save the profile.",
            ),
        };
        let lines = vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(palette::WHALE_INFO).bold(),
            )),
            Line::from(Span::styled(
                subtitle,
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    fn render_review(&self, area: Rect, buf: &mut Buffer) {
        // A ratify-ready draft is on screen: show the exact TOML preview
        // inline, scrolled by the same `review_scroll` state, so `g`/Enter in
        // THIS view's own `handle_key` ratify it directly — no separate pager
        // in the way to swallow the keypress (#4093).
        if let Some(preview) = self.model_draft_preview.as_deref() {
            render_scrollable_text(area, buf, preview, self.review_scroll);
            return;
        }

        let role = &ROLES[self.role_idx.min(ROLES.len() - 1)];
        // Cached on entry to this step and on scope toggle; see `profile_status`.
        let profile_value = self
            .profile_status
            .as_ref()
            .map(|(value, _)| value.clone())
            .unwrap_or_default();
        let file_stem = profile_file_stem(&role.label);
        let mut lines: Vec<Line> = Vec::new();
        let section = |lines: &mut Vec<Line>, label: &str, body: String| {
            lines.push(Line::from(Span::styled(
                label.to_string(),
                Style::default().fg(palette::WHALE_INFO).bold(),
            )));
            lines.push(Line::from(Span::styled(
                body,
                Style::default().fg(palette::TEXT_PRIMARY),
            )));
            lines.push(Line::from(""));
        };

        section(
            &mut lines,
            "Role",
            match self.roster_override_note() {
                Some(note) => format!("{} — {} · {note}", role.label, role.summary),
                None => format!("{} — {}", role.label, role.summary),
            },
        );
        section(
            &mut lines,
            "Model",
            // The picked route's OWN provider, not the parent/current
            // session's — a cross-provider pin must never be misreported as
            // running on the active provider (#4093).
            match self.selected_route() {
                Some((provider, model)) => {
                    let readiness = self
                        .snapshot
                        .available_models
                        .iter()
                        .find(|(candidate_provider, candidate_model, _)| {
                            candidate_provider == &provider && candidate_model == &model
                        })
                        .map(|(_, _, readiness)| readiness.label().into_owned())
                        .unwrap_or_else(|| {
                            if self.snapshot.provider_ready {
                                "ready".to_string()
                            } else {
                                "needs action".to_string()
                            }
                        });
                    format!(
                        "{model}  ·  provider {}  ·  {readiness}",
                        provider_display_label(&provider)
                    )
                }
                None => format!(
                    "inherit  ·  route {} / {}  ·  {}",
                    self.snapshot.provider,
                    self.snapshot.model,
                    if self.snapshot.provider_ready {
                        "ready"
                    } else {
                        "needs action"
                    }
                ),
            },
        );
        section(&mut lines, "Thinking", self.selected_thinking_label());
        section(
            &mut lines,
            "Profile availability",
            match self.profile_scope {
                FleetProfileScope::Project => format!(
                    "Project — saved with this repository at {PROFILE_DIR}. Press s for a personal profile reusable across repositories. This choice only controls where the profile is available; active workspace, trusted-path, and permission policy still govern execution."
                ),
                FleetProfileScope::Personal => format!(
                    "Personal — reusable at {}; project profiles override by id. Press s for project. Scope changes discovery only; workspace, trusted-path, and permission policy still govern execution.",
                    self.profile_scope.display_dir()
                ),
            },
        );
        section(
            &mut lines,
            "Auth & readiness",
            if self.snapshot.provider_ready {
                "Active route can be attempted with the current credentials.".to_string()
            } else {
                "Active route is not ready — fix auth/readiness before relying on this profile at runtime.".to_string()
            },
        );
        section(
            &mut lines,
            "Permissions",
            "Inherit the parent envelope and narrow only. Children cannot widen approval, trust, or secrets, and required approvals stay on.".to_string(),
        );
        section(
            &mut lines,
            "Tools",
            "Read tools by default; write tools for builders within scope; shell stays policy-gated; artifacts and receipts stay inspectable.".to_string(),
        );
        section(
            &mut lines,
            "Workspace & org",
            format!(
                "{} · sub-agents {} ({} concurrent, {} launch slots, {} admitted) · recursion agent {} / fleet {} (ceiling {})",
                self.snapshot.workspace.display(),
                if self.snapshot.subagents_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                self.snapshot.max_subagents,
                self.snapshot.launch_concurrency,
                self.snapshot.max_admitted,
                self.snapshot.subagent_spawn_depth,
                self.snapshot.fleet_spawn_depth,
                codewhale_config::MAX_SPAWN_DEPTH_CEILING,
            ),
        );
        section(&mut lines, "Review policy", self.review_policy_summary());
        section(
            &mut lines,
            "Profile",
            format!(
                "{}/{file_stem}.toml  ·  {profile_value} present. Press Enter or g once to save the deterministic starter profile.",
                self.profile_scope.display_dir(),
            ),
        );

        // `scroll` offsets by *visual* (post-wrap) rows, so the bound must count
        // wrapped rows — not logical lines — or the bottom sections become
        // unreachable. Estimate each line's wrapped height from its display
        // width; an over-estimate is harmless (scroll clamps at the real end).
        let wrap_width = usize::from(area.width).max(1);
        let visual_rows: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(wrap_width).max(1))
            .sum();
        let max_scroll = visual_rows.saturating_sub(usize::from(area.height).max(1));
        let scroll = self.review_scroll.min(max_scroll);
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .scroll((scroll as u16, 0))
            .render(area, buf);
    }

    fn review_policy_summary(&self) -> String {
        format!(
            "Workers run without a token cap by default · {}s api, {}s heartbeat. Launch with Fleet → exec; /fleet workers (or /subagents) shows sub-agents in the current interactive session; /fleet status and codewhale fleet status both read the persistent .codewhale/fleet.jsonl ledger.",
            self.snapshot.api_timeout_secs, self.snapshot.heartbeat_timeout_secs
        )
    }
}

/// Render wrapped, line-scrolled plain text (the ratify-ready draft TOML
/// preview) into `area`, clamping `scroll` to the real wrapped-row bound the
/// same way [`FleetSetupView::render_review`]'s summary does — an
/// over-estimate of wrapped height is harmless (scroll clamps at the end).
fn render_scrollable_text(area: Rect, buf: &mut Buffer, text: &str, scroll: usize) {
    let lines: Vec<Line> = text
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    let wrap_width = usize::from(area.width).max(1);
    let visual_rows: usize = lines
        .iter()
        .map(|line| line.width().div_ceil(wrap_width).max(1))
        .sum();
    let max_scroll = visual_rows.saturating_sub(usize::from(area.height).max(1));
    let scroll = scroll.min(max_scroll);
    Paragraph::new(lines)
        .wrap(Wrap { trim: true })
        .scroll((scroll as u16, 0))
        .render(area, buf);
}

/// Render a wizard choice step: a list of selectable identifiers on the left and
/// a wrapped detail pane (summary + description + context) on the right. Stacks
/// vertically when the body is too narrow for two columns so nothing truncates.
fn render_choice_step(
    area: Rect,
    buf: &mut Buffer,
    choices: &[Choice],
    selected: usize,
    context: &[String],
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let (list_area, detail_area) = if area.width >= CHOICE_TWO_COLUMN_MIN_WIDTH {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(CHOICE_LIST_WIDTH),
                Constraint::Min(CHOICE_DETAIL_MIN_WIDTH),
            ])
            .split(area);
        (cols[0], cols[1])
    } else {
        let list_height = (choices.len() as u16).min(area.height.saturating_sub(1).max(1));
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_height), Constraint::Min(1)])
            .split(area);
        (rows[0], rows[1])
    };

    // List: labels are identifiers, so a `▸`-marked single line each is safe.
    let list_width = usize::from(list_area.width);
    let visible = choices.len().min(usize::from(list_area.height));
    let row_start = choice_window_start(choices.len(), selected, visible);
    let mut list_lines: Vec<Line> = Vec::with_capacity(visible);
    for (idx, choice) in choices.iter().enumerate().skip(row_start).take(visible) {
        let is_selected = idx == selected;
        let pointer = format!("{} ", crate::tui::glyphs::selection_marker(is_selected));
        let style = if is_selected {
            menu_style::selected_row_style()
        } else {
            Style::default().fg(palette::TEXT_PRIMARY)
        };
        list_lines.push(Line::from(Span::styled(
            truncate_view_text(&format!("{pointer}{}", choice.label), list_width),
            style,
        )));
    }
    Paragraph::new(list_lines).render(list_area, buf);

    // Detail: summary + wrapped description + wrapped context, all word-wrapped.
    let choice = &choices[selected.min(choices.len().saturating_sub(1))];
    let mut detail_lines: Vec<Line> = vec![
        Line::from(Span::styled(
            choice.summary.clone(),
            Style::default().fg(palette::WHALE_ACTION).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            choice.description.clone(),
            Style::default().fg(palette::TEXT_PRIMARY),
        )),
    ];
    if !context.is_empty() {
        detail_lines.push(Line::from(""));
        for entry in context {
            detail_lines.push(Line::from(Span::styled(
                entry.clone(),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
    }
    Paragraph::new(detail_lines)
        .wrap(Wrap { trim: true })
        .render(detail_area, buf);
}

/// Register exactly the list column/stack rows painted by
/// [`render_choice_step`]. The detail pane intentionally owns no hitboxes.
fn register_choice_hitboxes(
    area: Rect,
    choice_count: usize,
    selected: usize,
    hitboxes: &RefCell<Vec<(Rect, usize)>>,
) {
    if area.width == 0 || area.height == 0 || choice_count == 0 {
        return;
    }
    let list_area = if area.width >= CHOICE_TWO_COLUMN_MIN_WIDTH {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(CHOICE_LIST_WIDTH),
                Constraint::Min(CHOICE_DETAIL_MIN_WIDTH),
            ])
            .split(area)[0]
    } else {
        let list_height = (choice_count as u16).min(area.height.saturating_sub(1).max(1));
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_height), Constraint::Min(1)])
            .split(area)[0]
    };
    let visible = choice_count.min(usize::from(list_area.height));
    let row_start = choice_window_start(choice_count, selected, visible);
    let mut rows = hitboxes.borrow_mut();
    rows.extend((0..visible).map(|visible_idx| {
        let choice_idx = row_start + visible_idx;
        (
            Rect::new(
                list_area.x,
                list_area.y.saturating_add(visible_idx as u16),
                list_area.width,
                1,
            ),
            choice_idx,
        )
    }));
}

fn choice_window_start(total: usize, selected: usize, visible: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    selected
        .saturating_add(1)
        .saturating_sub(visible)
        .min(total.saturating_sub(visible))
}

fn profile_file_status(scope: FleetProfileScope, workspace: &Path) -> (String, String) {
    let dir = match crate::fleet::profile::agent_profile_dir_for_scope(scope, workspace) {
        Ok(dir) => dir,
        Err(err) => {
            return (
                "blocked".to_string(),
                format!("profile save location unavailable: {err:#}"),
            );
        }
    };
    let display_dir = scope.display_dir();
    if !dir.exists() {
        return (
            "0 files".to_string(),
            format!("create {display_dir}/*.toml"),
        );
    }
    if !dir.is_dir() {
        return (
            "blocked".to_string(),
            format!("{} is not a dir", dir.display()),
        );
    }

    let count = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("toml"))
        .count();

    if count == 1 {
        ("1 file".to_string(), display_dir.to_string())
    } else {
        (format!("{count} files"), display_dir.to_string())
    }
}

/// Sanitize a planner role label into a safe TOML file stem.
fn profile_file_stem(role: &str) -> String {
    let stem: String = role
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let stem = stem.trim_matches('-').to_ascii_lowercase();
    if stem.is_empty() {
        "custom".to_string()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::views::ViewStack;
    use crossterm::event::KeyModifiers;
    use unicode_width::UnicodeWidthStr;

    const BLOCKER_SIZES: [(u16, u16); 5] = [(80, 24), (89, 50), (100, 30), (120, 32), (160, 40)];

    fn snapshot() -> FleetSetupSnapshot {
        FleetSetupSnapshot {
            workspace: PathBuf::from("/tmp/codewhale-test-workspace"),
            locale: crate::localization::Locale::En,
            provider_ready: true,
            provider: "DeepSeek".to_string(),
            model: "deepseek-v4-pro".to_string(),
            reasoning: "Auto".to_string(),
            subagents_enabled: true,
            max_subagents: 8,
            launch_concurrency: 3,
            max_admitted: 20,
            subagent_spawn_depth: 3,
            fleet_spawn_depth: 3,
            api_timeout_secs: 120,
            heartbeat_timeout_secs: 300,
            roster_members: crate::fleet::roster::FleetRoster::built_ins_only()
                .members()
                .iter()
                .map(|member| (member.id.to_lowercase(), member.origin.to_string()))
                .collect(),
            available_models: vec![
                (
                    "deepseek".to_string(),
                    "deepseek-v4-pro".to_string(),
                    crate::provider_readiness::ResolvedProviderReadiness::SavedUnchecked,
                ),
                (
                    "deepseek".to_string(),
                    "deepseek-v4-flash".to_string(),
                    crate::provider_readiness::ResolvedProviderReadiness::SavedUnchecked,
                ),
            ],
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sample_draft() -> Box<crate::fleet::profile::FleetProfileDraft> {
        let crate::fleet::profile::UntrustedProfileParse::Drafted(draft) =
            crate::fleet::profile::FleetProfileDraft::from_untrusted_json(
                r#"{"id":"reviewer","role_hint":"reviewer","description":"Reviews diffs.","instructions":"Read. Report. Stop."}"#,
            )
        else {
            panic!("sample draft should parse");
        };
        draft
    }

    /// #5038: the Model step's detail pane carries capability badges for
    /// known catalog models and honestly omits them for unknown models, so
    /// stale/absent data never blocks selection.
    #[test]
    fn model_step_detail_shows_capability_badges_for_known_models_only() {
        let mut snap = snapshot();
        snap.available_models.push((
            "deepseek".to_string(),
            "totally-made-up-model-xyz".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::SavedUnchecked,
        ));
        let view = FleetSetupView::from_snapshot(snap);

        let known = view
            .model_choices
            .iter()
            .find(|choice| choice.label == "deepseek-v4-pro")
            .expect("known catalog model row");
        assert!(
            known.description.contains("Capabilities:"),
            "{}",
            known.description
        );
        assert!(
            known.description.contains("1M ctx"),
            "{}",
            known.description
        );
        assert!(
            known.description.contains("catalog"),
            "catalog-backed rows must name catalog provenance: {}",
            known.description
        );

        let unknown = view.model_choices.last().expect("appended unknown row");
        assert_eq!(unknown.label, "totally-made-up-model-xyz");
        assert!(
            !unknown.description.contains("Capabilities:"),
            "{}",
            unknown.description
        );
        // The unknown row stays selectable; absence of data is not a block.
        assert_eq!(
            view.model_row_states.last(),
            Some(&FleetModelRowState::Ready)
        );
    }

    #[test]
    fn provider_display_label_preserves_case_colliding_custom_ids() {
        assert_eq!(provider_display_label("deepseek"), "DeepSeek");
        assert_eq!(provider_display_label("CUSTOM"), "CUSTOM");
        assert_eq!(provider_display_label("OPENAI"), "OPENAI");
    }

    fn to_review(view: &mut FleetSetupView) {
        view.handle_key(key(KeyCode::Enter)); // Role -> Model
        view.handle_key(key(KeyCode::Enter)); // Model -> Review
        assert_eq!(view.step, Step::Review);
    }

    #[test]
    fn review_step_m_requests_model_draft_with_current_answers() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);

        let action = view.handle_key(key(KeyCode::Char('m')));
        let ViewAction::Emit(ViewEvent::FleetProfileModelDraftRequested {
            role,
            model,
            provider,
            reasoning_effort,
            locale,
        }) = action
        else {
            panic!("expected model draft request");
        };
        assert!(!role.is_empty());
        assert!(!model.is_empty());
        // Default selection is `inherit` (model_idx 0), which carries no
        // concrete provider route.
        assert_eq!(provider, None);
        assert_eq!(reasoning_effort, None);
        assert_eq!(locale, crate::localization::Locale::En);
    }

    #[test]
    fn m_redraft_preserves_a_cross_provider_pick_regression_4093() {
        // #4093 BLOCKER 2 regression: a cross-provider route pick followed by an
        // `m` model-assisted redraft must STILL persist the picked provider. A
        // model draft comes from `from_untrusted_json`, which hard-sets
        // `provider: None` (and can echo any model). Without re-injection the
        // ratified profile would carry `model` with no `provider` — the exact
        // ambiguous, provider-scoped profile #4093 removes.
        //
        // The active/session provider is DeepSeek; the picked route is a
        // GLM model on Zai — a genuinely different provider than the parent.
        let mut snap = snapshot();
        snap.provider = "DeepSeek".to_string();
        snap.model = "deepseek-v4-pro".to_string();
        snap.available_models = vec![(
            "zai".to_string(),
            "glm-5.2".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::SavedUnchecked,
        )];
        let mut view = FleetSetupView::from_snapshot(snap);

        // Role step: keep the first role. Model step: inherit(0), then the one
        // cross-provider row (1) -> pick it. Then advance to Review.
        view.handle_key(key(KeyCode::Enter)); // Role -> Model
        view.handle_key(key(KeyCode::Down)); // -> the zai/glm-5.2 row
        assert_eq!(
            view.selected_route(),
            Some(("zai".to_string(), "glm-5.2".to_string()))
        );
        view.handle_key(key(KeyCode::Enter)); // Model -> Review
        while view.selected_reasoning_effort().as_deref() != Some("max") {
            view.handle_key(key(KeyCode::Char('t')));
        }

        // `m` requests a draft and carries the picked cross-provider route.
        let action = view.handle_key(key(KeyCode::Char('m')));
        let ViewAction::Emit(ViewEvent::FleetProfileModelDraftRequested {
            model,
            provider,
            reasoning_effort,
            ..
        }) = action
        else {
            panic!("expected model draft request");
        };
        assert_eq!(model, "glm-5.2");
        assert_eq!(provider.as_deref(), Some("zai"));
        assert_eq!(reasoning_effort.as_deref(), Some("max"));

        // The host reconstructs the picked route from the event exactly as
        // `handle_fleet_profile_model_draft` does, and carries it to
        // `install_model_draft` (immune to the selection changing mid-draft).
        let picked_route = provider.map(|provider| (provider, model.clone()));

        // The model returns a draft that (as always) has provider: None — the
        // untrusted gate strips any provider a model tries to smuggle.
        let drafted = sample_draft();
        assert_eq!(drafted.provider, None);

        // Installing it re-injects the picked route, so the ratified draft keeps
        // BOTH the provider and the model the user actually chose, plus the
        // captured thinking tier.
        let (_title, content) = view.install_model_draft(
            drafted,
            "GLM-5.2".to_string(),
            picked_route,
            reasoning_effort,
        );
        let ratified = view.model_draft.as_deref().expect("draft installed");
        assert_eq!(ratified.provider.as_deref(), Some("zai"));
        assert_eq!(ratified.model.as_deref(), Some("glm-5.2"));
        assert_eq!(ratified.reasoning_effort.as_deref(), Some("max"));

        // The rendered TOML the ratify keypress would persist names the provider
        // explicitly — never a provider-scoped ambiguity.
        assert!(content.contains("provider = \"zai\""), "{content}");
        assert!(content.contains("model = \"glm-5.2\""), "{content}");
        assert!(content.contains("reasoning_effort = \"max\""), "{content}");

        // And ratifying commits exactly that route.
        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, scope }) =
            action
        else {
            panic!("expected ratify commit event");
        };
        assert_eq!(scope, FleetProfileScope::Personal);
        assert_eq!(draft.provider.as_deref(), Some("zai"));
        assert_eq!(draft.model.as_deref(), Some("glm-5.2"));
        assert_eq!(draft.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn model_step_filter_narrows_large_catalogs_by_provider_and_model() {
        let mut snap = snapshot();
        // Simulate an OpenRouter-scale catalog: many rows from one provider.
        for i in 0..120 {
            snap.available_models.push((
                "openrouter".to_string(),
                format!("vendor/model-{i:03}"),
                crate::provider_readiness::ResolvedProviderReadiness::SavedUnchecked,
            ));
        }
        snap.available_models.push((
            "openrouter".to_string(),
            "z-ai/glm-5-turbo".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::SavedUnchecked,
        ));
        let mut view = FleetSetupView::from_snapshot(snap);
        // Role → Model.
        view.handle_key(key(KeyCode::Enter));
        let full_len = view.step_len();
        assert!(full_len > 120, "unfiltered shows the whole catalog");

        // `/` opens the filter; typing narrows by model id substring.
        view.handle_key(key(KeyCode::Char('/')));
        for ch in "glm".chars() {
            view.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(view.step_len(), 1, "only the glm row survives the filter");
        let route = view.selected_route().expect("filtered selection resolves");
        assert_eq!(
            route,
            ("openrouter".to_string(), "z-ai/glm-5-turbo".to_string())
        );

        // Provider substring filters too.
        view.handle_key(key(KeyCode::Esc));
        view.handle_key(key(KeyCode::Char('/')));
        for ch in "deepseek".chars() {
            view.handle_key(key(KeyCode::Char(ch)));
        }
        // inherit's route IS the active DeepSeek route, so it matches too.
        assert_eq!(
            view.step_len(),
            3,
            "deepseek rows plus the inherit (active deepseek route) match"
        );

        // Enter keeps the filter but releases the input; Esc in filter clears.
        view.handle_key(key(KeyCode::Enter));
        assert!(!view.model_filter_active);
        assert_eq!(view.step_len(), 3);
        view.handle_key(key(KeyCode::Char('/')));
        view.handle_key(key(KeyCode::Esc));
        assert_eq!(
            view.step_len(),
            full_len,
            "clearing restores the full catalog"
        );
    }

    #[test]
    fn review_saves_starter_or_ratifies_installed_model_draft() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);

        // A structured starter draft is save-ready from the summary.
        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, scope }) =
            action
        else {
            panic!("expected starter commit event");
        };
        assert_eq!(scope, FleetProfileScope::Personal);
        assert_eq!(draft.id, "manager");

        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);
        let (title, content) =
            view.install_model_draft(sample_draft(), "GLM-5.2".to_string(), None, None);
        assert!(title.contains("GLM-5.2"));
        assert!(content.contains("id = \"reviewer\""), "{content}");
        assert!(content.contains("Nothing is saved until"), "{content}");

        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, scope }) =
            action
        else {
            panic!("expected ratify commit event");
        };
        assert_eq!(scope, FleetProfileScope::Personal);
        assert_eq!(draft.id, "reviewer");
    }

    #[test]
    fn changing_answers_discards_a_stale_draft() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);
        let _ = view.install_model_draft(sample_draft(), "GLM-5.2".to_string(), None, None);
        assert!(view.model_draft.is_some());

        // Back to the role step and change the selection: the draft no
        // longer matches the answers and must not survive to ratification.
        view.handle_key(key(KeyCode::Left));
        view.handle_key(key(KeyCode::Left));
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.step, Step::Role);
        view.handle_key(key(KeyCode::Down));
        assert!(view.model_draft.is_none());

        to_review(&mut view);
        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, .. }) =
            action
        else {
            panic!("expected fresh deterministic starter");
        };
        assert_eq!(draft.id, "scout");
    }

    #[test]
    fn arrows_move_within_step_and_enter_advances() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        assert_eq!(view.step, Step::Role);

        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.role_idx, 1);

        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.step, Step::Model);

        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.model_idx, 1);

        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.step, Step::Review);

        // `t` cycles thinking on the review step without an extra wizard screen.
        view.handle_key(key(KeyCode::Char('t')));
        assert_eq!(view.thinking_idx, 1);

        // Left steps back through the wizard.
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.step, Step::Model);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.step, Step::Role);
    }

    #[test]
    fn roster_role_handoff_starts_at_model_and_can_return_to_role() {
        let mut via_left = FleetSetupView::from_snapshot_for_role(snapshot(), "consultant");
        assert_eq!(via_left.step, Step::Model);
        assert_eq!(via_left.selected_role(), "consultant");
        assert!(matches!(
            via_left.handle_key(key(KeyCode::Left)),
            ViewAction::None
        ));
        assert_eq!(via_left.step, Step::Role);
        assert_eq!(via_left.selected_role(), "consultant");

        let mut via_esc = FleetSetupView::from_snapshot_for_role(snapshot(), "reviewer");
        assert_eq!(via_esc.step, Step::Model);
        assert_eq!(via_esc.selected_role(), "reviewer");
        assert!(matches!(
            via_esc.handle_key(key(KeyCode::Esc)),
            ViewAction::None
        ));
        assert_eq!(via_esc.step, Step::Role);
        assert_eq!(via_esc.selected_role(), "reviewer");

        let custom = FleetSetupView::from_snapshot_for_role(snapshot(), "domain-expert");
        assert_eq!(custom.step, Step::Model);
        assert_eq!(custom.selected_role(), "custom");
    }

    #[test]
    fn esc_steps_back_then_cancels_from_role() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        view.handle_key(key(KeyCode::Enter)); // -> Model
        let action = view.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.step, Step::Role);
        let action = view.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn mouse_selects_rows_and_wheel_matches_keyboard_navigation() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        let (rect, row) = view.row_hitboxes.borrow()[2];

        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(row, 2);
        assert_eq!(view.role_idx, 2);

        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(view.role_idx, 3);
        view.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(view.role_idx, 2);
    }

    #[test]
    fn compact_choice_window_keeps_deep_selection_visible_and_clickable() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        view.role_idx = ROLES.len() - 1;
        let area = Rect::new(0, 0, 80, 16);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        let rendered = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("▸ custom"), "{rendered}");
        assert!(
            view.row_hitboxes
                .borrow()
                .iter()
                .any(|(_, idx)| *idx == ROLES.len() - 1),
            "selected row needs an aligned mouse hitbox"
        );
    }

    /// #3908: `render_review` recomputed `profile_file_status` — `exists()` +
    /// `is_dir()` + a full `read_dir` extension count — on every paint. It is
    /// now computed on the transitions that can change it, so the value the
    /// Review step paints must be present and must track a scope toggle.
    #[test]
    fn review_profile_status_is_cached_on_transitions_not_recomputed_per_paint() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        assert!(
            view.profile_status.is_none(),
            "nothing is stat-ed before the user reaches Review"
        );

        view.advance();
        view.advance();
        assert_eq!(view.step, Step::Review);
        let on_entry = view
            .profile_status
            .clone()
            .expect("entering Review must populate the cached status");

        // Painting repeatedly must not change the cached value — that is the
        // whole point — and must not panic on the cached-read path.
        let area = Rect::new(0, 0, 80, 24);
        for _ in 0..3 {
            let mut buf = Buffer::empty(area);
            view.render(area, &mut buf);
        }
        assert_eq!(view.profile_status.as_ref(), Some(&on_entry));

        // Toggling scope changes which directory is described, so the cache
        // has to be refreshed on that keypress.
        let before_scope = view.profile_scope;
        view.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_ne!(view.profile_scope, before_scope);
        assert!(
            view.profile_status.is_some(),
            "a scope toggle must leave a freshly computed status behind"
        );
    }

    #[test]
    fn profile_status_distinguishes_fresh_and_existing_workspaces() {
        let temp = tempfile::tempdir().expect("temp workspace");
        assert_eq!(
            profile_file_status(FleetProfileScope::Project, temp.path()),
            (
                "0 files".to_string(),
                "create .codewhale/agents/*.toml".to_string()
            )
        );

        let profile_dir = temp.path().join(PROFILE_DIR);
        std::fs::create_dir_all(&profile_dir).expect("profile dir");
        std::fs::write(profile_dir.join("reviewer.toml"), "id = \"reviewer\"\n")
            .expect("existing profile");
        assert_eq!(
            profile_file_status(FleetProfileScope::Project, temp.path()),
            ("1 file".to_string(), PROFILE_DIR.to_string())
        );
    }

    #[test]
    fn one_enter_from_review_saves_starter_profile_for_selection() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        // Role: manager(0) scout(1) builder(2) -> builder.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter)); // -> Model
        // Model: inherit(0) deepseek-v4-pro(1) -> deepseek-v4-pro.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter)); // Model -> Review
        while view.selected_reasoning_effort().as_deref() != Some("max") {
            view.handle_key(key(KeyCode::Char('t')));
        }

        // The Review summary is already the structured confirmation surface;
        // one Enter saves the deterministic starter without another state.
        let action = view.handle_key(key(KeyCode::Enter));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, scope }) =
            action
        else {
            panic!("expected one-Enter starter save");
        };
        let content = draft.render_toml();
        assert!(content.contains("id = \"builder\""));
        assert!(content.contains("role_hint = \"builder\""));
        assert!(content.contains("model = \"deepseek-v4-pro\""));
        assert!(content.contains("reasoning_effort = \"max\""));
        // A concrete cross-provider route pin names its own provider
        // explicitly (#4093) — the saved profile must not be ambiguously
        // scoped to whatever provider happens to be active at launch time.
        assert!(content.contains("provider = \"deepseek\""), "{content}");
        for forbidden in ["base_url", "api_key"] {
            assert!(
                !content.contains(forbidden),
                "starter profile must not carry {forbidden}: {content}"
            );
        }

        assert_eq!(scope, FleetProfileScope::Personal);
        assert_eq!(draft.id, "builder");
        assert_eq!(draft.role_hint, "builder");
        assert_eq!(draft.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(draft.provider.as_deref(), Some("deepseek"));
        assert_eq!(draft.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn review_defaults_to_personal_and_can_switch_to_project() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);

        assert_eq!(view.profile_scope, FleetProfileScope::Personal);
        view.handle_key(key(KeyCode::Char('s')));
        assert_eq!(view.profile_scope, FleetProfileScope::Project);

        let action = view.handle_key(key(KeyCode::Enter));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, scope }) =
            action
        else {
            panic!("expected project profile save event");
        };
        assert_eq!(scope, FleetProfileScope::Project);
        let rendered = draft.render_toml();
        assert!(rendered.contains("id = \"manager\""), "{rendered}");
    }

    #[test]
    fn inherit_selection_starter_draft_carries_no_provider() {
        // `inherit` (no concrete route pin) must never carry a provider —
        // there's no explicit route to name (#4093).
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);
        let action = view.handle_key(key(KeyCode::Enter));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft, .. }) =
            action
        else {
            panic!("expected inherit starter save");
        };
        assert_eq!(draft.model, None);
        assert_eq!(draft.provider, None);
        assert_eq!(draft.reasoning_effort, None);
        let content = draft.render_toml();
        assert!(!content.contains("provider"), "{content}");
        assert!(!content.contains("reasoning_effort"), "{content}");
    }

    #[test]
    fn role_and_review_steps_note_roster_overrides() {
        // "reviewer" collides with the built-in roster member; the
        // role step context and review Role section must both say so.
        let mut view = FleetSetupView::from_snapshot(snapshot());
        for _ in 0..3 {
            view.handle_key(key(KeyCode::Down));
        }
        assert_eq!(view.selected_role(), "reviewer");
        assert_eq!(
            view.roster_override_note().as_deref(),
            Some("Overrides built-in 'reviewer' unless a project profile exists.")
        );

        let role_step = render_through_stack(
            || {
                let mut v = FleetSetupView::from_snapshot(snapshot());
                for _ in 0..3 {
                    v.handle_key(key(KeyCode::Down));
                }
                v
            },
            120,
            40,
        )
        .join("\n");
        assert!(
            role_step.contains("Overrides built-in 'reviewer'"),
            "{role_step}"
        );

        let review = render_through_stack(
            || {
                let mut v = FleetSetupView::from_snapshot(snapshot());
                for _ in 0..3 {
                    v.handle_key(key(KeyCode::Down));
                }
                v.step = Step::Review;
                v
            },
            120,
            40,
        )
        .join("\n");
        assert!(review.contains("Overrides built-in 'reviewer'"), "{review}");

        // "custom" matches no roster member: no override note anywhere.
        let mut custom_view = FleetSetupView::from_snapshot(snapshot());
        for _ in 0..8 {
            custom_view.handle_key(key(KeyCode::Down));
        }
        assert_eq!(custom_view.selected_role(), "custom");
        assert!(custom_view.roster_override_note().is_none());
    }

    #[test]
    fn default_selection_targets_manager_inherit() {
        let view = FleetSetupView::from_snapshot(snapshot());
        let draft = view.starter_profile_draft();
        assert_eq!(draft.file_name(), "manager.toml");
        assert_eq!(draft.role_hint, "manager");
        assert!(draft.model.is_none());
        assert!(draft.model_class_hint.is_none());
        assert!(
            draft
                .instructions
                .as_deref()
                .is_some_and(|text| text.contains("assigned Fleet slice"))
        );
    }

    #[test]
    fn fleet_model_rows_keep_failed_provider_visible_with_reason() {
        let mut snap = snapshot();
        snap.available_models = vec![(
            "zai".to_string(),
            "glm-5.2".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::SavedLastCheckFailed {
                category: crate::error_taxonomy::ErrorCategory::Authentication,
                message: "auth failed".to_string(),
            },
        )];
        let mut view = FleetSetupView::from_snapshot(snap);
        assert_eq!(view.model_choices.len(), 2);
        assert!(
            view.model_choices[1]
                .summary
                .contains("last check failed (authentication)")
        );
        assert!(view.model_choices[1].summary.contains("auth failed"));
        assert_eq!(
            view.model_routes[1],
            ("zai".to_string(), "glm-5.2".to_string())
        );
        assert!(matches!(
            &view.model_row_states[1],
            FleetModelRowState::Blocked { reason } if reason == "auth failed"
        ));
        view.step = Step::Model;
        view.model_idx = 1;
        assert!(matches!(
            view.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(view.step, Step::Model);
    }

    #[test]
    fn fleet_invalid_route_stays_visible_but_cannot_advance() {
        let mut snap = snapshot();
        snap.available_models = vec![(
            "zai".to_string(),
            "broken-model".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::InvalidRoute,
        )];
        let mut view = FleetSetupView::from_snapshot(snap);
        view.step = Step::Model;
        view.model_idx = 1;

        assert!(view.model_choices[1].summary.contains("invalid route"));
        assert!(matches!(
            view.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(view.step, Step::Model);
    }

    #[test]
    fn fleet_includes_saved_model_outside_bundled_catalog() {
        let providers = crate::config::ProvidersConfig {
            openrouter: crate::config::ProviderConfig {
                api_key: Some("openrouter-test-key".to_string()),
                model: Some("acme/private-preview".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(providers),
            ..Default::default()
        };

        let routes = cross_provider_model_routes(
            &config,
            crate::config::ApiProvider::Openrouter,
            &crate::provider_readiness::ProviderReadinessSnapshot::default(),
        );

        assert!(routes.iter().any(|(provider, model, readiness)| {
            provider == "openrouter" && model == "acme/private-preview" && readiness.can_attempt()
        }));
        assert_eq!(
            routes
                .iter()
                .filter(|(provider, model, _)| {
                    provider == "openrouter" && model == "acme/private-preview"
                })
                .count(),
            1,
            "saved models must not be duplicated when the catalog later learns them"
        );
    }

    #[test]
    fn fleet_routes_and_saved_draft_keep_exact_named_custom_provider() {
        let mut custom = std::collections::HashMap::new();
        for (name, base_url, model) in [
            ("custom-a", "http://127.0.0.1:18181/v1", "model-a"),
            ("custom-b", "http://127.0.0.1:18182/v1", "model-b"),
        ] {
            custom.insert(
                name.to_string(),
                crate::config::ProviderConfig {
                    kind: Some("openai-compatible".to_string()),
                    base_url: Some(base_url.to_string()),
                    model: Some(model.to_string()),
                    api_key: Some("local-test-key".to_string()),
                    ..Default::default()
                },
            );
        }
        let config = Config {
            provider: Some("custom-a".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                custom,
                ..Default::default()
            }),
            ..Default::default()
        };
        let routes = cross_provider_model_routes(
            &config,
            crate::config::ApiProvider::Custom,
            &crate::provider_readiness::ProviderReadinessSnapshot::default(),
        );
        assert!(
            routes
                .iter()
                .any(|(provider, model, _)| { provider == "custom-a" && model == "model-a" })
        );
        assert!(
            routes
                .iter()
                .any(|(provider, model, _)| { provider == "custom-b" && model == "model-b" })
        );
        assert!(!routes.iter().any(|(provider, _, _)| provider == "custom"));

        let mut view = FleetSetupView::from_snapshot(FleetSetupSnapshot {
            available_models: routes,
            provider: "custom-a".to_string(),
            model: "model-a".to_string(),
            ..snapshot()
        });
        let route = view
            .model_routes
            .iter()
            .find(|(provider, model)| provider == "custom-b" && model == "model-b")
            .cloned()
            .expect("custom B route selectable while A is active");
        let draft = sample_draft();
        let (_, rendered) =
            view.install_model_draft(draft, "model-b".to_string(), Some(route), None);
        assert!(rendered.contains("provider = \"custom-b\""), "{rendered}");
    }

    #[test]
    fn fleet_routes_keep_legacy_literal_custom_without_named_tables() {
        let config = Config {
            provider: Some("custom".to_string()),
            base_url: Some("http://127.0.0.1:18080/v1".to_string()),
            api_key: Some("local-test-key".to_string()),
            default_text_model: Some("legacy-custom-model".to_string()),
            ..Default::default()
        };

        let routes = cross_provider_model_routes(
            &config,
            crate::config::ApiProvider::Custom,
            &crate::provider_readiness::ProviderReadinessSnapshot::default(),
        );

        assert!(
            routes.iter().any(|(provider, model, readiness)| {
                provider == "custom"
                    && model == "legacy-custom-model"
                    && matches!(
                        readiness,
                        crate::provider_readiness::ResolvedProviderReadiness::LocalUnchecked
                    )
                    && readiness.can_attempt()
            }),
            "{routes:?}"
        );
    }

    #[test]
    fn role_step_keeps_list_and_detail_separate_at_80_columns() {
        let rows = render_through_stack(|| FleetSetupView::from_snapshot(snapshot()), 80, 24);
        let text = rows.join("\n");

        let manager_row = rows
            .iter()
            .position(|row| row.contains("▸ manager"))
            .expect("manager row should render");
        let custom_row = rows
            .iter()
            .position(|row| row.contains("  custom"))
            .expect("custom row should render");
        let summary_row = rows
            .iter()
            .position(|row| row.contains("Plan & split queued work"))
            .expect("selected role summary should render");
        let description_row = rows
            .iter()
            .position(|row| row.contains("Coordinates the Fleet run"))
            .expect("selected role description should render");

        assert!(
            manager_row < custom_row,
            "expected the full role list before details:\n{text}"
        );
        assert!(
            custom_row < summary_row,
            "selected summary must not share a row with role names:\n{text}"
        );
        assert!(
            custom_row < description_row,
            "selected description must render below the list:\n{text}"
        );
        for row in &rows[manager_row..=custom_row] {
            assert!(
                !row.contains("Plan & split queued work")
                    && !row.contains("Coordinates the Fleet run")
                    && !row.contains("Fleet runs sub-agents"),
                "role list row contains detail copy at 80 columns: {row:?}\n{text}"
            );
        }
    }

    fn render_through_stack(view_at: impl Fn() -> FleetSetupView, w: u16, h: u16) -> Vec<String> {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        for y in 0..h {
            for x in 0..w {
                buf[(x, y)].set_symbol("X");
            }
        }
        let mut stack = ViewStack::new();
        stack.push(view_at());
        stack.render(area, &mut buf);
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn fleet_setup_is_usable_and_opaque_at_blocker_sizes() {
        // Exercise each step so all three screens are validated at every size.
        type Builder = (&'static str, fn() -> FleetSetupView);
        let builders: [Builder; 3] = [
            ("role", || FleetSetupView::from_snapshot(snapshot())),
            ("model", || {
                let mut v = FleetSetupView::from_snapshot(snapshot());
                v.step = Step::Model;
                v
            }),
            ("review", || {
                let mut v = FleetSetupView::from_snapshot(snapshot());
                v.step = Step::Review;
                v
            }),
        ];

        for (label, make) in builders {
            for (w, h) in BLOCKER_SIZES {
                let rows = render_through_stack(make, w, h);
                let text = rows.join("\n");

                // No bleed-through anywhere in the composited frame.
                assert!(
                    !text.contains('X'),
                    "{label} {w}x{h}: background bleed-through"
                );
                // Some action label is always visible.
                assert!(text.contains("cancel"), "{label} {w}x{h}: missing footer");
                // The first impression communicates Fleet = agent team.
                assert!(
                    text.contains("agent team"),
                    "{label} {w}x{h}: missing framing"
                );
                // No row overflows the frame width.
                for (y, row) in rows.iter().enumerate() {
                    assert!(
                        UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                        "{label} {w}x{h}: row {y} overflows: {row:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn review_at_cursor_size_keeps_content_and_actions_apart() {
        let rows = render_through_stack(
            || {
                let mut view = FleetSetupView::from_snapshot(snapshot());
                view.step = Step::Review;
                view
            },
            89,
            50,
        );
        let popup = centered_modal_area(Rect::new(0, 0, 89, 50), 96, 31, 60, 16);
        let review_row = rows
            .iter()
            .position(|row| row.contains("Review & save"))
            .expect("review heading");
        let review_col = rows[review_row]
            .chars()
            .position(|ch| ch == 'R')
            .expect("review heading column") as u16;
        assert!(
            review_col >= popup.x.saturating_add(2),
            "body copy must not touch the popup border: {:?}",
            rows[review_row]
        );

        let action_row = rows
            .iter()
            .rposition(|row| row.contains("cancel"))
            .expect("footer cancel action");
        let footer_row = rows[..action_row]
            .iter()
            .rposition(|row| row.contains("scroll"))
            .expect("footer shortcut row");
        assert!(footer_row > 0);
        let gutter = rows[footer_row - 1]
            .chars()
            .skip(usize::from(popup.x.saturating_add(1)))
            .take(usize::from(popup.width.saturating_sub(2)))
            .collect::<String>();
        assert!(
            gutter.trim().is_empty(),
            "review body needs a quiet row before the action rail: {gutter:?}"
        );
    }

    #[test]
    fn choice_steps_at_cursor_size_stay_content_sized() {
        for (step, expected_height) in [(Step::Role, 21usize), (Step::Model, 22usize)] {
            let rows = render_through_stack(
                || {
                    let mut view = FleetSetupView::from_snapshot(snapshot());
                    view.step = step;
                    view
                },
                89,
                50,
            );
            let top = rows
                .iter()
                .position(|row| row.contains("Fleet setup — your agent team"))
                .expect("fleet setup title");
            let bottom = rows
                .iter()
                .rposition(|row| row.contains("Step "))
                .expect("fleet setup step receipt");
            assert_eq!(
                bottom - top + 1,
                expected_height,
                "choice card should follow its content instead of filling the 89x50 frame"
            );
        }
    }

    #[test]
    fn review_lists_model_permissions_tools_and_profile_availability() {
        // Top of the review: the leading sections are visible without scrolling.
        let top = render_through_stack(
            || {
                let mut v = FleetSetupView::from_snapshot(snapshot());
                v.step = Step::Review;
                v
            },
            120,
            40,
        )
        .join("\n");
        for section in [
            "Role",
            "Model",
            "Profile availability",
            "Auth & readiness",
            "Permissions",
        ] {
            assert!(top.contains(section), "review missing section: {section}");
        }
        for truth in [
            "Scope changes discovery only",
            "trusted-path",
            "permission policy still",
            "govern execution",
        ] {
            assert!(
                top.contains(truth),
                "profile availability must not imply execution authority: {top}"
            );
        }

        // The review is intentionally scrollable; scrolling to the bottom reveals
        // the workspace/org execution policy, review policy, and honest save note.
        let bottom = render_through_stack(
            || {
                let mut v = FleetSetupView::from_snapshot(snapshot());
                v.step = Step::Review;
                v.review_scroll = 999; // clamps to max in render
                v
            },
            120,
            40,
        )
        .join("\n");
        for needle in [
            "Tools",
            "Workspace",
            "Review policy",
            "Press Enter or g once",
        ] {
            assert!(bottom.contains(needle), "scrolled review missing: {needle}");
        }

        let policy = FleetSetupView::from_snapshot(snapshot()).review_policy_summary();
        for truth in [
            "current interactive session",
            "codewhale fleet status",
            ".codewhale/fleet.jsonl",
        ] {
            assert!(policy.contains(truth), "review policy missing: {truth}");
        }
        assert!(
            !policy.contains("inspects the ledger"),
            "the interactive status command must not claim to inspect the durable ledger: {policy}"
        );
    }

    #[test]
    fn dormant_external_consent_row_requires_activation() {
        let mut snap = snapshot();
        snap.available_models = vec![(
            "openai-codex".to_string(),
            "gpt-5.6-sol".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::ExternalConsentPendingSelection,
        )];
        let view = FleetSetupView::from_snapshot(snap);
        assert!(
            view.model_choices[1]
                .summary
                .contains("external consent · select to check")
        );
        assert!(matches!(
            view.model_row_states[1],
            FleetModelRowState::NeedsActivation
        ));
    }

    #[test]
    fn enter_on_dormant_external_consent_emits_activation_event() {
        let mut snap = snapshot();
        snap.available_models = vec![(
            "openai-codex".to_string(),
            "gpt-5.6-terra".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::ExternalConsentPendingSelection,
        )];
        let mut view = FleetSetupView::from_snapshot(snap);
        view.handle_key(key(KeyCode::Enter)); // Role -> Model
        view.handle_key(key(KeyCode::Down)); // inherit -> codex row
        assert_eq!(
            view.selected_route(),
            Some(("openai-codex".to_string(), "gpt-5.6-terra".to_string()))
        );
        let action = view.handle_key(key(KeyCode::Enter));
        let ViewAction::Emit(ViewEvent::FleetSetupExternalConsentActivationRequested {
            provider_id,
            model,
        }) = action
        else {
            panic!("expected external-consent activation request, got {action:?}");
        };
        assert_eq!(provider_id, "openai-codex");
        assert_eq!(model, "gpt-5.6-terra");
        assert_eq!(
            view.step,
            Step::Model,
            "stays on Model step until host validates"
        );
    }

    #[test]
    fn refresh_from_snapshot_makes_activated_row_ready() {
        let mut snap = snapshot();
        snap.available_models = vec![(
            "xai".to_string(),
            "grok-4.5".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::ExternalConsentPendingSelection,
        )];
        let mut view = FleetSetupView::from_snapshot(snap);
        view.handle_key(key(KeyCode::Enter)); // Role -> Model
        view.handle_key(key(KeyCode::Down)); // xai row
        assert!(matches!(
            view.model_row_states[1],
            FleetModelRowState::NeedsActivation
        ));

        // Simulate the host validating the route and rebuilding the snapshot:
        // the same row is now Ready.
        let mut refreshed = snapshot();
        refreshed.available_models = vec![(
            "xai".to_string(),
            "grok-4.5".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::Ready,
        )];
        view.refresh_from_snapshot(refreshed);

        assert!(matches!(
            view.model_row_states[1],
            FleetModelRowState::Ready
        ));
        // Selection and step are preserved.
        assert_eq!(view.step, Step::Model);
        assert_eq!(
            view.selected_route(),
            Some(("xai".to_string(), "grok-4.5".to_string()))
        );
    }

    #[test]
    fn blocked_row_cannot_advance() {
        let mut snap = snapshot();
        snap.available_models = vec![(
            "xai".to_string(),
            "grok-4.5".to_string(),
            crate::provider_readiness::ResolvedProviderReadiness::MissingKey,
        )];
        let mut view = FleetSetupView::from_snapshot(snap);
        view.step = Step::Model;
        view.model_idx = 1;
        assert!(matches!(
            &view.model_row_states[1],
            FleetModelRowState::Blocked { reason } if reason == "missing API key"
        ));
        assert!(matches!(
            view.handle_key(key(KeyCode::Enter)),
            ViewAction::None
        ));
        assert_eq!(view.step, Step::Model);
    }

    #[test]
    fn fleet_setup_includes_openai_codex_account_roster_with_dormant_consent() {
        let _env = crate::test_support::lock_test_env();
        let codex_home = tempfile::tempdir().expect("Codex home");
        let _home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
        std::fs::write(
            codex_home.path().join("models_cache.json"),
            serde_json::to_vec(&serde_json::json!({
                "fetched_at": chrono::Utc::now(),
                "models": [
                    { "slug": "gpt-5.6-sol", "priority": 1 },
                    { "slug": "gpt-5.6-terra", "priority": 2 },
                    { "slug": "gpt-5.6-luna", "priority": 3 }
                ]
            }))
            .expect("serialize cache"),
        )
        .expect("write cache");

        let mut config = crate::config::Config::default();
        config.providers = Some(crate::config::ProvidersConfig {
            openai_codex: crate::config::ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                external_credentials: Some(
                    codewhale_config::ExternalCredentialConsentToml::read_only(
                        codewhale_config::ProviderKind::OpenaiCodex,
                        codewhale_config::ExternalCredentialSource::CodexCli,
                        codex_home.path().join("auth.json"),
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        });

        let routes = cross_provider_model_routes(
            &config,
            crate::config::ApiProvider::Moonshot,
            &crate::provider_readiness::ProviderReadinessSnapshot::default(),
        );

        for model in ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"] {
            assert!(
                routes.iter().any(|(provider, m, readiness)| {
                    provider == "openai-codex"
                        && m == model
                        && matches!(
                            readiness,
                            crate::provider_readiness::ResolvedProviderReadiness::ExternalConsentPendingSelection
                        )
                }),
                "missing dormant-consent Codex route for {model}: {routes:?}"
            );
        }
    }

    #[test]
    fn fleet_setup_includes_xai_grok_routes_with_dormant_consent() {
        let _env = crate::test_support::lock_test_env();
        let grok_home = tempfile::tempdir().expect("Grok home");
        let mut config = crate::config::Config::default();
        config.providers = Some(crate::config::ProvidersConfig {
            xai: crate::config::ProviderConfig {
                auth_mode: Some("oauth".to_string()),
                external_credentials: Some(
                    codewhale_config::ExternalCredentialConsentToml::read_only(
                        codewhale_config::ProviderKind::Xai,
                        codewhale_config::ExternalCredentialSource::GrokCli,
                        grok_home.path().join("grok-auth.json"),
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        });

        let routes = cross_provider_model_routes(
            &config,
            crate::config::ApiProvider::Moonshot,
            &crate::provider_readiness::ProviderReadinessSnapshot::default(),
        );

        let xai_rows: Vec<_> = routes
            .iter()
            .filter(|(provider, _, _)| provider == "xai")
            .collect();
        assert!(
            !xai_rows.is_empty(),
            "xAI routes must be offered when Grok CLI consent is configured: {routes:?}"
        );
        assert!(
            xai_rows.iter().all(|(_, _, readiness)| {
                matches!(
                    readiness,
                    crate::provider_readiness::ResolvedProviderReadiness::ExternalConsentPendingSelection
                )
            }),
            "every xAI row must require explicit activation: {xai_rows:?}"
        );
    }
}
