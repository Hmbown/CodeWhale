//! `/fleet setup` — a progressive "set up your agent team" flow.
//!
//! Replaces the old six-column config matrix (#3791). Fleet is presented as an
//! agent team: the user makes one focused choice at a time (a role, then a model
//! class) and then reviews the full posture — model/route, permissions, tools,
//! workspace/org scope, and review policy — before starting. "Start" previews a
//! deterministic starter TOML profile; nothing is written until the user
//! explicitly ratifies the exact rendered bytes.
//!
//! NOTE (audit #7 / #3167): the role/model taxonomy and copy below are
//! intentionally English for now; #3167 reworks this into an interactive
//! provider/model picker that will churn most of this text. The command entry
//! (`CmdFleetDescription`) is already localized.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::localization::{MessageId, tr};
use crate::palette;
use crate::tui::app::App;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_chrome,
    render_panel_scrollbar, truncate_view_text,
};

const PROFILE_DIR: &str = ".codewhale/agents";

/// A selectable choice in a wizard step: a short identifier `label`, a one-line
/// `summary`, and a longer `description` shown (wrapped) in the detail pane.
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
const ROLES: [Choice; 8] = [
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
        "Reuse the active provider, model, and reasoning for this worker — the operator's route. Recommended default.",
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
        summary: Cow::Borrowed("Let CodeWhale choose"),
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
    token_budget: Option<u64>,
    api_timeout_secs: u64,
    heartbeat_timeout_secs: u64,
    /// Lowercased roster member ids with their origin labels (built-in /
    /// config / project), so the wizard can say when a chosen role would
    /// override an existing roster member.
    roster_members: Vec<(String, String)>,
    /// `(canonical provider id, model id)` pairs selectable for a worker,
    /// drawn from ALL configured providers — not only the active one (#4093).
    /// Shown after `inherit` in the Model step so a Fleet worker can be pinned
    /// to a route independent of the parent/current provider. The provider id
    /// is the canonical [`crate::config::ApiProvider::as_str`] identifier
    /// (e.g. `"deepseek"`), not a display label — see
    /// [`cross_provider_model_routes`].
    available_models: Vec<(String, String)>,
}

impl FleetSetupSnapshot {
    #[must_use]
    pub fn from_app(app: &App, config: &Config) -> Self {
        let provider = app.api_provider.display_name().to_string();
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

        Self {
            workspace: app.workspace.clone(),
            locale: app.ui_locale,
            provider_ready: crate::config::has_api_key_for(config, app.api_provider),
            provider,
            model,
            reasoning: app.reasoning_effort_display_label(),
            subagents_enabled: config.subagents_enabled_for_provider(app.api_provider),
            max_subagents: config.max_subagents_for_provider(app.api_provider),
            launch_concurrency: config.launch_concurrency_for_provider(app.api_provider),
            max_admitted: config.max_admitted_subagents_for_provider(app.api_provider),
            subagent_spawn_depth: config.subagent_max_spawn_depth_for_provider(app.api_provider),
            fleet_spawn_depth,
            token_budget: config.subagent_token_budget_for_provider(app.api_provider),
            api_timeout_secs: config.subagent_api_timeout_secs_for_provider(app.api_provider),
            heartbeat_timeout_secs: config
                .subagent_heartbeat_timeout_secs_for_provider(app.api_provider),
            roster_members,
            available_models: cross_provider_model_routes(config, app.api_provider),
        }
    }
}

/// Build the `(canonical provider id, model id)` pairs selectable for a worker
/// from EVERY configured provider — not only the active one (#4093). Fleet
/// workers can be pinned to a route independent of the parent/current provider,
/// so the Model step must offer the same cross-provider catalog the model
/// picker does, instead of the active provider's models alone.
///
/// The provider id here is the canonical [`crate::config::ApiProvider::as_str`]
/// identifier (e.g. `"deepseek"`), not a display label — this is the exact
/// value persisted into the saved profile's `provider` field and read back by
/// the loader (#4093), so it must round-trip through `ApiProvider::parse`.
/// Callers derive a human-readable label from it for UI text.
fn cross_provider_model_routes(
    config: &Config,
    active: crate::config::ApiProvider,
) -> Vec<(String, String)> {
    let mut routes = Vec::new();
    for provider in crate::provider_lake::configured_providers(config, active) {
        for model in crate::provider_lake::models_for_provider(config, active, provider) {
            routes.push((provider.as_str().to_string(), model));
        }
    }
    routes
}

