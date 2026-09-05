//! `/model` picker modal: pick a model and thinking-effort tier (#39, #2026).
//!
//! The picker intentionally presents model and thinking as independent choices
//! instead of collapsing them into preset route names. The "auto" option is
//! always available; custom (unrecognized) model ids appear as a separate row.
//! Pass-through providers fall back to only "auto" plus the current custom row.
//!
//! On apply we emit a [`ViewEvent::ModelPickerApplied`] with the resolved
//! model id and effort tier.

use std::cell::RefCell;
use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};

use codewhale_config::catalog::CatalogSource;
use codewhale_config::model_reference::ModelReferenceCard;
use codewhale_config::pricing::OfferingPricing;

use crate::codex_model_cache::{
    self, CodexModelCacheFreshness, CodexModelMetadata, CodexModelRoster,
};
use crate::config::{ApiProvider, Config, DEEPSEEK_ALIAS_REPLACEMENT};
use crate::localization::{Locale, MessageId, tr};
use crate::model_profile::{
    CapabilityOverride, SupportState, resolved_capability_profile_for_route_with_overrides,
    resolved_capability_profile_with_overrides,
};
use crate::model_registry;
use crate::models_dev_live::{self, ModelsDevFreshness};
use crate::palette;
use crate::provider_lake::{
    all_catalog_models_for_provider, catalog_offering_for_model, configured_providers,
};
use crate::settings::PinnedModel;
use crate::tui::app::{App, ReasoningEffort};
use crate::tui::menu_style;
use crate::tui::views::{
    ActionHint, ListDetailLayout, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_footer,
    render_underwater_surface,
};

/// Thinking-effort rows shown for DeepSeek-style providers, in the order
/// DeepSeek behaviorally distinguishes them.
const DEFAULT_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Auto,
    ReasoningEffort::Off,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];
/// First-party DeepSeek routes document a real `low` wire tier alongside
/// `high`/`max` (#52), so their picker exposes the cheaper tier the generic
/// default list cannot claim for routes where low collapses onto high.
const DEEPSEEK_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Auto,
    ReasoningEffort::Off,
    ReasoningEffort::Low,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];
/// Kimi Code K3 accepts route-specific low and medium controls at the
/// official membership endpoint. Medium becomes K3's nested high wire effort,
/// but keeping the selected intent visible is important for recovery and
/// route receipts.
const KIMI_CODE_K3_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Auto,
    ReasoningEffort::Off,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];
const CODEX_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];
/// Auto model routing has no concrete provider dialect yet, so retain the
/// complete preference vocabulary and defer normalization to dispatch.
const AUTO_MODEL_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Auto,
    ReasoningEffort::Off,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];

/// `/model` catalog views (#4115).
///
/// Configured stays the calm default. Typing searches every provider and a
/// cross-provider selection switches its route transactionally, so `/provider`
/// is never a prerequisite. Discoverability views (Recent / Coding / Cheap /
/// Long context) never auto-select a surprising route — the active model
/// remains the selection until the operator moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelListView {
    Configured,
    Catalog,
    Recent,
    Coding,
    Cheap,
    LongContext,
}

impl ModelListView {
    const ALL: [Self; 6] = [
        Self::Configured,
        Self::Catalog,
        Self::Recent,
        Self::Coding,
        Self::Cheap,
        Self::LongContext,
    ];

    fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|view| *view == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    fn from_memory_name(name: &str) -> Option<Self> {
        match name {
            "configured" => Some(Self::Configured),
            "catalog" => Some(Self::Catalog),
            "recent" => Some(Self::Recent),
            "coding" => Some(Self::Coding),
            "cheap" => Some(Self::Cheap),
            "long_context" => Some(Self::LongContext),
            _ => None,
        }
    }

    fn memory_name(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Catalog => "catalog",
            Self::Recent => "recent",
            Self::Coding => "coding",
            Self::Cheap => "cheap",
            Self::LongContext => "long_context",
        }
    }

    /// Short chrome / action label for this view.
    fn title_label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Catalog => "catalog",
            Self::Recent => "recent",
            Self::Coding => "coding",
            Self::Cheap => "cheap",
            Self::LongContext => "long ctx",
        }
    }

    /// Views that browse beyond the conservative configured-provider set.
    fn is_discoverability(self) -> bool {
        !matches!(self, Self::Configured)
    }

    fn browses_all_providers(self) -> bool {
        self.is_discoverability()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Model,
    Effort,
}

#[derive(Debug, Clone, Copy)]
struct PaneRenderState {
    pane: Pane,
    selected: usize,
    focused: bool,
}

pub struct ModelPickerView {
    initial_model: String,
    /// Exact runtime value before the picker opened. Keep this raw so choosing
    /// the canonical replacement for a retired alias performs a real migration
    /// instead of being misclassified as "unchanged".
    previous_model: String,
    initial_provider: ApiProvider,
    /// Raw preference before the picker opened. An absent explicit preference
    /// is represented by Auto so applying a visible fixed-route tier is still
    /// recognized as an intentional picker choice.
    initial_effort: ReasoningEffort,
    /// Working raw preference. Model-row navigation only changes how this is
    /// projected into the visible route-specific effort rows.
    selected_effort_request: ReasoningEffort,
    active_accepts_custom_model_ids: bool,
    query: String,
    /// Working selection (separate from the initial values so we can offer a
    /// clean Esc-to-cancel without mutating App state).
    selected_model_idx: usize,
    selected_effort_idx: usize,
    focus: Pane,
    /// True when the active model is one we don't list — we still show it
    /// so the picker doesn't quietly forget the user's chosen IDs.
    show_custom_model_row: bool,
    model_rows: Vec<ModelPickerRow>,
    /// Static route facts used to validate custom/current rows at apply time.
    route_config: Config,
    /// Session-local provider checks used by custom/current rows. Catalog rows
    /// resolve the same snapshot during construction.
    provider_health: crate::provider_readiness::ProviderReadinessSnapshot,
    view: ModelListView,
    /// Other providers considered "configured" (#3830), shown by default
    /// alongside `initial_provider`'s own rows without requiring the user to
    /// type a search query first. Uses the same definition as the
    /// `/provider` manager's default view
    /// (`crate::config::provider_is_configured_for_active`): active
    /// provider, working credentials/OAuth, or an explicit
    /// `[providers.<name>]` entry. Self-hosted providers (Ollama/Sglang/
    /// Vllm) don't qualify just because routing to them doesn't require a
    /// key.
    configured_providers: Vec<ApiProvider>,
    row_hitboxes: RefCell<Vec<(Rect, Pane, usize)>>,
    last_mouse_selected: Option<(Pane, usize)>,
    /// UI locale captured from the app at construction (#4057 wave 2).
    locale: Locale,
    pinned_models: Vec<PinnedModel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPickerRow {
    id: String,
    provider: Option<ApiProvider>,
    /// Concrete persistence identity. `Custom` alone cannot identify a named
    /// custom route, so pins must carry this exact key when present.
    provider_identity: Option<String>,
    hint: String,
    metadata: EffectivePickerMetadata,
    selectable: bool,
    /// Why this route cannot be attempted, kept structured so the scannable
    /// row can show the reason without re-parsing the prose `hint`. `None`
    /// whenever the route is attemptable.
    blocked_reason: Option<String>,
    /// Whether this provider/model pair belongs in the conservative ordinary
    /// chooser. Explicit catalog views ignore this flag.
    enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct EffectivePickerMetadata {
    context_window: Option<u32>,
    /// The context window came through the legacy provider fallback rather
    /// than an offering, catalog row, roster, or operator override — shown,
    /// but never as a verified capability (#5239, #5441).
    context_window_unverified: bool,
    max_output: Option<u32>,
    /// The output ceiling is an assumed floor for a route that publishes no
    /// ceiling we can stand behind (unknown Anthropic-family models),
    /// clamped but never labeled "documented" (#5440).
    max_output_unverified: bool,
    tool_calls: Option<bool>,
    reasoning: bool,
    vision: SupportState,
    pricing: PickerPricing,
    source: Option<CatalogSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum PickerPricing {
    /// The route explicitly does not expose authoritative token pricing.
    Unavailable,
    Known(String),
    #[default]
    Unknown,
}

impl ModelPickerView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let initial_model = if app.auto_model {
            "auto".to_string()
        } else {
            picker_visible_model_id(app.api_provider, &app.model, app.accepts_custom_model_ids())
                .to_string()
        };
        let previous_model = if app.auto_model {
            "auto".to_string()
        } else {
            app.model.clone()
        };
        let model_rows = picker_model_rows_for_app(app, config);
        let configured_providers: Vec<_> = configured_providers(config, app.api_provider)
            .into_iter()
            .filter(|provider| *provider != app.api_provider)
            .collect();
        let mut default_visible_rows: Vec<_> = model_rows
            .iter()
            .filter(|row| {
                model_row_visible_in_view(row, ModelListView::Configured, app.api_provider)
            })
            .collect();
        // Selection indices must be calculated in the same order that the
        // configured view renders. Pinned rows are sorted to the top by
        // `visible_model_rows`; using the unsorted construction order here
        // made the cursor land on a different row (or look unselected) after
        // a pin reordered the list.
        let pins = picker_pins_for_app(app);
        sort_model_rows_for_view(&mut default_visible_rows, ModelListView::Configured, &pins);
        let mut selected_model_idx = default_visible_rows.iter().position(|row| {
            row.id == initial_model
                && (row.provider.is_none() || row.provider == Some(app.api_provider))
        });
        let show_custom_model_row = selected_model_idx.is_none();
        if show_custom_model_row {
            selected_model_idx = Some(default_visible_rows.len());
        }
        let selected_model_idx = selected_model_idx.unwrap_or(0);

        let initial_effort = app
            .reasoning_effort_preference
            .unwrap_or(ReasoningEffort::Auto);
        let selected_effort_request = app
            .reasoning_effort_preference
            .unwrap_or(app.reasoning_effort);
        let effort_rows = picker_efforts_for_route(
            app.api_provider,
            &config.deepseek_base_url(),
            &initial_model,
            app.auto_model,
        );
        let normalized = normalize_picker_effort(
            selected_effort_request,
            app.api_provider,
            &config.deepseek_base_url(),
            &initial_model,
            app.auto_model,
        );
        let selected_effort_idx = effort_rows
            .iter()
            .position(|e| *e == normalized)
            .unwrap_or_else(|| {
                default_picker_effort_idx(
                    app.api_provider,
                    &config.deepseek_base_url(),
                    &initial_model,
                    app.auto_model,
                )
            });

        let mut view = Self {
            initial_model,
            previous_model,
            initial_provider: app.api_provider,
            initial_effort,
            selected_effort_request,
            active_accepts_custom_model_ids: app.accepts_custom_model_ids(),
            query: String::new(),
            selected_model_idx,
            selected_effort_idx,
            focus: Pane::Model,
            show_custom_model_row,
            model_rows,
            route_config: config.clone(),
            provider_health: app.provider_health.clone(),
            view: ModelListView::Configured,
            configured_providers,
            row_hitboxes: RefCell::new(Vec::new()),
            last_mouse_selected: None,
            locale: app.ui_locale,
            pinned_models: pins,
        };
        view.restore_memory(app.model_picker_memory.as_ref());
        view
    }

    /// Restore the browsing context from the last dismissed picker (#4109):
    /// the named catalog view and, when the remembered row still exists in
    /// that view, the highlighted row. The active model remains the selection
    /// when nothing was remembered or the row is gone.
    fn restore_memory(&mut self, memory: Option<&crate::tui::app::ModelPickerMemory>) {
        let Some(memory) = memory else {
            return;
        };
        if let Some(view_name) = memory.view.as_deref() {
            if let Some(view) = ModelListView::from_memory_name(view_name) {
                self.view = view;
            }
        } else if memory.catalog_view {
            self.view = ModelListView::Catalog;
        }
        if let Some(remembered_id) = memory.selected_row_id.as_deref() {
            let position = self
                .visible_model_rows()
                .iter()
                .position(|row| row.id == remembered_id);
            if let Some(position) = position {
                self.selected_model_idx = position;
                self.select_effort_for_current_model();
            }
        }
        self.clamp_model_selection();
    }

    fn visible_model_rows(&self) -> Vec<&ModelPickerRow> {
        let query = self.query.trim();
        let mut rows: Vec<&ModelPickerRow> = self
            .model_rows
            .iter()
            .filter(|row| {
                if query.is_empty() {
                    // Empty query: view scope only (Configured stays conservative).
                    model_row_visible_in_view(row, self.view, self.initial_provider)
                } else {
                    // Typed filter searches the full lake so cross-provider
                    // routes remain discoverable without leaving Configured.
                    model_row_matches_query(row, query, self.initial_provider)
                }
            })
            .collect();
        if query.is_empty() {
            sort_model_rows_for_view(&mut rows, self.view, &self.pinned_models);
        } else {
            // Rank typed results (#4639): rows whose provider matches the
            // query first (provider drill-down), then exact/prefix id
            // matches, then the active provider's rows, then alphabetical —
            // so a provider-heavy catalog (e.g. OpenRouter) surfaces the
            // intended route in the first few rows, not raw catalog order.
            let query_lower = query.to_ascii_lowercase();
            let initial_provider = self.initial_provider;
            rows.sort_by(|a, b| {
                let rank = |row: &ModelPickerRow| {
                    let provider_matches = row.provider.is_some_and(|provider| {
                        row.provider_identity.as_deref().is_some_and(|identity| {
                            identity.to_ascii_lowercase().contains(&query_lower)
                        }) || provider
                            .as_str()
                            .to_ascii_lowercase()
                            .contains(&query_lower)
                            || provider
                                .display_name()
                                .to_ascii_lowercase()
                                .contains(&query_lower)
                    });
                    let id = row.id.to_ascii_lowercase();
                    let id_rank = if id == query_lower {
                        0
                    } else if id.starts_with(&query_lower) {
                        1
                    } else {
                        2
                    };
                    let provider_rank =
                        if row.provider.is_none() || row.provider == Some(initial_provider) {
                            0
                        } else {
                            1
                        };
                    (
                        if provider_matches { 0 } else { 1 },
                        id_rank,
                        provider_rank,
                        id,
                    )
                };
                rank(a).cmp(&rank(b))
            });
        }
        rows
    }

    fn model_row_count(&self) -> usize {
        let rows = self.visible_model_rows();
        rows.len() + usize::from(self.custom_model_row_for_visible(&rows).is_some())
    }

    /// Resolve the currently highlighted row to a model id.
    fn resolved_model(&self) -> String {
        let rows = self.visible_model_rows();
        if self.selected_model_idx < rows.len() {
            return rows[self.selected_model_idx].id.clone();
        }
        self.custom_model_row()
            .map(|(model, _)| model)
            .unwrap_or_else(|| self.initial_model.clone())
    }

    fn selected_model_is_selectable(&self) -> bool {
        let rows = self.visible_model_rows();
        if let Some(row) = rows.get(self.selected_model_idx) {
            return row.selectable;
        }
        self.custom_model_row().is_some_and(|(model, provider)| {
            crate::provider_readiness::resolve_for_model(
                &self.route_config,
                provider,
                &model,
                &self.provider_health,
            )
            .can_attempt()
        })
    }

    /// Feedback when Enter/apply is pressed on a locked (unauthenticated) model.
    /// Surfaces the readiness reason instead of a silent no-op, and routes the
    /// user toward provider authentication/setup when possible.
    fn explain_unselectable_selection(&self) -> ViewAction {
        let rows = self.visible_model_rows();
        let Some(row) = rows.get(self.selected_model_idx) else {
            return ViewAction::None;
        };
        let reason = if row.hint.trim().is_empty() {
            "This model is not available with the current provider credentials.".to_string()
        } else {
            row.hint.clone()
        };
        let message = format!(
            "🔒 {} is locked — {reason}. Open /provider to authenticate, then refresh.",
            row.id
        );
        // Prefer opening provider setup so the user can remediate in one step.
        if let Some(provider) = row.provider {
            return ViewAction::Emit(ViewEvent::ModelPickerNeedsAuth {
                provider,
                model: row.id.clone(),
                reason: message,
            });
        }
        ViewAction::Emit(ViewEvent::StatusMessage { message })
    }

    fn resolved_provider(&self) -> Option<ApiProvider> {
        let rows = self.visible_model_rows();
        if self.selected_model_idx < rows.len() {
            return rows[self.selected_model_idx].provider;
        }
        self.custom_model_row()
            .map(|(_, provider)| provider)
            .or(Some(self.initial_provider))
    }

    fn resolved_effort(&self) -> ReasoningEffort {
        let efforts = self.current_efforts();
        efforts[self
            .selected_effort_idx
            .min(efforts.len().saturating_sub(1))]
    }

    fn current_efforts(&self) -> Vec<ReasoningEffort> {
        let provider = self.resolved_provider().unwrap_or(self.initial_provider);
        let model = self.resolved_model();
        let base_url = self.resolved_base_url_for_provider(provider, &model);
        picker_efforts_for_route(
            provider,
            &base_url,
            &model,
            model.trim().eq_ignore_ascii_case("auto"),
        )
    }

    fn resolved_base_url_for_provider(&self, provider: ApiProvider, model: &str) -> String {
        crate::route_runtime::resolve_runtime_route(&self.route_config, provider, Some(model))
            .map(|route| route.candidate.endpoint().base_url.clone())
            .unwrap_or_else(|_| provider.default_base_url().to_string())
    }

    fn custom_model_row(&self) -> Option<(String, ApiProvider)> {
        let rows = self.visible_model_rows();
        self.custom_model_row_for_visible(&rows)
    }

    fn custom_model_row_for_visible(
        &self,
        visible_rows: &[&ModelPickerRow],
    ) -> Option<(String, ApiProvider)> {
        let query = self.query.trim();
        if query.is_empty() {
            return self
                .show_custom_model_row
                .then(|| (self.initial_model.clone(), self.initial_provider));
        }
        if let Some((provider, model)) = self.provider_qualified_custom_query(query) {
            if visible_rows.iter().any(|row| {
                row.provider == Some(provider) && row.id.eq_ignore_ascii_case(model.trim())
            }) {
                return None;
            }
            if self.provider_accepts_custom_model(provider, &model) {
                return Some((model, provider));
            }
            return None;
        }
        if !self.active_accepts_custom_model_ids {
            return None;
        }
        if visible_rows.iter().any(|row| {
            row.provider == Some(self.initial_provider) && row.id.eq_ignore_ascii_case(query)
        }) {
            return None;
        }
        Some((query.to_string(), self.initial_provider))
    }

    fn provider_qualified_custom_query(&self, query: &str) -> Option<(ApiProvider, String)> {
        for (provider_key, model) in provider_query_splits(query) {
            let Some(provider) = ApiProvider::parse(provider_key) else {
                continue;
            };
            if provider != self.initial_provider
                && !self.view.browses_all_providers()
                && !self.configured_providers.contains(&provider)
            {
                continue;
            }
            let model = model.trim();
            if model.is_empty() {
                continue;
            }
            return Some((provider, model.to_string()));
        }
        None
    }

    fn provider_accepts_custom_model(&self, provider: ApiProvider, model: &str) -> bool {
        (provider == self.initial_provider && self.active_accepts_custom_model_ids)
            || crate::config::normalize_model_name_for_provider(provider, model).is_some()
    }

    fn clamp_model_selection(&mut self) {
        let count = self.model_row_count();
        if count == 0 {
            self.selected_model_idx = 0;
        } else if self.selected_model_idx >= count {
            self.selected_model_idx = count - 1;
        }
    }

    fn update_query(&mut self, next: String) {
        self.query = next;
        self.selected_model_idx = 0;
        self.clamp_model_selection();
        self.select_effort_for_current_model();
    }

    fn select_effort_for_current_model(&mut self) {
        let provider = self.resolved_provider().unwrap_or(self.initial_provider);
        let model = self.resolved_model();
        let model_is_auto = model.trim().eq_ignore_ascii_case("auto");
        let base_url = self.resolved_base_url_for_provider(provider, &model);
        let normalized = normalize_picker_effort(
            self.selected_effort_request,
            provider,
            &base_url,
            &model,
            model_is_auto,
        );
        self.selected_effort_idx =
            picker_efforts_for_route(provider, &base_url, &model, model_is_auto)
                .iter()
                .position(|candidate| *candidate == normalized)
                .unwrap_or_else(|| {
                    default_picker_effort_idx(provider, &base_url, &model, model_is_auto)
                });
    }

    fn move_up(&mut self) -> bool {
        match self.focus {
            Pane::Model => {
                if self.selected_model_idx > 0 {
                    self.selected_model_idx -= 1;
                    self.select_effort_for_current_model();
                    return true;
                }
            }
            Pane::Effort => {
                if self.selected_effort_idx > 0 {
                    self.selected_effort_idx -= 1;
                    self.selected_effort_request = self.resolved_effort();
                    return true;
                }
            }
        }
        false
    }

    fn move_down(&mut self) -> bool {
        match self.focus {
            Pane::Model => {
                let max = self.model_row_count().saturating_sub(1);
                if self.selected_model_idx < max {
                    self.selected_model_idx += 1;
                    self.select_effort_for_current_model();
                    return true;
                }
            }
            Pane::Effort => {
                let max = self.current_efforts().len().saturating_sub(1);
                if self.selected_effort_idx < max {
                    self.selected_effort_idx += 1;
                    self.selected_effort_request = self.resolved_effort();
                    return true;
                }
            }
        }
        false
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Pane::Model => Pane::Effort,
            Pane::Effort => Pane::Model,
        };
    }

    fn toggle_view(&mut self) {
        self.view = self.view.next();
        self.selected_model_idx = 0;
        self.clamp_model_selection();
        self.select_effort_for_current_model();
    }

    fn build_event(&self) -> ViewEvent {
        self.build_event_with_startup_default(false)
    }

    fn build_event_with_startup_default(&self, save_as_startup_default: bool) -> ViewEvent {
        let resolved_provider = self.resolved_provider().unwrap_or(self.initial_provider);
        let provider = (resolved_provider != self.initial_provider).then_some(resolved_provider);
        let provider_id = (resolved_provider == ApiProvider::Custom)
            .then(|| self.route_config.provider_identity_for(resolved_provider));
        ViewEvent::ModelPickerApplied {
            model: self.resolved_model(),
            provider,
            provider_id,
            effort: self.selected_effort_request,
            previous_model: self.previous_model.clone(),
            previous_effort: self.initial_effort,
            save_as_startup_default,
        }
    }

    fn render_pane(
        &self,
        area: Rect,
        buf: &mut Buffer,
        title: &str,
        rows: Vec<PaneRow>,
        state: PaneRenderState,
    ) {
        let visible_height = usize::from(area.height.saturating_sub(1));
        let (start, end) = visible_row_window(state.selected, rows.len(), visible_height);
        let title = if rows.len() > visible_height && visible_height > 0 {
            if start + 1 == end {
                // A scrollable pane whose visible window spans exactly one row
                // renders a single position (`Model 2/3`), not a degenerate
                // `2-2/3` range (#3995).
                format!(" {title} {}/{} ", end, rows.len())
            } else {
                format!(" {title} {}-{}/{} ", start + 1, end, rows.len())
            }
        } else {
            format!(" {title} ")
        };
        Block::default()
            .style(Style::default().bg(palette::WHALE_BG))
            .render(area, buf);
        let title_area = Rect { height: 1, ..area };
        Paragraph::new(Line::from(vec![
            Span::styled(
                if state.focused { "▸ " } else { "  " },
                Style::default().fg(palette::WHALE_ACTION),
            ),
            Span::styled(
                title,
                Style::default()
                    .fg(if state.focused {
                        palette::WHALE_ACTION
                    } else {
                        palette::TEXT_PRIMARY
                    })
                    .bold(),
            ),
        ]))
        .render(title_area, buf);
        let inner = Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        };

        // Column widths are measured over the rows actually on screen, so the
        // route column lands at one predictable offset for the whole page
        // instead of drifting with whatever long id happens to be scrolled in.
        let columns = ModelRowColumns::for_page(&rows[start.min(rows.len())..end.min(rows.len())]);