/// Human-readable label for a canonical provider id, falling back to the raw
/// id verbatim when it doesn't parse (defensive — every id this module hands
/// out itself comes from [`crate::config::ApiProvider::as_str`], so this only
/// matters for a foreign/stale id read back from an old snapshot).
fn provider_display_label(provider_id: &str) -> String {
    crate::config::ApiProvider::parse(provider_id)
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
    /// Pick the saved thinking tier.
    Thinking,
    /// Review the full posture and start.
    Review,
}

pub struct FleetSetupView {
    snapshot: FleetSetupSnapshot,
    step: Step,
    role_idx: usize,
    model_idx: usize,
    thinking_idx: usize,
    review_scroll: usize,
    /// A model-drafted profile awaiting ratification (already sanitized and
    /// bounded by the untrusted gate). Cleared when the selection changes so
    /// a stale draft can never be ratified against fresh answers.
    model_draft: Option<Box<crate::fleet::profile::FleetProfileDraft>>,
    /// Display label of the model that authored `model_draft`.
    model_draft_label: Option<String>,
    /// Exact rendered TOML preview for `model_draft` (header comment + the
    /// deterministic bytes ratifying would persist). Rendered inline on the
    /// Review step — never in a separate pager (#4093): a standalone pager
    /// view owns its own `g`/`G` scroll bindings, which silently swallowed
    /// the ratify keypress and left users unable to save without first
    /// pressing Esc. Keeping the preview and the ratify control in the same
    /// view means the footer's `g`/Enter hints are never a lie.
    model_draft_preview: Option<String>,
    /// Model-step rows: `inherit` followed by one row per concrete model from
    /// every configured provider (#4093).
    model_choices: Vec<Choice>,
    /// `(provider, model)` aligned with `model_choices`. Index 0 is `inherit`
    /// (the active route); later rows pin a concrete, possibly cross-provider
    /// route. Drives the review/copy so a pinned route names its own provider.
    model_routes: Vec<(String, String)>,
}