        let mut lines = Vec::with_capacity(end.saturating_sub(start));
        let pane_height = usize::from(inner.height);
        for (idx, row) in rows.iter().enumerate().skip(start).take(end - start) {
            // Family headers consume pane lines too: stop building (and stop
            // recording hitboxes) as soon as the pane is full, so rendering
            // never addresses the buffer past its bounds.
            if lines.len() >= pane_height {
                break;
            }
            let is_selected = idx == state.selected;
            // Non-selectable rows are dimmed with a lock glyph so they never
            // look choosable. Selection still highlights, but stays muted.
            let locked = state.pane == Pane::Model
                && self
                    .visible_model_rows()
                    .get(idx)
                    .is_some_and(|row| !row.selectable);
            // Marker precedence: a locked route first (it is the reason Enter
            // will not work), then the keyboard cursor, then the route this
            // session is already on. `CURRENT` is the charter's "current human
            // choice" mark, so "which one am I on?" is answered by shape rather
            // than by a second accent colour.
            let marker = if locked {
                "🔒"
            } else if is_selected {
                crate::tui::glyphs::SELECTION
            } else if row.active {
                crate::tui::glyphs::CURRENT
            } else {
                " "
            };
            let label_style = if is_selected && !locked {
                menu_style::selected_row_style()
            } else if is_selected && locked {
                menu_style::disabled_selected_row_style()
            } else if locked {
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_selected && !locked {
                menu_style::selected_row_bg_style().fg(palette::SELECTION_TEXT)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            // Provider → family → model grouping: a dim family header is
            // drawn when the catalog states a family and it differs from the
            // previous visible row's (families sort contiguously). Unknown
            // families draw nothing.
            if let Some(family) = row.family.as_deref() {
                let prev_family = rows
                    .get(idx.wrapping_sub(1))
                    .and_then(|prev| prev.family.as_deref());
                let prev_provider = rows
                    .get(idx.wrapping_sub(1))
                    .map(|prev| prev.route.as_str());
                if prev_family != Some(family) || prev_provider != Some(row.route.as_str()) {
                    lines.push(Line::from(Span::styled(
                        format!("  ─ {family}"),
                        Style::default().fg(palette::TEXT_DIM),
                    )));
                }
            }
            // The hitbox points at the row's own line (after any family
            // header), so mouse/scan targets and keyboard targets agree.
            let row_y = inner.y.saturating_add(lines.len() as u16);
            self.row_hitboxes.borrow_mut().push((
                Rect::new(inner.x, row_y, inner.width, 1),
                state.pane,
                idx,
            ));
            let spans = picker_row_spans(
                row,
                marker,
                usize::from(inner.width),
                columns,
                label_style,
                hint_style,
            );
            lines.push(Line::from(spans));
        }
        if rows.is_empty() {
            // A search that matches nothing must say so, not render a bare
            // empty box (#3757 UX review).
            let message = if self.query.is_empty() {
                tr(self.locale, MessageId::RouteNoModels).into_owned()
            } else {
                tr(self.locale, MessageId::RouteNoModelMatch).replace("{query}", &self.query)
            };
            lines.push(Line::from(Span::styled(
                message,
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        // Family headers can push the visible rows past the viewport; clip
        // to the area so rendering never indexes the buffer out of bounds
        // (ratatui-core 0.1.0 panics instead of clipping).
        if lines.len() > usize::from(inner.height) {
            lines.truncate(usize::from(inner.height));
        }
        Paragraph::new(lines).render(inner, buf);
    }
}

fn visible_row_window(selected: usize, total: usize, viewport_height: usize) -> (usize, usize) {
    if total == 0 || viewport_height == 0 {
        return (0, 0);
    }

    let visible = viewport_height.min(total);
    let mut start = selected.saturating_sub(visible / 2);
    if start + visible > total {
        start = total.saturating_sub(visible);
    }
    (start, start + visible)
}

/// Widest Thinking row plus its marker: `max  (extra-high reasoning)`.
const EFFORT_PANE_WIDTH: u16 = 30;

/// Give the model list the width the Thinking pane cannot use.
///
/// The generic list/detail split caps the list at 52 columns and hands the
/// remainder to the detail pane. Thinking rows are a fixed, short vocabulary,
/// so on a wide terminal most of the row went to a pane with nothing to put
/// there while the model rows — which carry the id, route and metadata that
/// tell near-identical routes apart — were squeezed into half a screen.
fn widen_model_pane(layout: ListDetailLayout) -> ListDetailLayout {
    if layout.stacked {
        return layout;
    }
    let gap = layout
        .detail
        .x
        .saturating_sub(layout.list.x.saturating_add(layout.list.width));
    let total = layout.list.width + gap + layout.detail.width;
    let detail_width = layout.detail.width.min(EFFORT_PANE_WIDTH);
    let list_width = total.saturating_sub(gap + detail_width);
    ListDetailLayout {
        list: Rect {
            width: list_width,
            ..layout.list
        },
        detail: Rect {
            x: layout.list.x + list_width + gap,
            width: detail_width,
            ..layout.detail
        },
        stacked: false,
    }
}

/// One rendered row in either picker pane, split into the columns the row is
/// laid out from.
///
/// Model rows fill all three: the wire id (`primary`), the route identity that
/// separates same-named models on different endpoints (`route`), and the facts
/// that actually vary between neighbouring rows (`meta`). Thinking-effort rows
/// leave `route` empty and keep their descriptive `meta`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PaneRow {
    primary: String,
    route: String,
    /// Metadata as separable units. Kept as a list so a squeezed column sheds
    /// whole facts instead of rendering half a word.
    meta: Vec<String>,
    /// Catalog model family (e.g. `deepseek`, `glm`) for section headers.
    /// None = the catalog did not state a family (no header is drawn).
    family: Option<String>,
    /// The route this session is already on.
    active: bool,
}

impl PaneRow {
    fn effort(primary: String, meta: String) -> Self {
        Self {
            primary,
            route: String::new(),
            meta: if meta.is_empty() {
                Vec::new()
            } else {
                vec![meta]
            },
            family: None,
            active: false,
        }
    }

    fn meta_width(&self) -> usize {
        unicode_width::UnicodeWidthStr::width(self.meta.join(" · ").as_str())
    }
}

/// Per-page column offsets for a picker pane.
///
/// Rows used to render as `label  (one long parenthesised hint)`, which meant
/// the hint was dropped whole whenever it did not fit — and at every real
/// terminal width it never fit, so a dozen DeepSeek routes all rendered as
/// nothing but their near-identical ids. Fixed columns fix that: each field
/// gets a measured share of the row and is truncated on its own, so the
/// distinguishing token is always on screen at a predictable offset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModelRowColumns {
    primary: usize,
    route: usize,
    meta: usize,
}

/// Width reserved for the marker glyph itself. The lock is a two-column emoji
/// while `▸` and `●` are one, so the cell is padded to the widest of them —
/// otherwise a single locked row shifts every column on its line by one.
const MARKER_CELL_WIDTH: usize = 2;
/// ` ▸  ` — one leading space, the marker cell, one trailing space.
const ROW_PREFIX_WIDTH: usize = MARKER_CELL_WIDTH + 2;
/// Blank cells between two columns.
const COLUMN_GAP: usize = 2;
/// Below this a route column tells the user nothing, so the space goes to the
/// id instead.
const MIN_ROUTE_WIDTH: usize = 6;
/// Below this the metadata column cannot hold even a context-window token.
const MIN_META_WIDTH: usize = 4;

impl ModelRowColumns {
    /// Measure the natural width each column wants, over the rows on screen.
    fn for_page(rows: &[PaneRow]) -> Self {
        let widest = |pick: fn(&PaneRow) -> usize| rows.iter().map(pick).max().unwrap_or(0);
        Self {
            primary: widest(|row| unicode_width::UnicodeWidthStr::width(row.primary.as_str())),
            route: widest(|row| unicode_width::UnicodeWidthStr::width(row.route.as_str())),
            meta: widest(PaneRow::meta_width),
        }
    }

    /// Fit the measured widths into the width actually available.
    ///
    /// When everything fits, every column keeps its natural width. When it does
    /// not, the scarce space is divided rather than handed to whichever column
    /// comes first: the id used to take everything and the metadata was dropped
    /// whole, which is precisely how a dozen near-identical routes ended up
    /// rendering as nothing but their shared prefix.
    fn resolve(self, width: usize) -> Self {
        let available = width.saturating_sub(ROW_PREFIX_WIDTH);
        if available == 0 {
            return Self::default();
        }
        let gaps = COLUMN_GAP * (usize::from(self.route > 0) + usize::from(self.meta > 0));
        let content = available.saturating_sub(gaps);
        if content == 0 {
            return Self {
                primary: available,
                route: 0,
                meta: 0,
            };
        }
        if self.primary + self.route + self.meta <= content {
            return self;
        }
        // With no route column there is nothing to protect from a long id, so
        // the id keeps its natural width and the trailing metadata yields — a
        // clipped model id is worse than a hidden hint.
        if self.route == 0 {
            let primary = self.primary.min(content);
            let meta = self.meta.min(content.saturating_sub(primary));
            return Self {
                primary,
                route: 0,
                meta: if meta < MIN_META_WIDTH { 0 } else { meta },
            };
        }

        // Floors first, so no column that has something to say disappears
        // entirely; then each takes the smaller of its natural width and its
        // share. Metadata is the densest per column and gets the tightest cap.
        let mut meta = if self.meta == 0 {
            0
        } else {
            self.meta
                .min((content / 4).max(MIN_META_WIDTH.min(content)))
        };
        let after_meta = content.saturating_sub(meta);
        let mut route = if self.route == 0 {
            0
        } else {
            self.route
                .min((after_meta / 3).max(MIN_ROUTE_WIDTH.min(after_meta)))
        };
        let mut primary = after_meta.saturating_sub(route);

        // The id's share is whatever the other two did not take, which can
        // exceed the longest id on the page. Hand that surplus back rather than
        // padding blank space next to a metadata column that is shedding facts.
        if primary > self.primary {
            let mut slack = primary - self.primary;
            primary = self.primary;
            for (column, natural) in [(&mut meta, self.meta), (&mut route, self.route)] {
                let gain = slack.min(natural.saturating_sub(*column));
                *column += gain;
                slack -= gain;
            }
            primary += slack;
        }

        Self {
            primary,
            route,
            meta,
        }
    }
}

/// Truncate an identifier from the middle, keeping both ends.
///
/// Model ids and route names share their heads and differ in their tails:
/// `deepseek-ai/DeepSeek-V4-Pro` and `deepseek-ai/DeepSeek-V4-Flash` are
/// identical for twenty characters and only separate at the very end. Clipping
/// the tail therefore deletes the one token that tells them apart — both rows
/// render as `deepseek-ai/DeepSee...`. Keeping a slice of each end costs one
/// column for the ellipsis and preserves the variant.
fn fit_identifier(text: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    // Too narrow to seat a head, an ellipsis and a meaningful tail; fall back
    // to the plain head-first form rather than emit punctuation soup.
    if width < 8 {
        return fit_text(text, width);
    }

    let budget = width - 1;
    // The tail is the discriminating end, so it gets the larger share.
    let tail_budget = (budget * 3) / 5;
    let head_budget = budget - tail_budget;

    let mut head = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > head_budget {
            break;
        }
        used += ch_width;
        head.push(ch);
    }

    let mut tail: Vec<char> = Vec::new();
    let mut used = 0usize;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > tail_budget {
            break;
        }
        used += ch_width;
        tail.push(ch);
    }
    tail.reverse();

    let mut out = head;
    out.push('…');
    out.extend(tail);
    out
}

/// Lay a row out into aligned, individually-truncated columns.
///
/// Colour vocabulary is deliberately two-valued: `label_style` for the row's
/// primary content and `hint_style` for every secondary column. Selection is
/// the only thing that changes a row's colour.
fn picker_row_spans<'a>(
    row: &'a PaneRow,
    marker: &'static str,
    width: usize,
    columns: ModelRowColumns,
    label_style: Style,
    hint_style: Style,
) -> Vec<Span<'a>> {
    use unicode_width::UnicodeWidthStr;

    let columns = columns.resolve(width);
    let marker_pad = MARKER_CELL_WIDTH.saturating_sub(UnicodeWidthStr::width(marker));
    let mut spans = vec![
        Span::styled(" ", label_style),
        Span::styled(marker, label_style),
        Span::styled(" ".repeat(marker_pad + 1), label_style),
    ];
    let mut used = ROW_PREFIX_WIDTH;

    let primary = fit_identifier(&row.primary, columns.primary.max(1));
    used += UnicodeWidthStr::width(primary.as_str());
    spans.push(Span::styled(primary, label_style));

    // Pad to the column edge only when something follows; a trailing run of
    // spaces would otherwise extend the selected row's highlight past its text.
    let pad_to = |spans: &mut Vec<Span<'a>>, used: &mut usize, target: usize| {
        if *used < target {
            spans.push(Span::styled(" ".repeat(target - *used), label_style));
            *used = target;
        }
    };

    if columns.route > 0 && !row.route.is_empty() {
        pad_to(&mut spans, &mut used, ROW_PREFIX_WIDTH + columns.primary);
        spans.push(Span::styled(" ".repeat(COLUMN_GAP), label_style));
        used += COLUMN_GAP;
        let route = fit_identifier(&row.route, columns.route);
        used += UnicodeWidthStr::width(route.as_str());
        spans.push(Span::styled(route, hint_style));
    }

    if !row.meta.is_empty() {
        let column_edge = if columns.route > 0 && !row.route.is_empty() {
            ROW_PREFIX_WIDTH + columns.primary + COLUMN_GAP + columns.route
        } else {
            ROW_PREFIX_WIDTH + columns.primary
        };
        // Take the smaller of the column's share and the physical remainder, so
        // a row that ended early cannot overrun the pane.
        let remaining = width
            .saturating_sub(column_edge)
            .saturating_sub(COLUMN_GAP)
            .min(columns.meta.max(MIN_META_WIDTH));
        let meta = fit_meta_chips(&row.meta, remaining);
        if !meta.is_empty() {
            pad_to(&mut spans, &mut used, column_edge);
            spans.push(Span::styled(" ".repeat(COLUMN_GAP), label_style));
            spans.push(Span::styled(meta, hint_style));
        }
    }

    spans
}

fn fit_text(text: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width <= 3 {
        return ".".repeat(width);
    }

    let mut out = String::new();
    let target = width - 3;
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > target {
            break;
        }
        used += ch_width;
        out.push(ch);
    }
    out.push_str("...");
    out
}

pub(crate) fn provider_scoped_model_completion_ids(app: &App) -> Vec<String> {
    // Slash completions inline the current custom model so `/model <current>`
    // stays visible even when it is outside the provider catalog.
    provider_scoped_model_ids_for_app(app, true)
}

/// The pins the picker sorts and labels by: the fleet's models first (the
/// selected Fleet's operator and every pinned member, labelled with the roles
/// each fills — design §10 F1), then the person's own pins.
fn picker_pins_for_app(app: &App) -> Vec<PinnedModel> {
    // A selected fleet that cannot be read contributes no pins; ⇧F on any
    // row then surfaces that store error instead of writing past it.
    crate::fleet::members::fleet_models(&app.workspace)
        .unwrap_or_default()
        .into_iter()
        .map(|member| PinnedModel {
            provider: member.provider.clone(),
            model: member.model.clone(),
            label: Some(format!("fleet · {}", member.roles_label())),
        })
        .chain(app.pinned_models.iter().cloned())
        .collect()
}

fn picker_model_rows_for_app(app: &App, config: &Config) -> Vec<ModelPickerRow> {
    let mut rows = Vec::new();
    let auto_hint = auto_picker_hint(app, config);
    push_auto_model_row(&mut rows, app, config, &auto_hint);
    // One snapshot supplies both IDs, capabilities, and freshness so a cache
    // replacement cannot produce mixed-generation picker rows.
    let codex_roster = codex_model_cache::model_roster();
    let mut active_model_ids = if app.api_provider == ApiProvider::OpenaiCodex {
        let mut models = vec!["auto".to_string()];
        for id in codex_roster.model_ids() {
            push_model_id(&mut models, &id);
        }
        if let Some(model) = app
            .provider_models
            .get(app.provider_identity_for_persistence())
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
        {
            push_model_id(
                &mut models,
                picker_visible_model_id(app.api_provider, model, app.accepts_custom_model_ids()),
            );
        }
        models
    } else {
        provider_scoped_model_ids_for_app(app, false)
    };
    push_configured_provider_model(&mut active_model_ids, config, app.api_provider);
    push_provider_model_rows(
        &mut rows,
        app.api_provider,
        active_model_ids,
        app.api_provider,
        config,
        &codex_roster,
        &app.provider_health,
    );

    for provider in ApiProvider::sorted_for_display() {
        if provider == app.api_provider {
            continue;
        }
        let mut model_ids = if provider == ApiProvider::OpenaiCodex {
            codex_roster.model_ids()
        } else {
            provider_catalog_model_ids(provider)
        };
        if let Some(model) = app
            .provider_models
            .get(provider.as_str())
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
        {
            push_model_id(
                &mut model_ids,
                picker_visible_model_id(
                    provider,
                    model,
                    config.model_ids_pass_through_for_provider(provider),
                ),
            );
        }
        push_configured_provider_model(&mut model_ids, config, provider);
        push_provider_model_rows(
            &mut rows,
            provider,
            model_ids,
            app.api_provider,
            config,
            &codex_roster,
            &app.provider_health,
        );
    }

    // `ApiProvider::Custom` is shared by every named custom route. Preserve
    // the concrete active route key on rows so exact pins cannot collide.
    let active_custom_identity = (app.api_provider == ApiProvider::Custom)
        .then(|| app.provider_identity_for_persistence().to_string());
    for row in &mut rows {
        if row.provider == Some(ApiProvider::Custom) {
            row.provider_identity = active_custom_identity.clone();
        }
    }

    // The fleet comes first (design §10 F1): every model the person added
    // to the selected Fleet rides the pin machinery ahead of their own pins,
    // labelled with the roles it fills, so the list leads with what they
    // chose rather than with a provider's alphabet.
    let pins = picker_pins_for_app(app);
    for row in &mut rows {
        row.enabled = model_row_enabled_for_app(app, config, row);
        if let Some(pin) = pins.iter().find(|pin| {
            row_provider_identity(row)
                .is_some_and(|provider| provider.eq_ignore_ascii_case(&pin.provider))
                && row.id.eq_ignore_ascii_case(&pin.model)
        }) {
            let label = pin.label.as_deref().unwrap_or("pinned");
            row.hint = format!(
                "{label} · exact {} / {} · {}",
                pin.provider, pin.model, row.hint
            );
        }
    }

    for pin in &pins {
        let provider = ApiProvider::parse(&pin.provider).unwrap_or(ApiProvider::Custom);
        if rows.iter().any(|row| {
            row_provider_identity(row)
                .is_some_and(|identity| identity.eq_ignore_ascii_case(&pin.provider))
                && row.id.eq_ignore_ascii_case(&pin.model)
        }) {
            continue;
        }
        let metadata = effective_picker_metadata(config, Some(provider), &pin.model);
        // Bypass the ordinary `(enum provider, model)` de-duplication here:
        // two named Custom routes may intentionally expose the same model id.
        rows.push(ModelPickerRow {
            id: pin.model.clone(),
            provider: Some(provider),
            provider_identity: Some(pin.provider.clone()),
            hint: format!(
                "stale pinned · exact {} / {} · unavailable; repair or remove",
                pin.provider, pin.model
            ),
            metadata,
            selectable: false,
            blocked_reason: Some("stale pin".to_string()),
            enabled: true,
        });
    }

    rows
}

fn model_row_enabled_for_app(app: &App, config: &Config, row: &ModelPickerRow) -> bool {
    let Some(provider) = row.provider else {
        return true;
    };
    if provider == app.api_provider {
        let current =
            picker_visible_model_id(app.api_provider, &app.model, app.accepts_custom_model_ids());
        if row.id.eq_ignore_ascii_case(current) {
            return true;
        }
    }
    let provider_identity = if provider == app.api_provider {
        app.provider_identity_for_persistence()
    } else {
        provider.as_str()
    };
    if app.provider_model_is_enabled(provider_identity, &row.id)
        || app
            .provider_models
            .get(provider_identity)
            .is_some_and(|model| model.eq_ignore_ascii_case(&row.id))
    {
        return true;
    }
    let configured_model = config
        .provider_config_for(provider)
        .and_then(|entry| entry.model.as_deref());
    if configured_model.is_some_and(|model| model.eq_ignore_ascii_case(&row.id)) {
        return true;
    }

    // A Z.ai route saved before GLM-5.3 became the provider default normally
    // carries an explicit GLM-5.2 choice. Keep that exact route intact, but do
    // not let the conservative Configured view hide the current default and
    // make `/model` look permanently stuck on 5.2. Once Z.ai has any enabled
    // model, surface GLM-5.3 alongside it; selecting the row still sends the
    // distinct GLM-5.3 wire id through the ordinary transactional route flow.
    provider == ApiProvider::Zai
        && row
            .id
            .eq_ignore_ascii_case(crate::config::DEFAULT_ZAI_MODEL)
        && (app
            .enabled_provider_models
            .get(provider_identity)
            .is_some_and(|models| !models.is_empty())
            || app
                .provider_models
                .get(provider_identity)
                .is_some_and(|model| !model.trim().is_empty())
            || configured_model.is_some_and(|model| !model.trim().is_empty()))
}

fn push_provider_model_rows(
    rows: &mut Vec<ModelPickerRow>,
    provider: ApiProvider,
    model_ids: Vec<String>,
    active_provider: ApiProvider,
    config: &Config,
    codex_roster: &CodexModelRoster,
    provider_health: &crate::provider_readiness::ProviderReadinessSnapshot,
) {
    for id in model_ids {
        if id == "auto" {
            continue;
        }
        let readiness =
            crate::provider_readiness::resolve_for_model(config, provider, &id, provider_health);
        let selectable = readiness.can_attempt();
        let readiness_label = readiness.label();
        let roster_entry = if provider == ApiProvider::OpenaiCodex {
            codex_roster.metadata_for(&id)
        } else {
            None
        };
        let codex_metadata = if codex_roster.freshness == CodexModelCacheFreshness::Fresh {
            roster_entry
        } else {
            None
        };
        let codex_freshness = roster_entry.map(|_| codex_roster.freshness);
        let metadata =
            effective_picker_metadata_with_codex(config, Some(provider), &id, codex_metadata);
        let mut hint = render_picker_model_hint(&id, Some(provider), &metadata, codex_freshness);
        hint = format!("{readiness_label} · {hint}");
        if provider != active_provider {
            hint = format!("switch route · {hint}");
        }
        let blocked_reason = (!selectable).then(|| readiness_label.to_string());
        push_model_row(
            rows,
            id.clone(),
            Some(provider),
            hint,
            metadata,
            selectable,
            blocked_reason,
        );
    }
}

fn push_auto_model_row(rows: &mut Vec<ModelPickerRow>, app: &App, config: &Config, hint: &str) {
    let readiness = crate::provider_readiness::resolve_for_model(
        config,
        app.api_provider,
        "auto",
        &app.provider_health,
    );
    let metadata = effective_picker_metadata(config, None, "auto");
    let selectable = readiness.can_attempt();
    let blocked_reason = (!selectable).then(|| readiness.label().to_string());
    push_model_row(
        rows,
        "auto".to_string(),
        None,
        format!("{} · {hint}", readiness.label()),
        metadata,
        selectable,
        blocked_reason,
    );
}

fn auto_picker_hint(app: &App, config: &Config) -> String {
    let inventory = crate::model_inventory::ModelInventory::from_config(config);
    // #4411: the classifier only sees other providers under the persisted
    // `[auto] cross_provider` opt-in, so the default hint says active provider
    // only and names the classifier route it will actually call.
    let hint_id = match (inventory.router_available, inventory.cross_provider_auto) {
        (true, true) => MessageId::ModelPickerAutoNetworkHint,
        (true, false) => MessageId::ModelPickerAutoNetworkActiveProviderHint,
        (false, _) => MessageId::ModelPickerAutoLocalHint,
    };
    let mut hint = app
        .tr(hint_id)
        .into_owned()
        .replace("{provider}", inventory.router_provider.display_name())
        .replace("{model}", &inventory.router_model);
    if let (Some(provider), Some(model)) = (
        app.last_effective_provider,
        app.last_effective_model.as_deref(),
    ) {
        let provider_label = if provider == ApiProvider::Custom {
            app.last_effective_provider_identity
                .as_deref()
                .unwrap_or_else(|| app.provider_identity_for_persistence())
        } else {
            provider.display_name()
        };
        let last = app
            .tr(MessageId::ModelPickerAutoLastRoute)
            .replace("{provider}", provider_label)
            .replace("{model}", model);
        hint.push_str(" · ");
        hint.push_str(&last);
    }
    hint
}

fn push_configured_provider_model(
    models: &mut Vec<String>,
    config: &Config,
    provider: ApiProvider,
) {
    if let Some(model) = config
        .provider_config_for(provider)
        .and_then(|entry| entry.model.as_deref())
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        push_model_id(
            models,
            picker_visible_model_id(
                provider,
                model,
                config.model_ids_pass_through_for_provider(provider),
            ),
        );
    }
}

fn provider_catalog_model_ids(provider: ApiProvider) -> Vec<String> {
    let mut models = Vec::new();
    for id in all_catalog_models_for_provider(provider) {
        // The catalog describes the built-in provider route. A custom route's
        // endpoint-owned current/configured model is appended separately.
        push_model_id(&mut models, picker_visible_model_id(provider, &id, false));
    }
    models
}