impl FleetSetupView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        Self::from_snapshot(FleetSetupSnapshot::from_app(app, config))
    }

    fn from_snapshot(snapshot: FleetSetupSnapshot) -> Self {
        let mut model_choices = vec![MODEL_INHERIT];
        // `inherit` (index 0) maps to the active route; every later row pins a
        // concrete (provider, model) drawn from all configured providers.
        let mut model_routes = vec![(snapshot.provider.clone(), snapshot.model.clone())];
        for (provider, model) in &snapshot.available_models {
            let provider_label = provider_display_label(provider);
            model_choices.push(Choice {
                label: Cow::Owned(model.clone()),
                summary: Cow::Owned(format!("Pin this model ({provider_label})")),
                description: Cow::Owned(format!(
                    "Route this worker to {model} on {provider_label} instead of inheriting the session route."
                )),
            });
            // Canonical provider id (not the display label above) — this is
            // what gets persisted into the saved profile (#4093).
            model_routes.push((provider.clone(), model.clone()));
        }
        Self {
            snapshot,
            step: Step::Role,
            role_idx: 0,
            model_idx: 0,
            thinking_idx: 0,
            review_scroll: 0,
            model_draft: None,
            model_draft_label: None,
            model_draft_preview: None,
            model_choices,
            model_routes,
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
        let content = format!("{header}{}", draft.render_toml());
        self.model_draft = Some(draft);
        self.model_draft_label = Some(model_label);
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
            .map(|(id, origin)| format!("Overrides the {origin} '{id}' roster member."))
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
        if self.model_idx == 0 {
            return None;
        }
        self.model_routes.get(self.model_idx).cloned()
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

    /// Number of selectable rows on the current step (0 on the review step).
    fn step_len(&self) -> usize {
        match self.step {
            Step::Role => ROLES.len(),
            Step::Model => self.model_choices.len(),
            Step::Thinking => THINKING_CHOICES.len(),
            Step::Review => 0,
        }
    }

    fn move_up(&mut self) {
        match self.step {
            Step::Role => {
                self.role_idx = self.role_idx.saturating_sub(1);
                self.discard_model_draft();
            }
            Step::Model => {
                self.model_idx = self.model_idx.saturating_sub(1);
                self.discard_model_draft();
            }
            Step::Thinking => {
                self.thinking_idx = self.thinking_idx.saturating_sub(1);
                self.discard_model_draft();
            }
            Step::Review => self.review_scroll = self.review_scroll.saturating_sub(1),
        }
    }

    /// A draft is only valid for the answers it was requested against.
    fn discard_model_draft(&mut self) {
        self.model_draft = None;
        self.model_draft_label = None;
        self.model_draft_preview = None;
    }

    fn move_down(&mut self) {
        match self.step {
            Step::Role => {
                self.role_idx = (self.role_idx + 1).min(self.step_len().saturating_sub(1));
                self.discard_model_draft();
            }
            Step::Model => {
                self.model_idx = (self.model_idx + 1).min(self.step_len().saturating_sub(1));
                self.discard_model_draft();
            }
            Step::Thinking => {
                self.thinking_idx = (self.thinking_idx + 1).min(self.step_len().saturating_sub(1));
                self.discard_model_draft();
            }
            Step::Review => self.review_scroll = self.review_scroll.saturating_add(1),
        }
    }

    /// Advance to the next step, or — on the review step — preview the exact
    /// starter profile TOML the next ratify keypress would persist.
    fn advance(&mut self) -> ViewAction {
        match self.step {
            Step::Role => {
                self.step = Step::Model;
                ViewAction::None
            }
            Step::Model => {
                self.step = Step::Thinking;
                ViewAction::None
            }
            Step::Thinking => {
                self.step = Step::Review;
                self.review_scroll = 0;
                ViewAction::None
            }
            Step::Review => self.preview_starter_profile_action(),
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
            Step::Thinking => {
                self.step = Step::Model;
                ViewAction::None
            }
            Step::Review => {
                self.step = Step::Thinking;
                ViewAction::None
            }
        }
    }

    /// Preview the exact starter profile TOML the next ratify keypress would
    /// persist. Renders inline within the Review step's own scrollable pane —
    /// deliberately NOT via `ViewEvent::OpenTextPager` (#4093): a standalone
    /// pager view has its own `g`/`G` scroll bindings and would swallow the
    /// ratify keypress, forcing an Esc-then-g round trip to actually save.
    fn preview_starter_profile_action(&mut self) -> ViewAction {
        let draft = self.starter_profile_draft();
        let header = tr(self.snapshot.locale, MessageId::FleetPreviewHeader)
            .replace("{name}", &draft.file_name());
        self.model_draft_preview = Some(format!("{header}{}", draft.render_toml()));
        self.model_draft = Some(draft);
        self.model_draft_label = Some("CodeWhale starter".to_string());
        self.review_scroll = 0;
        ViewAction::None
    }

    /// Build a deterministic starter profile for the current role/model
    /// selection. The same ratify event persists this as model-drafted profiles,
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
                hints.push(ActionHint::new("Enter", "next"));
                hints.push(ActionHint::new("←", "back"));
            }
            Step::Thinking => {
                hints.push(ActionHint::new("↑/↓", "choose"));
                hints.push(ActionHint::new("Enter", "next"));
                hints.push(ActionHint::new("←", "back"));
            }
            Step::Review => {
                hints.push(ActionHint::new("↑/↓", "scroll"));
                if self.model_draft.is_some() {
                    hints.push(ActionHint::new("Enter", "ratify"));
                    hints.push(ActionHint::new("g", "ratify draft"));
                    hints.push(ActionHint::new("m", "redraft"));
                } else if self.snapshot.provider_ready {
                    hints.push(ActionHint::new("Enter", "preview"));
                    hints.push(ActionHint::new("m", "model draft"));
                } else {
                    hints.push(ActionHint::new("Enter", "preview"));
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

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
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
            KeyCode::Char('g') if self.step == Step::Review => match self.model_draft.clone() {
                Some(draft) => {
                    ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft })
                }
                None => ViewAction::None,
            },
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
                if self.step == Step::Review && self.model_draft.is_some() =>
            {
                // A ratify-ready draft is on screen; Enter should ratify it,
                // not silently start the manual profile-prompt flow and drop
                // the draft.
                match self.model_draft.clone() {
                    Some(draft) => {
                        ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested {
                            draft,
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
        let step_no = match self.step {
            Step::Role => 1,
            Step::Model => 2,
            Step::Thinking => 3,
            Step::Review => 4,
        };
        let hints = self.footer_hints();
        let chrome = render_modal_chrome(
            area,
            buf,
            Line::from(Span::styled(
                format!(" Fleet setup — your agent team · Step {step_no}/4 "),
                Style::default()
                    .fg(palette::WHALE_ACCENT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            96,
            30,
            60,
            16,
            &hints,
        );
        let content = chrome.body;

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
                render_choice_step(chunks[1], buf, &ROLES, self.role_idx, &context)
            }
            Step::Model => render_choice_step(
                chunks[1],
                buf,
                &self.model_choices,
                self.model_idx,
                &[
                    format!(
                        "Current route: {} / {}  ·  reasoning {}",
                        self.snapshot.provider, self.snapshot.model, self.snapshot.reasoning
                    ),
                    match self.selected_model() {
                        Some(model) => format!("This worker will run on {model}."),
                        None => "This worker inherits your current route.".to_string(),
                    },
                ],
            ),
            Step::Thinking => render_choice_step(
                chunks[1],
                buf,
                THINKING_CHOICES,
                self.thinking_idx,
                &[
                    format!("Current reasoning: {}", self.snapshot.reasoning),
                    format!("This worker will use {}.", self.selected_thinking_label()),
                ],
            ),
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
            Step::Thinking => (
                "Choose thinking",
                "Pick this worker's reasoning tier, or inherit your current setting.",
            ),
            Step::Review if self.model_draft.is_some() => (
                "Ratify the draft",
                "Exact TOML shown below. Press Enter or g to ratify, m to redraft.",
            ),
            Step::Review => (
                "Review & start",
                "Confirm the posture below, preview exact TOML, then ratify to save.",
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
        let (profile_value, _) = profile_file_status(&self.snapshot.workspace);
        let file_stem = profile_file_stem(&role.label);
        let token_budget = self
            .snapshot
            .token_budget
            .map(|budget| format!("{budget} tokens"))
            .unwrap_or_else(|| "unbounded".to_string());

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
                    format!("{model}  ·  provider {}", provider_display_label(&provider))
                }
                None => format!(
                    "inherit  ·  route {} / {}, reasoning {}",
                    self.snapshot.provider, self.snapshot.model, self.snapshot.reasoning
                ),
            },
        );
        section(&mut lines, "Thinking", self.selected_thinking_label());
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
        section(
            &mut lines,
            "Review policy",
            format!(
                "Budget {token_budget} · {}s api, {}s heartbeat. Fleet -> exec runs the workers; /fleet status (or /subagents) inspects the ledger.",
                self.snapshot.api_timeout_secs, self.snapshot.heartbeat_timeout_secs
            ),
        );
        section(
            &mut lines,
            "Profile",
            format!(
                "{PROFILE_DIR}/{file_stem}.toml  ·  {profile_value} present. Start previews a deterministic starter profile; nothing is written to disk until ratification.",
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
    render_panel_scrollbar(area, buf, visual_rows, usize::from(area.height), scroll);
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
        let list_height = (choices.len() as u16 + 1).min(area.height.saturating_sub(1).max(1));
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(list_height), Constraint::Min(1)])
            .split(area);
        (rows[0], rows[1])
    };

    // List: labels are identifiers, so a `>`-marked single line each is safe.
    let list_width = usize::from(list_area.width);
    let mut list_lines: Vec<Line> = Vec::with_capacity(choices.len());
    for (idx, choice) in choices.iter().enumerate() {
        let is_selected = idx == selected;
        let pointer = if is_selected { "> " } else { "  " };
        let style = if is_selected {
            Style::default()
                .fg(palette::SELECTION_TEXT)
                .bg(palette::SELECTION_BG)
                .add_modifier(Modifier::BOLD)
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
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
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

fn profile_file_status(workspace: &Path) -> (String, String) {
    let dir = workspace.join(PROFILE_DIR);
    if !dir.exists() {
        return (
            "0 files".to_string(),
            format!("create {PROFILE_DIR}/*.toml"),
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
        ("1 file".to_string(), PROFILE_DIR.to_string())
    } else {
        (format!("{count} files"), PROFILE_DIR.to_string())
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

    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

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
            token_budget: Some(100_000),
            api_timeout_secs: 120,
            heartbeat_timeout_secs: 300,
            roster_members: crate::fleet::roster::FleetRoster::built_ins_only()
                .members()
                .iter()
                .map(|member| (member.id.to_lowercase(), member.origin.to_string()))
                .collect(),
            available_models: vec![
                ("deepseek".to_string(), "deepseek-v4-pro".to_string()),
                ("deepseek".to_string(), "deepseek-v4-flash".to_string()),
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

    fn to_review(view: &mut FleetSetupView) {
        view.handle_key(key(KeyCode::Enter));
        view.handle_key(key(KeyCode::Enter));
        view.handle_key(key(KeyCode::Enter));
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
        snap.available_models = vec![("zai".to_string(), "glm-5.2".to_string())];
        let mut view = FleetSetupView::from_snapshot(snap);

        // Role step: keep the first role. Model step: inherit(0), then the one
        // cross-provider row (1) -> pick it. Then advance to Review.
        view.handle_key(key(KeyCode::Enter)); // Role -> Model
        view.handle_key(key(KeyCode::Down)); // -> the zai/glm-5.2 row
        assert_eq!(
            view.selected_route(),
            Some(("zai".to_string(), "glm-5.2".to_string()))
        );
        view.handle_key(key(KeyCode::Enter)); // Model -> Thinking
        while view.selected_reasoning_effort().as_deref() != Some("max") {
            view.handle_key(key(KeyCode::Down));
        }
        view.handle_key(key(KeyCode::Enter)); // Thinking -> Review

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
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft }) =
            action
        else {
            panic!("expected ratify commit event");
        };
        assert_eq!(draft.provider.as_deref(), Some("zai"));
        assert_eq!(draft.model.as_deref(), Some("glm-5.2"));
        assert_eq!(draft.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn ratify_is_inert_without_a_draft_and_commits_with_one() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);

        // No draft installed: g does nothing, m is the offered action.
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('g'))),
            ViewAction::None
        ));

        let (title, content) =
            view.install_model_draft(sample_draft(), "GLM-5.2".to_string(), None, None);
        assert!(title.contains("GLM-5.2"));
        assert!(content.contains("id = \"reviewer\""), "{content}");
        assert!(content.contains("Nothing is saved until"), "{content}");

        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft }) =
            action
        else {
            panic!("expected ratify commit event");
        };
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
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('g'))),
            ViewAction::None
        ));
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
        assert_eq!(view.step, Step::Thinking);

        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.thinking_idx, 1);

        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.step, Step::Review);

        // Left steps back through the wizard.
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.step, Step::Thinking);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.step, Step::Model);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.step, Step::Role);
    }

    #[test]
    fn esc_cancels_from_any_step() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        view.handle_key(key(KeyCode::Enter)); // -> Model
        let action = view.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn start_on_review_previews_inline_and_ratifies_starter_profile_for_selection() {
        let mut view = FleetSetupView::from_snapshot(snapshot());
        // Role: manager(0) scout(1) builder(2) -> builder.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter)); // -> Model
        // Model: inherit(0) deepseek-v4-pro(1) -> deepseek-v4-pro.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Enter)); // -> Thinking
        while view.selected_reasoning_effort().as_deref() != Some("max") {
            view.handle_key(key(KeyCode::Down));
        }
        view.handle_key(key(KeyCode::Enter)); // -> Review

        // Start previews inline (#4093: no separate pager to steal the next
        // ratify keypress) — the action stays `None` and the draft/preview
        // land directly on this same view.
        let action = view.handle_key(key(KeyCode::Enter)); // Start
        assert!(matches!(action, ViewAction::None));
        assert!(view.model_draft.is_some());
        let content = view
            .model_draft_preview
            .as_deref()
            .expect("preview installed inline");
        assert!(content.contains("# .codewhale/agents/builder.toml"));
        assert!(content.contains("id = \"builder\""));
        assert!(content.contains("role_hint = \"builder\""));
        assert!(content.contains("model = \"deepseek-v4-pro\""));
        assert!(content.contains("reasoning_effort = \"max\""));
        // A concrete cross-provider route pin names its own provider
        // explicitly (#4093) — the saved profile must not be ambiguously
        // scoped to whatever provider happens to be active at launch time.
        assert!(content.contains("provider = \"deepseek\""), "{content}");
        assert!(content.contains("Nothing is saved until"));
        for forbidden in ["base_url", "api_key"] {
            assert!(
                !content.contains(forbidden),
                "starter profile must not carry {forbidden}: {content}"
            );
        }

        // `g` ratifies directly from this same view — no Esc-then-g round
        // trip through a separate pager required.
        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::FleetProfileDraftCommitRequested { draft }) =
            action
        else {
            panic!("expected ratified starter draft");
        };
        assert_eq!(draft.id, "builder");
        assert_eq!(draft.role_hint, "builder");
        assert_eq!(draft.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(draft.provider.as_deref(), Some("deepseek"));
        assert_eq!(draft.reasoning_effort.as_deref(), Some("max"));
    }

    #[test]
    fn inherit_selection_starter_draft_carries_no_provider() {
        // `inherit` (no concrete route pin) must never carry a provider —
        // there's no explicit route to name (#4093).
        let mut view = FleetSetupView::from_snapshot(snapshot());
        to_review(&mut view);
        view.handle_key(key(KeyCode::Enter)); // Start -> preview inherit draft
        let draft = view.model_draft.as_deref().expect("draft installed");
        assert_eq!(draft.model, None);
        assert_eq!(draft.provider, None);
        assert_eq!(draft.reasoning_effort, None);
        let content = view.model_draft_preview.as_deref().unwrap();
        assert!(!content.contains("provider"), "{content}");
        assert!(!content.contains("reasoning_effort"), "{content}");
    }

    #[test]
    fn role_and_review_steps_note_roster_overrides() {
        // "reviewer" (index 4) collides with the built-in roster member; the
        // role step context and review Role section must both say so.
        let mut view = FleetSetupView::from_snapshot(snapshot());
        for _ in 0..3 {
            view.handle_key(key(KeyCode::Down));
        }
        assert_eq!(view.selected_role(), "reviewer");
        assert_eq!(
            view.roster_override_note().as_deref(),
            Some("Overrides the built-in 'reviewer' roster member.")
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
            role_step.contains("Overrides the built-in 'reviewer'"),
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
        assert!(
            review.contains("Overrides the built-in 'reviewer'"),
            "{review}"
        );

        // "custom" matches no roster member: no override note anywhere.
        let mut custom_view = FleetSetupView::from_snapshot(snapshot());
        for _ in 0..7 {
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
    fn role_step_keeps_list_and_detail_separate_at_80_columns() {
        let rows = render_through_stack(|| FleetSetupView::from_snapshot(snapshot()), 80, 24);
        let text = rows.join("\n");

        let manager_row = rows
            .iter()
            .position(|row| row.contains("> manager"))
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
    fn review_lists_model_permissions_tools_and_scope() {
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
        for section in ["Role", "Model", "Permissions", "Tools"] {
            assert!(top.contains(section), "review missing section: {section}");
        }

        // The review is intentionally scrollable; scrolling to the bottom reveals
        // the workspace/org scope, review policy, and the honest ratification
        // note on the Start action.
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
        for needle in ["Workspace", "Review policy", "until ratification"] {
            assert!(bottom.contains(needle), "scrolled review missing: {needle}");
        }
    }
}