fn provider_scoped_model_ids_for_app(app: &App, include_current_model: bool) -> Vec<String> {
    // `include_current_model` is for completion surfaces that do not have a
    // separate custom/current-model row.
    let mut models = Vec::new();
    push_model_id(&mut models, "auto");
    for id in provider_catalog_model_ids(app.api_provider) {
        push_model_id(&mut models, &id);
    }

    if let Some(model) = app
        .provider_models
        .get(app.provider_identity_for_persistence())
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        push_model_id(
            &mut models,
            picker_visible_model_id(app.api_provider, model, app.accepts_custom_model_ids()),
        );
    }

    if include_current_model && !app.auto_model {
        push_model_id(
            &mut models,
            picker_visible_model_id(
                app.api_provider,
                app.model.trim(),
                app.accepts_custom_model_ids(),
            ),
        );
    }

    models
}

fn push_model_id(models: &mut Vec<String>, model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    if !models
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(model))
    {
        models.push(model.to_string());
    }
}

/// Migrate retired aliases out of first-party DeepSeek model choices. Custom
/// endpoints and aggregators own their namespaces, where `deepseek-reasoner`
/// can remain a native wire id.
fn picker_visible_model_id(
    provider: ApiProvider,
    model: &str,
    preserve_endpoint_model_ids: bool,
) -> &str {
    if !preserve_endpoint_model_ids
        && matches!(
            provider,
            ApiProvider::Deepseek | ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic
        )
        && (model.eq_ignore_ascii_case("deepseek-chat")
            || model.eq_ignore_ascii_case("deepseek-reasoner"))
    {
        DEEPSEEK_ALIAS_REPLACEMENT
    } else {
        model
    }
}

fn provider_query_splits(query: &str) -> Vec<(&str, &str)> {
    let trimmed = query.trim();
    let mut splits = Vec::new();
    if let Some((provider, model)) = trimmed.split_once(':') {
        splits.push((provider.trim(), model.trim()));
    }
    if let Some(idx) = trimmed.find(char::is_whitespace) {
        let (provider, model) = trimmed.split_at(idx);
        splits.push((provider.trim(), model.trim()));
    }
    splits
}

fn push_model_row(
    rows: &mut Vec<ModelPickerRow>,
    id: String,
    provider: Option<ApiProvider>,
    hint: String,
    metadata: EffectivePickerMetadata,
    selectable: bool,
    blocked_reason: Option<String>,
) {
    if rows
        .iter()
        .any(|row| row.id == id && row.provider == provider)
    {
        return;
    }
    rows.push(ModelPickerRow {
        id,
        provider,
        provider_identity: None,
        hint,
        metadata,
        selectable,
        blocked_reason,
        enabled: false,
    });
}

/// Compact Models.dev freshness chip for the picker chrome (#4139).
///
/// Fresh/live rows stay unmarked; stale and failed caches get an explicit
/// suffix so users know the live layer is still visible but not current.
fn catalog_freshness_title_suffix() -> &'static str {
    catalog_freshness_title_suffix_for(models_dev_live::status().freshness)
}

fn catalog_freshness_title_suffix_for(freshness: ModelsDevFreshness) -> &'static str {
    match freshness {
        ModelsDevFreshness::Stale => " · stale",
        ModelsDevFreshness::Failed => " · refresh failed; catalog available",
        ModelsDevFreshness::Bundled | ModelsDevFreshness::Live => "",
    }
}

/// Cross-field search (#4141): match a query against the provider name
/// (provider key + display name), the display model name, and the wire model
/// id, mirroring `ProviderDashboardRow::matches_query` so the two pickers behave
/// consistently. `row.id` is both the model's display name and the id it is
/// sent to the provider as, so matching it covers the display model name and
/// the wire model id. The compact hint is only searched for the active
/// provider / `auto` rows, preserving the existing cross-provider behavior.
fn model_row_matches_query(
    row: &ModelPickerRow,
    query: &str,
    initial_provider: ApiProvider,
) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    let normalized_query = normalize_picker_search_text(&query);
    let matches = |candidate: &str| {
        let candidate = candidate.to_ascii_lowercase();
        candidate.contains(&query)
            || normalize_picker_search_text(&candidate).contains(&normalized_query)
    };
    let provider_matches = row.provider.is_some_and(|provider| {
        row.provider_identity.as_deref().is_some_and(matches)
            || matches(provider.as_str())
            || matches(provider.display_name())
    });
    provider_matches
        || matches(&row.id)
        || ((row.provider.is_none() || row.provider == Some(initial_provider))
            && matches(&row.hint))
}

fn normalize_picker_search_text(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Route-identity labels for a set of rows, disambiguated where two providers
/// answer to the same display name.
///
/// `Deepseek` and `DeepseekAnthropic` are both spelled "DeepSeek", so a picker
/// listing both showed two rows of literally identical text for two genuinely
/// different endpoints. When a display name is not unique among the rows on
/// offer, the provider's own id — the `[providers.<id>]` key the user would
/// edit — supplies the discriminator, with the leading run it already shares
/// with the display name removed so the suffix is the part that differs.
fn route_labels_for_rows(rows: &[&ModelPickerRow]) -> BTreeMap<&'static str, String> {
    let mut by_display: BTreeMap<&'static str, Vec<ApiProvider>> = BTreeMap::new();
    for provider in rows.iter().filter_map(|row| row.provider) {
        let bucket = by_display.entry(provider.display_name()).or_default();
        if !bucket.contains(&provider) {
            bucket.push(provider);
        }
    }
    let mut labels = BTreeMap::new();
    for (display, providers) in by_display {
        let ambiguous = providers.len() > 1;
        for provider in providers {
            let label = match ambiguous.then(|| route_discriminator(display, provider.as_str())) {
                Some(Some(suffix)) => format!("{display} {suffix}"),
                // The canonical route — the one whose id is just the display
                // name — keeps the bare name; provider ids are unique, so at
                // most one member of a group can land here and the labels stay
                // distinct.
                Some(None) | None => display.to_string(),
            };
            labels.insert(provider.as_str(), label);
        }
    }
    labels
}

/// The part of a provider id that is not already carried by its display name.
fn route_discriminator(display: &str, provider_id: &str) -> Option<String> {
    let squash = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
    };
    let display_key = squash(display).to_ascii_lowercase();
    let id_key = squash(provider_id).to_ascii_lowercase();
    if display_key.is_empty() || !id_key.starts_with(&display_key) {
        return None;
    }
    // Walk the raw id until the display name's alphanumerics are consumed; what
    // remains is the endpoint-specific tail (`-anthropic`, `-CN`, …).
    // Count CHARACTERS, not bytes: `display_key.len()` is a byte length, and
    // for a non-ASCII display name it exceeds the alphanumeric char count, so
    // the loop would over-consume and the discriminator would be wrong or
    // empty (2026-08-04 review).
    let display_key_chars = display_key.chars().count();
    let mut consumed = 0usize;
    let mut tail = provider_id;
    for (offset, ch) in provider_id.char_indices() {
        if consumed == display_key_chars {
            tail = &provider_id[offset..];
            break;
        }
        if ch.is_alphanumeric() {
            consumed += 1;
        }
        tail = &provider_id[offset + ch.len_utf8()..];
    }
    let tail = tail.trim_matches(|c: char| !c.is_alphanumeric());
    (!tail.is_empty()).then(|| tail.to_string())
}

/// The handful of facts that actually differ between neighbouring model rows,
/// in the order they earn their space.
///
/// Everything the old prose hint carried but that reads the same on nearly
/// every row — `tools`, `no vision`, `price unknown`, `bundled` — is dropped
/// here: a token repeated on forty rows cannot tell them apart, and it is what
/// pushed the differentiating tokens off the end of the line. Facts the
/// registry does not know are omitted rather than guessed.
/// The picker section label for a provider/model row.
///
/// Catalog families are useful grouping metadata, but they are not model names.
/// DeepSeek has published both `deepseek` and `deepseek-thinking` as family
/// values for its current V4 models, so keep its picker heading stable and
/// provider-facing rather than exposing either implementation detail.
fn catalog_family_for(provider: ApiProvider, model_id: &str) -> Option<String> {
    if provider == ApiProvider::Deepseek {
        return Some(provider.display_name().to_string());
    }
    crate::provider_lake::catalog_offering_for_model(provider, model_id)
        .and_then(|offering| offering.family)
}

fn model_row_meta_chips(row: &ModelPickerRow) -> Vec<String> {
    let mut chips = Vec::new();
    if let Some(context_window) = row.metadata.context_window {
        chips.push(format_picker_context_window(u64::from(context_window)));
    }
    // The reasoning stance is the most decision-relevant fact for a coding
    // harness, so it sits before the limits/modality chips — the chip budget
    // sheds from the tail, and a squeezed row must never lose the stance.
    chips.push(
        if row.metadata.reasoning {
            "reasoning"
        } else {
            "no reasoning"
        }
        .to_string(),
    );
    // #5239/#5441: an unverified window still drives budgets, but the chip
    // must not lend it a verified reading. Honesty rides as its own chip
    // *after* the stance so a squeezed row sheds the marker before it ever
    // sheds the stance; the full "(unverified)" prose lives in the hint
    // line, which always renders it.
    if row.metadata.context_window_unverified {
        chips.push("unverified ctx".to_string());
    }
    if let Some(max_output) = row.metadata.max_output {
        // #5440: an assumed floor is shown as such, never as a documented
        // ceiling.
        let suffix = if row.metadata.max_output_unverified {
            " (assumed floor)"
        } else {
            ""
        };
        chips.push(format!("{max_output} out{suffix}"));
    }
    // Modality and tool facts are shown only when the catalog genuinely knows
    // them — an unknown is never rendered as a claim.
    match row.metadata.vision {
        SupportState::Supported => chips.push("vision".to_string()),
        SupportState::Unsupported => chips.push("text only".to_string()),
        SupportState::Unknown => {}
    }
    if let Some(tool_calls) = row.metadata.tool_calls {
        chips.push(if tool_calls {
            "tools".to_string()
        } else {
            "no tools".to_string()
        });
    }
    if let Some(reason) = row.blocked_reason.as_deref() {
        chips.push(reason.to_string());
    }
    chips
}

/// Join metadata chips, dropping the lowest-priority ones until the result
/// fits. Truncating mid-chip would render a half-word fact, so whole chips are
/// shed instead.
fn fit_meta_chips(chips: &[String], width: usize) -> String {
    for take in (1..=chips.len()).rev() {
        let joined = chips[..take].join(" · ");
        if unicode_width::UnicodeWidthStr::width(joined.as_str()) <= width {
            return joined;
        }
    }
    // A single chip that still does not fit is prose (an `auto` explanation or
    // an effort description) rather than a fact token, so it is truncated
    // instead of dropped — but only when the column can hold something worth
    // reading.
    match chips.first() {
        Some(first) if width >= MIN_META_WIDTH => fit_text(first, width),
        _ => String::new(),
    }
}

/// Whether a model row shows in the active catalog view (#3830 / #4115).
fn model_row_visible_in_view(
    row: &ModelPickerRow,
    view: ModelListView,
    active_provider: ApiProvider,
) -> bool {
    match view {
        ModelListView::Configured => model_row_visible_by_default(row, active_provider),
        ModelListView::Catalog => true,
        ModelListView::Recent
        | ModelListView::Coding
        | ModelListView::Cheap
        | ModelListView::LongContext => {
            // Discoverability views browse the full lake but hide the synthetic
            // `auto` row — it is not a catalog offering.
            row.provider.is_some() || row.id != "auto"
        }
    }
}

/// Whether a model row shows up without the user typing a search query
/// (#3830): `auto`, every catalog row for the active provider, and rows for
/// other providers once those providers are configured — the selected route
/// stays complete while cross-provider choices remain conservative.
fn model_row_visible_by_default(row: &ModelPickerRow, active_provider: ApiProvider) -> bool {
    row.provider.is_none() || row.provider == Some(active_provider) || row.enabled
}

fn sort_model_rows_for_view(
    rows: &mut [&ModelPickerRow],
    view: ModelListView,
    pins: &[PinnedModel],
) {
    let pin_rank = |row: &ModelPickerRow| {
        row_provider_identity(row)
            .and_then(|provider| {
                pins.iter().position(|pin| {
                    provider.eq_ignore_ascii_case(&pin.provider)
                        && row.id.eq_ignore_ascii_case(&pin.model)
                })
            })
            .unwrap_or(usize::MAX)
    };
    match view {
        ModelListView::Configured | ModelListView::Catalog => rows.sort_by_key(|row| pin_rank(row)),
        ModelListView::Recent => rows.sort_by(|left, right| {
            offering_fetched_at(right)
                .cmp(&offering_fetched_at(left))
                .then_with(|| left.id.cmp(&right.id))
        }),
        ModelListView::Coding => rows.sort_by(|left, right| {
            coding_score(right)
                .cmp(&coding_score(left))
                .then_with(|| left.id.cmp(&right.id))
        }),
        ModelListView::Cheap => rows.sort_by(|left, right| {
            match (
                input_price_per_million(left),
                input_price_per_million(right),
            ) {
                (Some(l), Some(r)) => l
                    .partial_cmp(&r)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.id.cmp(&right.id)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => left.id.cmp(&right.id),
            }
        }),
        ModelListView::LongContext => rows.sort_by(|left, right| {
            context_tokens(right)
                .cmp(&context_tokens(left))
                .then_with(|| left.id.cmp(&right.id))
        }),
    }
}

fn row_provider_identity(row: &ModelPickerRow) -> Option<&str> {
    row.provider_identity.as_deref().or_else(|| {
        row.provider
            .filter(|provider| *provider != ApiProvider::Custom)
            .map(ApiProvider::as_str)
    })
}

fn offering_for_row(row: &ModelPickerRow) -> Option<codewhale_config::catalog::CatalogOffering> {
    let provider = row.provider?;
    catalog_offering_for_model(provider, &row.id)
}

fn offering_fetched_at(row: &ModelPickerRow) -> u64 {
    match offering_for_row(row).map(|o| o.source) {
        Some(
            CatalogSource::Live { fetched_at, .. }
            | CatalogSource::CodewhaleLive { fetched_at, .. },
        ) => fetched_at,
        _ => 0,
    }
}

fn context_tokens(row: &ModelPickerRow) -> u64 {
    row.metadata.context_window.map(u64::from).unwrap_or(0)
}

fn input_price_per_million(row: &ModelPickerRow) -> Option<f64> {
    if matches!(row.metadata.pricing, PickerPricing::Unavailable) {
        return None;
    }
    offering_for_row(row)
        .and_then(|offering| OfferingPricing::from_catalog_offering(&offering))
        .and_then(|pricing| pricing.input_per_million)
}

fn coding_score(row: &ModelPickerRow) -> u32 {
    let mut score = 0_u32;
    if let Some(offering) = offering_for_row(row) {
        let text_ok = offering.modalities.as_ref().is_none_or(|modalities| {
            modalities.output.is_empty()
                || modalities
                    .output
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case("text"))
        });
        if text_ok {
            score += 40;
        }
    }
    if row.metadata.tool_calls == Some(true) {
        score += 40;
    }
    if row.metadata.reasoning {
        score += 10;
    }
    if row.metadata.context_window.unwrap_or(0) >= 100_000 {
        score += 10;
    }
    score
}

fn effective_picker_metadata(
    config: &Config,
    provider: Option<ApiProvider>,
    id: &str,
) -> EffectivePickerMetadata {
    effective_picker_metadata_with_codex(config, provider, id, None)
}

fn effective_picker_metadata_with_codex(
    config: &Config,
    provider: Option<ApiProvider>,
    id: &str,
    codex_metadata: Option<&CodexModelMetadata>,
) -> EffectivePickerMetadata {
    let offering = provider.and_then(|provider| catalog_offering_for_model(provider, id));
    let card = offering.as_ref().map(ModelReferenceCard::from_offering);
    let registry = model_registry::lookup(id);

    let Some(provider) = provider else {
        return EffectivePickerMetadata {
            context_window: registry.as_ref().and_then(|meta| meta.context_window),
            context_window_unverified: false,
            max_output: registry.as_ref().and_then(|meta| meta.max_output),
            max_output_unverified: false,
            tool_calls: None,
            reasoning: registry
                .as_ref()
                .is_some_and(|meta| meta.supports_reasoning),
            vision: SupportState::Unknown,
            pricing: if crate::pricing::has_pricing_for_model(id) {
                PickerPricing::Known("priced".to_string())
            } else {
                PickerPricing::Unknown
            },
            source: None,
        };
    };

    let context_override = config.context_window_for_provider_config(provider);
    let overrides = CapabilityOverride {
        context_window: context_override,
        ..CapabilityOverride::default()
    };
    let profile = offering.as_ref().map_or_else(
        || resolved_capability_profile_with_overrides(provider, id, overrides.clone()),
        |offering| {
            let route_offering = offering.to_offering();
            resolved_capability_profile_for_route_with_overrides(
                provider,
                id,
                route_offering.capabilities,
                route_offering.limits,
                overrides.clone(),
            )
        },
    );
    let card_context = card
        .as_ref()
        .and_then(|card| card.context_window)
        .map(|tokens| tokens.min(u64::from(u32::MAX)) as u32);
    let preserves_unknown_limits = offering.is_some()
        || (provider == ApiProvider::Together
            && id.eq_ignore_ascii_case(crate::config::TOGETHER_INKLING_MODEL));
    let context_window = if context_override.is_some() {
        profile.context_window
    } else if provider == ApiProvider::OpenaiCodex {
        codex_metadata.and_then(|metadata| metadata.context_window)
    } else if preserves_unknown_limits {
        card_context
    } else {
        profile.context_window
    };
    let card_output = card
        .as_ref()
        .and_then(|card| card.max_output)
        .map(|tokens| tokens.min(u64::from(u32::MAX)) as u32);
    // The Codex cache does not publish a route-owned output ceiling. The
    // profile's current value is inherited from the same-id OpenAI API model,
    // so omitting it is more truthful than claiming that API limit for OAuth.
    let max_output = if provider == ApiProvider::OpenaiCodex {
        None
    } else if preserves_unknown_limits {
        card_output
    } else {
        profile.max_output
    };
    let profile_tool_calls = match profile.native_tool_calls {
        SupportState::Supported => Some(true),
        SupportState::Unsupported => Some(false),
        SupportState::Unknown => None,
    };
    let tool_calls = if provider == ApiProvider::OpenaiCodex {
        codex_metadata.and(profile_tool_calls)
    } else {
        offering
            .as_ref()
            .and_then(|offering| offering.tool_call)
            .or(profile_tool_calls)
    };
    let reasoning = if provider == ApiProvider::OpenaiCodex {
        codex_metadata
            .map(|metadata| {
                metadata
                    .reasoning
                    .unwrap_or_else(|| profile.supports_reasoning())
            })
            .unwrap_or(false)
    } else {
        offering
            .as_ref()
            .and_then(|offering| offering.reasoning)
            .unwrap_or_else(|| profile.supports_reasoning())
    };
    let vision = profile.image_input;
    let card_price = card.as_ref().and_then(|card| {
        let label = card.price_label();
        (label != "unknown").then_some(label)
    });
    let pricing = if provider == ApiProvider::OpenaiCodex {
        PickerPricing::Unavailable
    } else if let Some(label) = card_price {
        PickerPricing::Known(label)
    } else if crate::pricing::has_pricing_for_provider(provider, id) {
        PickerPricing::Known("priced".to_string())
    } else {
        PickerPricing::Unknown
    };

    // Honesty rungs (#5239, #5440, #5441). A window that reached the picker
    // only through the legacy provider fallback — no offering, no catalog
    // row, no Codex roster, no operator override — is a guess (possibly an
    // `_Nk` name-suffix parse), and an unknown Anthropic-family model's
    // output ceiling is an assumed floor. Both still drive budgets; both
    // must say what they are instead of borrowing a verified label.
    let context_window_unverified = context_window.is_some()
        && context_override.is_none()
        && provider != ApiProvider::OpenaiCodex
        && !preserves_unknown_limits
        && crate::model_catalog::resolved_context_window(id).is_none();
    let max_output_unverified = max_output.is_some()
        && matches!(
            provider,
            ApiProvider::Anthropic | ApiProvider::MinimaxAnthropic | ApiProvider::Openmodel
        )
        && !preserves_unknown_limits
        && crate::models::max_output_tokens_for_model(id).is_none();

    EffectivePickerMetadata {
        context_window,
        context_window_unverified,
        max_output,
        max_output_unverified,
        tool_calls,
        reasoning,
        vision,
        pricing,
        source: card.map(|card| card.source),
    }
}

fn render_picker_model_hint(
    id: &str,
    provider: Option<ApiProvider>,
    metadata: &EffectivePickerMetadata,
    codex_freshness: Option<CodexModelCacheFreshness>,
) -> String {
    debug_assert_ne!(id, "auto", "Auto rows use the context-aware picker hint");

    let mut parts = Vec::new();

    // `k3` and `kimi-k3` are the same underlying model on two different
    // products, so bare ids read as a confusing duplicate. Name the route:
    // bare `k3` is the Kimi Code membership route (validated pairing with
    // the coding endpoint, #4687), `kimi-k3` is the direct open platform.
    if provider == Some(ApiProvider::Moonshot) {
        match id.trim().to_ascii_lowercase().as_str() {
            "k3" => parts.push("Kimi Code plan route".to_string()),
            "kimi-k3" | "moonshotai/kimi-k3" => parts.push("Moonshot direct route".to_string()),
            _ => {}
        }
    }

    if let Some(context_window) = metadata.context_window {
        // The ChatGPT/Codex OAuth roster reports account-scoped windows (e.g.
        // 272K for gpt-5.x) that differ from the API route's limits by
        // deliberate policy. Label the value as route-scoped so it reads as a
        // route fact, not a wrong generic model limit (TUI-DOG-016).
        if provider == Some(ApiProvider::OpenaiCodex) {
            parts.push(format!(
                "{} ctx · ChatGPT route",
                format_picker_context_window(u64::from(context_window))
            ));
        } else if provider == Some(ApiProvider::Moonshot)
            && id.trim().eq_ignore_ascii_case("k3")
            && context_window == crate::models::KIMI_CODE_K3_CONTEXT_WINDOW_TOKENS
        {
            // The membership route's real window is plan-tier dependent
            // (256K on lower tiers, up to 1M on higher ones); this default
            // is the safe floor, raisable via the provider's
            // `context_window` setting when the plan includes 1M.
            parts.push(format!(
                "{} ctx (plan floor; raise via context_window)",
                format_picker_context_window(u64::from(context_window))
            ));
        } else {
            let suffix = if metadata.context_window_unverified {
                " (unverified)"
            } else {
                ""
            };
            parts.push(format!(
                "{} ctx{}",
                format_picker_context_window(u64::from(context_window)),
                suffix
            ));
        }
    }

    if let Some(max_output) = metadata.max_output {
        let suffix = if metadata.max_output_unverified {
            " (assumed floor)"
        } else {
            ""
        };
        parts.push(format!(
            "{} out{}",
            format_picker_context_window(u64::from(max_output)),
            suffix
        ));
    }

    match metadata.tool_calls {
        Some(true) => parts.push("tools".to_string()),
        Some(false) => parts.push("no tools".to_string()),
        None => {}
    }

    if metadata.reasoning {
        parts.push("reasoning".to_string());
    }

    match metadata.vision {
        SupportState::Supported => parts.push("vision".to_string()),
        SupportState::Unsupported => parts.push("no vision".to_string()),
        SupportState::Unknown => {}
    }

    match &metadata.pricing {
        PickerPricing::Unavailable => {}
        PickerPricing::Known(label) => parts.push(label.clone()),
        PickerPricing::Unknown => parts.push("price unknown".to_string()),
    }
    match metadata.source.as_ref() {
        Some(
            CatalogSource::Live { .. }
            | CatalogSource::ModelsDevLive { .. }
            | CatalogSource::CodewhaleLive { .. },
        ) => parts.push("live".to_string()),
        Some(CatalogSource::Bundled | CatalogSource::CodewhaleBundled { .. }) => {
            parts.push("bundled".to_string())
        }
        Some(CatalogSource::ConfigOverride | CatalogSource::UserOverride) => {
            parts.push("override".to_string())
        }
        None => {}
    }
    if provider == Some(ApiProvider::OpenaiCodex) {
        parts.push(match codex_freshness {
            Some(freshness) => freshness.picker_label().to_string(),
            None => "custom · OAuth roster unconfirmed".to_string(),
        });
    }

    if parts.is_empty() {
        "provider model".to_string()
    } else {
        parts.join(" · ")
    }
}

pub(crate) fn format_picker_context_window(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        if tokens.is_multiple_of(1_000_000) {
            format!("{}M", tokens / 1_000_000)
        } else {
            format!("{:.2}M", tokens as f64 / 1_000_000.0)
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        }
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

impl ModelPickerView {
    /// Rebuild model rows from a fresh app/config snapshot (readiness + catalog).
    pub fn re_resolve_from_app(&mut self, app: &App, config: &Config) {
        let selected = self
            .visible_model_rows()
            .get(self.selected_model_idx)
            .map(|row| {
                (
                    row_provider_identity(row).map(str::to_string),
                    row.id.clone(),
                )
            });
        self.provider_health = app.provider_health.clone();
        self.route_config = config.clone();
        self.pinned_models = picker_pins_for_app(app);
        self.model_rows = picker_model_rows_for_app(app, config);
        self.configured_providers = configured_providers(config, app.api_provider)
            .into_iter()
            .filter(|provider| *provider != app.api_provider)
            .collect();
        // Re-anchor to the same exact provider/model after pin sorting changes;
        // preserving only the numeric index can select a different model.
        if let Some((provider, model)) = selected
            && let Some(position) = self.visible_model_rows().iter().position(|row| {
                row.id.eq_ignore_ascii_case(&model)
                    && row_provider_identity(row).map(str::to_owned) == provider
            })
        {
            self.selected_model_idx = position;
            return;
        }
        // Keep selection stable when the row still exists.
        let rows = self.visible_model_rows();
        if self.selected_model_idx >= rows.len() + usize::from(self.show_custom_model_row) {
            self.selected_model_idx = rows.len().saturating_sub(1);
        }
    }
}

impl ModelPickerView {
    fn emit_pin_move(&self, delta: isize) -> ViewAction {
        let rows = self.visible_model_rows();
        let Some(row) = rows.get(self.selected_model_idx) else {
            return ViewAction::None;
        };
        let Some(provider) = row.provider else {
            return ViewAction::None;
        };
        ViewAction::Emit(ViewEvent::ModelPickerMovePin {
            provider,
            provider_id: row.provider_identity.clone(),
            model: row.id.clone(),
            delta,
        })
    }
}

impl ModalView for ModelPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ModelPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            // Esc carries the browsing context out so the next open can
            // restore it (#4109 picker memory).
            KeyCode::Esc => ViewAction::EmitAndClose(ViewEvent::ModelPickerDismissed {
                catalog_view: self.view.browses_all_providers(),
                view: self.view.memory_name().to_string(),
                selected_row_id: {
                    let rows = self.visible_model_rows();
                    rows.get(self.selected_model_idx).map(|row| row.id.clone())
                },
            }),
            KeyCode::Enter if self.model_row_count() == 0 => ViewAction::None,
            KeyCode::Enter if !self.selected_model_is_selectable() => {
                // Never silently ignore Enter on locked models — surface the
                // readiness reason and offer provider setup.
                self.explain_unselectable_selection()
            }
            KeyCode::Enter => ViewAction::EmitAndClose(self.build_event()),
            // Shift+D makes the visible provider/model pair the startup
            // default. Plain Enter deliberately stays session-local, so a
            // one-off route comparison cannot silently change the next launch.
            KeyCode::Char(ch)
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    && self.query.is_empty()
                    && ch.eq_ignore_ascii_case(&'d')
                    && self.selected_model_is_selectable() =>
            {
                ViewAction::EmitAndClose(self.build_event_with_startup_default(true))
            }
            KeyCode::Char(ch)
                if key.modifiers.contains(KeyModifiers::SHIFT) && ch.eq_ignore_ascii_case(&'d') =>
            {
                self.explain_unselectable_selection()
            }
            // Pinning must never steal the first character of a route search:
            // use the explicitly shifted key advertised in the footer.
            KeyCode::Char('P') if key.modifiers == KeyModifiers::SHIFT && self.query.is_empty() => {
                let rows = self.visible_model_rows();
                let Some(row) = rows.get(self.selected_model_idx) else {
                    return ViewAction::None;
                };
                let Some(provider) = row.provider else {
                    return ViewAction::None;
                };
                ViewAction::Emit(ViewEvent::ModelPickerTogglePin {
                    provider,
                    provider_id: row.provider_identity.clone(),
                    model: row.id.clone(),
                })
            }
            // Same rule as pinning: a shifted key, never a search character.
            KeyCode::Char('F') if key.modifiers == KeyModifiers::SHIFT && self.query.is_empty() => {
                let rows = self.visible_model_rows();
                let Some(row) = rows.get(self.selected_model_idx) else {
                    return ViewAction::None;
                };
                let Some(provider) = row.provider else {
                    return ViewAction::None;
                };
                ViewAction::Emit(ViewEvent::ModelPickerToggleFleet {
                    provider,
                    provider_id: row.provider_identity.clone(),
                    model: row.id.clone(),
                })
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) && self.query.is_empty() => {
                self.emit_pin_move(-1)
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) && self.query.is_empty() => {
                self.emit_pin_move(1)
            }
            // Cycle catalog views (#4115) without shadowing a typed provider
            // name such as `anthropic` or `azure`.
            KeyCode::Char('A') if key.modifiers == KeyModifiers::SHIFT && self.query.is_empty() => {
                self.toggle_view();
                ViewAction::None
            }
            KeyCode::Char(ch)
                if self.focus == Pane::Model
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                let mut query = self.query.clone();
                query.push(ch);
                self.update_query(query);
                ViewAction::None
            }
            KeyCode::Backspace if self.focus == Pane::Model && !self.query.is_empty() => {
                let mut query = self.query.clone();
                query.pop();
                self.update_query(query);
                ViewAction::None
            }
            KeyCode::Up => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_down();
                ViewAction::None
            }
            KeyCode::PageUp => {
                for _ in 0..5 {
                    self.move_up();
                }
                ViewAction::None
            }
            KeyCode::PageDown => {
                for _ in 0..5 {
                    self.move_down();
                }
                ViewAction::None
            }
            KeyCode::Home => {
                match self.focus {
                    Pane::Model => {
                        self.selected_model_idx = 0;
                        self.select_effort_for_current_model();
                    }
                    Pane::Effort => {
                        self.selected_effort_idx = 0;
                        self.selected_effort_request = self.resolved_effort();
                    }
                }
                ViewAction::None
            }
            KeyCode::End => {
                match self.focus {
                    Pane::Model => {
                        self.selected_model_idx = self.model_row_count().saturating_sub(1);
                        self.select_effort_for_current_model();
                    }
                    Pane::Effort => {
                        self.selected_effort_idx = self.current_efforts().len().saturating_sub(1);
                        self.selected_effort_request = self.resolved_effort();
                    }
                }
                ViewAction::None
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Left | KeyCode::BackTab => {
                self.toggle_focus();
                ViewAction::None
            }
            // Explicit readiness + catalog refresh (safe, non-destructive).
            // Plain `r` remains a route-search character.
            KeyCode::Char('r') | KeyCode::Char('R')
                if key.modifiers == crossterm::event::KeyModifiers::CONTROL =>
            {
                ViewAction::Emit(ViewEvent::ModelPickerRefresh)
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.last_mouse_selected = None;
                self.move_up();
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.last_mouse_selected = None;
                self.move_down();
                ViewAction::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let clicked = self
                    .row_hitboxes
                    .borrow()
                    .iter()
                    .find_map(|(rect, pane, idx)| {
                        rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                            .then_some((*pane, *idx))
                    });
                let Some((pane, idx)) = clicked else {
                    return ViewAction::None;
                };
                let apply = self.last_mouse_selected == Some((pane, idx))
                    && self.focus == pane
                    && match pane {
                        Pane::Model => self.selected_model_idx == idx,
                        Pane::Effort => self.selected_effort_idx == idx,
                    };
                self.focus = pane;
                match pane {
                    Pane::Model => {
                        self.selected_model_idx = idx.min(self.model_row_count().saturating_sub(1));
                        self.select_effort_for_current_model();
                    }
                    Pane::Effort => {
                        self.selected_effort_idx =
                            idx.min(self.current_efforts().len().saturating_sub(1));
                        self.selected_effort_request = self.resolved_effort();
                    }
                }
                self.last_mouse_selected = Some((pane, idx));
                if apply && self.selected_model_is_selectable() {
                    ViewAction::EmitAndClose(self.build_event())
                } else if apply {
                    self.explain_unselectable_selection()
                } else {
                    ViewAction::None
                }
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_route(area, buf);
    }
}

impl ModelPickerView {
    fn render_route(&self, area: Rect, buf: &mut Buffer) {
        self.row_hitboxes.borrow_mut().clear();
        let inner = render_underwater_surface(
            area,
            buf,
            tr(self.locale, MessageId::RouteSurfaceTitle)
                .replace("{view}", self.view.title_label()),
        );

        // Say what the action does in model language. Provider changes are an
        // implementation detail of applying a cross-provider model row.
        let view_action: std::borrow::Cow<'static, str> = match self.view {
            ModelListView::Configured => tr(self.locale, MessageId::RouteBrowseCatalog),
            other => other.next().title_label().into(),
        };
        let mut footer_hints = vec![
            ActionHint::new("↑↓", tr(self.locale, MessageId::PickerActionMove)),
            ActionHint::new("Tab", tr(self.locale, MessageId::PickerActionSwitch)),
            ActionHint::new(
                tr(self.locale, MessageId::RouteActionType),
                tr(self.locale, MessageId::RouteActionSearchAnyModel),
            ),
            ActionHint::new("Enter", tr(self.locale, MessageId::PickerActionApply)),
            ActionHint::new(
                "⇧D",
                tr(self.locale, MessageId::PickerActionSetStartupDefault),
            ),
            ActionHint::new("⇧A", view_action),
        ];
        // Keep compact route modals focused on the core browse/apply actions;
        // wider shells have room to disclose the pin action too.
        if inner.width >= 72 {
            footer_hints.push(ActionHint::new(
                "⇧F",
                tr(self.locale, MessageId::PickerActionFleet),
            ));
            footer_hints.push(ActionHint::new(
                "⇧P",
                tr(self.locale, MessageId::PickerActionPin),
            ));
        }
        footer_hints.push(ActionHint::new(
            "Esc",
            tr(self.locale, MessageId::PickerActionCancel),
        ));
        let content = render_modal_footer(inner, buf, &footer_hints);

        let shell = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                ratatui::layout::Constraint::Length(3),
                ratatui::layout::Constraint::Min(1),
            ])
            .split(content);
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("─ {} ", tr(self.locale, MessageId::RoutePanelHeader)),
                    Style::default().fg(palette::WHALE_ACTION).bold(),
                ),
                Span::styled(
                    "──────────────────────── ",
                    Style::default().fg(palette::BORDER_COLOR),
                ),
                Span::styled(
                    format!(
                        "{}{}",
                        self.view.title_label(),
                        catalog_freshness_title_suffix()
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled(
                    " ─────────────────",
                    Style::default().fg(palette::BORDER_COLOR),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    format!("  {} ", tr(self.locale, MessageId::RouteProviderLabel)),
                    Style::default().fg(palette::WHALE_ACTION),
                ),
                Span::styled(
                    self.resolved_provider()
                        .unwrap_or(self.initial_provider)
                        .display_name(),
                    Style::default().fg(palette::TEXT_PRIMARY),
                ),
                Span::styled(
                    format!(" · {}", tr(self.locale, MessageId::RouteModelFirstAtomic)),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ]),
        ])
        .render(shell[0], buf);

        let layout = widen_model_pane(ListDetailLayout::split(shell[1], 24));

        let visible = self.visible_model_rows();
        let route_labels = route_labels_for_rows(&visible);
        let mut model_rows: Vec<PaneRow> = visible
            .iter()
            .map(|row| {
                let active = row.id == self.initial_model
                    && (row.provider.is_none() || row.provider == Some(self.initial_provider));
                match row.provider {
                    // `auto` is not a catalog offering; it keeps its explanatory
                    // prose, which now has the whole row to be truncated into
                    // instead of being dropped for not fitting.
                    None => PaneRow {
                        primary: row.id.clone(),
                        route: String::new(),
                        meta: vec![row.hint.clone()],
                        family: None,
                        active,
                    },
                    Some(provider) => PaneRow {
                        primary: row.id.clone(),
                        route: route_labels
                            .get(provider.as_str())
                            .cloned()
                            .unwrap_or_else(|| provider.display_name().to_string()),
                        meta: model_row_meta_chips(row),
                        family: catalog_family_for(provider, &row.id),
                        active,
                    },
                }
            })
            .collect();
        if let Some((model, provider)) = self.custom_model_row() {
            model_rows.push(PaneRow {
                primary: model,
                family: None,
                route: provider.display_name().to_string(),
                meta: vec![if self.query.trim().is_empty() {
                    "current (custom)".to_string()
                } else {
                    "custom route".to_string()
                }],
                active: false,
            });
        }
        let model_title = if self.query.trim().is_empty() {
            format!("Model · {}", self.view.title_label())
        } else {
            format!("Model: {}", self.query.trim())
        };
        self.render_pane(
            layout.list,
            buf,
            &model_title,
            model_rows,
            PaneRenderState {
                pane: Pane::Model,
                selected: self.selected_model_idx,
                focused: self.focus == Pane::Model,
            },
        );

        let effort_provider = self.resolved_provider().unwrap_or(self.initial_provider);
        let current_efforts = self.current_efforts();
        let selected_effort_idx = self
            .selected_effort_idx
            .min(current_efforts.len().saturating_sub(1));
        let effort_rows: Vec<PaneRow> = current_efforts
            .iter()
            .map(|effort| {
                let label = effort
                    .display_label_for_provider(effort_provider)
                    .to_string();
                let hint = match effort {
                    ReasoningEffort::Auto => "choose per turn".to_string(),
                    ReasoningEffort::Off => "no extra reasoning".to_string(),
                    ReasoningEffort::Minimal => "minimal reasoning".to_string(),
                    ReasoningEffort::Low => "lighter reasoning".to_string(),
                    ReasoningEffort::Medium => "balanced reasoning".to_string(),
                    ReasoningEffort::High => "deeper reasoning".to_string(),
                    ReasoningEffort::XHigh => "extra-high reasoning".to_string(),
                    ReasoningEffort::Ultra => "ultra reasoning".to_string(),
                    ReasoningEffort::Max => {
                        if effort_provider == ApiProvider::OpenaiCodex {
                            "extra-high reasoning".to_string()
                        } else {
                            "maximum reasoning".to_string()
                        }
                    }
                };
                PaneRow::effort(label, hint)
            })
            .collect();
        self.render_pane(
            layout.detail,
            buf,
            "Thinking",
            effort_rows,
            PaneRenderState {
                pane: Pane::Effort,
                selected: selected_effort_idx,
                focused: self.focus == Pane::Effort,
            },
        );
    }
}

pub(crate) fn picker_efforts_for_route(
    provider: ApiProvider,
    base_url: &str,
    wire_model: &str,
    model_is_auto: bool,
) -> Vec<ReasoningEffort> {
    if model_is_auto {
        return AUTO_MODEL_PICKER_EFFORTS.to_vec();
    }
    // Exact-route overrides still win over catalog metadata: Kimi Code K3 and
    // OpenAI Codex have wire dialects the generic Models.dev shape does not
    // fully describe.
    if crate::config::is_exact_kimi_code_k3_route(provider, base_url, wire_model) {
        return KIMI_CODE_K3_PICKER_EFFORTS.to_vec();
    }
    if provider == ApiProvider::OpenaiCodex {
        return CODEX_PICKER_EFFORTS.to_vec();
    }
    if let Some(catalog_efforts) = catalog_picker_efforts(provider, wire_model) {
        return catalog_efforts;
    }
    if matches!(
        provider,
        crate::config::ApiProvider::Deepseek | crate::config::ApiProvider::DeepseekCN
    ) {
        return DEEPSEEK_PICKER_EFFORTS.to_vec();
    }
    DEFAULT_PICKER_EFFORTS.to_vec()
}

/// Build thinking-tier rows from Models.dev `reasoning_options` when present.
///
/// Expected shape (already parsed onto the catalog offering):
/// `[{ "type": "effort", "values": ["high", "max"] }]`.
/// Non-effort option types (e.g. MiniMax `thinking`) are mapped when their
/// values collapse cleanly onto our tier vocabulary; unknown values are
/// skipped. Returns `None` when the catalog has no usable effort list so the
/// caller can keep the provider default rather than inventing tiers.
fn catalog_picker_efforts(provider: ApiProvider, wire_model: &str) -> Option<Vec<ReasoningEffort>> {
    let offering = catalog_offering_for_model(provider, wire_model)?;
    let mut efforts = Vec::new();
    let mut saw_effort_list = false;
    for option in &offering.reasoning_options {
        let option_type = option
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        // Prefer explicit effort lists; also accept thinking-mode lists whose
        // values map onto our tiers (adaptive→auto, disabled→off, always_on→max).
        if option_type != "effort" && option_type != "thinking" {
            continue;
        }
        let Some(values) = option.get("values").and_then(|value| value.as_array()) else {
            continue;
        };
        saw_effort_list = true;
        for value in values {
            let Some(raw) = value.as_str() else {
                continue;
            };
            if let Some(effort) = catalog_effort_value(raw)
                && !efforts.contains(&effort)
            {
                efforts.push(effort);
            }
        }
    }
    if !saw_effort_list || efforts.is_empty() {
        return None;
    }
    // Always offer Auto when the catalog published discrete tiers so the
    // operator can still leave the choice to the route default. Do not invent
    // Off unless the catalog said so — some models are always-on.
    if !efforts.contains(&ReasoningEffort::Auto) {
        efforts.insert(0, ReasoningEffort::Auto);
    }
    Some(efforts)
}

fn catalog_effort_value(raw: &str) -> Option<ReasoningEffort> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "false" => Some(ReasoningEffort::Off),
        "none" => Some(ReasoningEffort::Off), // Muse "none" maps to Off in our enum but display as "none"
        "minimal" | "minimum" => Some(ReasoningEffort::Minimal),
        "low" | "light" => Some(ReasoningEffort::Low),
        "medium" | "mid" => Some(ReasoningEffort::Medium),
        "high" => Some(ReasoningEffort::High),
        "xhigh" => Some(ReasoningEffort::XHigh),
        "ultra" | "ultracode" => Some(ReasoningEffort::Ultra),
        "max" | "maximum" => Some(ReasoningEffort::Max),
        "auto" | "automatic" | "adaptive" => Some(ReasoningEffort::Auto),
        "always_on" | "always-on" => Some(ReasoningEffort::Max),
        _ => None,
    }
}

fn normalize_picker_effort(
    effort: ReasoningEffort,
    provider: ApiProvider,
    base_url: &str,
    wire_model: &str,
    model_is_auto: bool,
) -> ReasoningEffort {
    let normalized = if model_is_auto {
        effort
    } else {
        effort.normalize_for_route(provider, base_url, wire_model)
    };
    let efforts = picker_efforts_for_route(provider, base_url, wire_model, model_is_auto);
    if efforts.contains(&normalized) {
        return normalized;
    }
    // Catalog-driven lists may keep Low/Medium that route normalization would
    // otherwise collapse. Prefer the operator's exact choice when the picker
    // still shows it.
    if efforts.contains(&effort) {
        return effort;
    }
    default_picker_effort(provider, &efforts)
}

fn default_picker_effort(provider: ApiProvider, efforts: &[ReasoningEffort]) -> ReasoningEffort {
    let preferred = if provider == ApiProvider::OpenaiCodex {
        ReasoningEffort::Medium
    } else {
        ReasoningEffort::High
    };
    if efforts.contains(&preferred) {
        preferred
    } else {
        efforts
            .iter()
            .copied()
            .find(|effort| *effort != ReasoningEffort::Auto && *effort != ReasoningEffort::Off)
            .or_else(|| efforts.first().copied())
            .unwrap_or(preferred)
    }
}

fn default_picker_effort_idx(
    provider: ApiProvider,
    base_url: &str,
    wire_model: &str,
    model_is_auto: bool,
) -> usize {
    let efforts = picker_efforts_for_route(provider, base_url, wire_model, model_is_auto);
    let default_effort = default_picker_effort(provider, &efforts);
    efforts
        .iter()
        .position(|effort| *effort == default_effort)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_row(provider: ApiProvider, enabled: bool) -> ModelPickerRow {
        ModelPickerRow {
            id: "model".to_string(),
            provider: Some(provider),
            provider_identity: None,
            hint: String::new(),
            metadata: EffectivePickerMetadata::default(),
            selectable: true,
            blocked_reason: None,
            enabled,
        }
    }

    fn test_picker() -> ModelPickerView {
        ModelPickerView {
            initial_model: "model".to_string(),
            previous_model: "model".to_string(),
            initial_provider: ApiProvider::Openai,
            initial_effort: ReasoningEffort::Auto,
            selected_effort_request: ReasoningEffort::Auto,
            active_accepts_custom_model_ids: false,
            query: String::new(),
            selected_model_idx: 0,
            selected_effort_idx: 0,
            focus: Pane::Model,
            show_custom_model_row: false,
            model_rows: vec![model_row(ApiProvider::Openai, true)],
            route_config: Config::default(),
            provider_health: Default::default(),
            view: ModelListView::Configured,
            configured_providers: Vec::new(),
            row_hitboxes: RefCell::new(Vec::new()),
            last_mouse_selected: None,
            locale: Locale::En,
            pinned_models: Vec::new(),
        }
    }

    fn render_text(picker: &ModelPickerView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        picker.render(area, &mut buffer);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn deepseek_picker_heading_hides_legacy_family_metadata() {
        assert_eq!(
            catalog_family_for(ApiProvider::Deepseek, "deepseek-v4-pro").as_deref(),
            Some("DeepSeek")
        );
    }

    #[test]
    fn configured_view_keeps_active_provider_catalog_models_visible() {
        let row = model_row(ApiProvider::Deepseek, false);

        assert!(model_row_visible_by_default(&row, ApiProvider::Deepseek));
        assert!(!model_row_visible_by_default(&row, ApiProvider::Openai));
    }

    #[test]
    fn lowercase_picker_action_letters_begin_a_model_search() {
        for ch in ['a', 'p', 'r'] {
            let mut picker = test_picker();
            assert!(matches!(
                picker.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
                ViewAction::None
            ));
            assert_eq!(picker.query, ch.to_string(), "{ch} must begin a search");
            assert_eq!(picker.view, ModelListView::Configured);
        }
    }

    #[test]
    fn shifted_picker_actions_cycle_views_pin_and_refresh_explicitly() {
        let mut picker = test_picker();

        assert!(matches!(
            picker.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            ViewAction::None
        ));
        assert_eq!(picker.view, ModelListView::Catalog);
        assert!(picker.query.is_empty());

        assert!(matches!(
            picker.handle_key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT)),
            ViewAction::Emit(ViewEvent::ModelPickerTogglePin {
                provider: ApiProvider::Openai,
                provider_id: None,
                model,
            }) if model == "model"
        ));
        assert!(picker.query.is_empty());

        assert!(matches!(
            picker.handle_key(KeyEvent::new(KeyCode::Char('F'), KeyModifiers::SHIFT)),
            ViewAction::Emit(ViewEvent::ModelPickerToggleFleet {
                provider: ApiProvider::Openai,
                provider_id: None,
                model,
            }) if model == "model"
        ));
        assert!(picker.query.is_empty());

        assert!(matches!(
            picker.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            ViewAction::Emit(ViewEvent::ModelPickerRefresh)
        ));
    }

    #[test]
    fn fleet_models_lead_the_pins_the_picker_sorts_by() {
        let _lock = crate::test_support::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let workspace = temp.path().join("repo");
        std::fs::create_dir_all(&workspace).expect("workspace");
        crate::fleet::members::add_fleet_model(
            &workspace,
            "openrouter",
            "z-ai/glm-5.3-flash",
            &["scout".to_string()],
        )
        .expect("fleet add");

        let options = crate::tui::app::TuiOptions {
            ..crate::test_support::test_tui_options(workspace.clone())
        };
        let mut app = crate::tui::app::App::new(options, &Config::default());
        app.workspace = workspace;
        app.pinned_models = vec![PinnedModel {
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5".to_string(),
            label: None,
        }];

        let pins = picker_pins_for_app(&app);
        assert_eq!(pins.len(), 2, "fleet model then the person's pin: {pins:?}");
        assert_eq!(pins[0].provider, "openrouter");
        assert_eq!(pins[0].model, "z-ai/glm-5.3-flash");
        assert_eq!(pins[0].label.as_deref(), Some("fleet · explore"));
        assert_eq!(pins[1].model, "claude-haiku-4-5");
    }

    #[test]
    fn wide_picker_footer_advertises_shifted_view_and_pin_actions() {
        let picker = test_picker();
        let text = render_text(&picker, 100, 30);

        assert!(text.contains("⇧A"), "missing shifted view hint: {text}");
        assert!(text.contains("⇧P"), "missing shifted pin hint: {text}");
        assert!(text.contains("⇧F"), "missing shifted fleet hint: {text}");
    }
}
