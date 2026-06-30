//! `/model` picker modal: pick a model and thinking-effort tier (#39, #2026).
//!
//! The picker intentionally presents model and thinking as independent choices
//! instead of collapsing them into preset route names. The "auto" option is
//! always available; custom (unrecognized) model ids appear as a separate row.
//! Pass-through providers fall back to only "auto" plus the current custom row.
//!
//! On apply we emit a [`ViewEvent::ModelPickerApplied`] with the resolved
//! model id and effort tier.

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::config::{ApiProvider, Config, model_completion_names_for_provider};
use crate::model_registry;
use crate::palette;
use crate::tui::app::{App, ReasoningEffort};
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer, render_modal_surface,
};

/// Thinking-effort rows shown for DeepSeek-style providers, in the order
/// DeepSeek behaviorally distinguishes them.
const DEFAULT_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Auto,
    ReasoningEffort::Off,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];
const CODEX_PICKER_EFFORTS: &[ReasoningEffort] = &[
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::Max,
];
const AUTO_MODEL_PICKER_EFFORTS: &[ReasoningEffort] = &[ReasoningEffort::Auto];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Model,
    Effort,
}

pub struct ModelPickerView {
    initial_model: String,
    initial_provider: ApiProvider,
    initial_effort: ReasoningEffort,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelPickerRow {
    id: String,
    provider: Option<ApiProvider>,
    hint: String,
}

impl ModelPickerView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let initial_model = if app.auto_model {
            "auto".to_string()
        } else {
            app.model.clone()
        };
        let model_rows = picker_model_rows_for_app(app);
        let configured_providers = configured_providers_for(config, app.api_provider);
        let mut selected_model_idx = model_rows.iter().position(|row| {
            row.id == initial_model
                && (row.provider.is_none() || row.provider == Some(app.api_provider))
        });
        let show_custom_model_row = selected_model_idx.is_none();
        if show_custom_model_row {
            // The custom row is conceptually appended right after every row
            // the default (empty-query) view shows, so its index must match
            // that count, not just the active-provider-scoped one (#3830) —
            // `resolved_model`/`model_row_count` treat any selection at or
            // past `visible_model_rows().len()` as "the custom row."
            selected_model_idx = Some(default_visible_model_row_count(
                &model_rows,
                app.api_provider,
                &configured_providers,
            ));
        }
        let selected_model_idx = selected_model_idx.unwrap_or(0);

        let initial_effort = app.reasoning_effort;
        let effort_rows = picker_efforts_for_provider(app.api_provider, app.auto_model);
        let normalized = normalize_picker_effort(initial_effort, app.api_provider, app.auto_model);
        let selected_effort_idx = effort_rows
            .iter()
            .position(|e| *e == normalized)
            .unwrap_or_else(|| default_picker_effort_idx(app.api_provider, app.auto_model));

        Self {
            initial_model,
            initial_provider: app.api_provider,
            initial_effort,
            active_accepts_custom_model_ids: app.accepts_custom_model_ids(),
            query: String::new(),
            selected_model_idx,
            selected_effort_idx,
            focus: Pane::Model,
            show_custom_model_row,
            model_rows,
            configured_providers,
        }
    }

    #[cfg(test)]
    fn visible_model_ids(&self) -> Vec<&str> {
        self.visible_model_rows()
            .iter()
            .map(|row| row.id.as_str())
            .collect()
    }

    fn visible_model_rows(&self) -> Vec<&ModelPickerRow> {
        let query = self.query.trim();
        self.model_rows
            .iter()
            .filter(|row| {
                if query.is_empty() {
                    model_row_visible_by_default(
                        row.provider,
                        self.initial_provider,
                        &self.configured_providers,
                    )
                } else {
                    model_row_matches_query(row, query, self.initial_provider)
                }
            })
            .collect()
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
        if self.resolved_model().trim().eq_ignore_ascii_case("auto") {
            return ReasoningEffort::Auto;
        }
        let efforts = self.current_efforts();
        efforts[self
            .selected_effort_idx
            .min(efforts.len().saturating_sub(1))]
    }

    fn current_efforts(&self) -> &'static [ReasoningEffort] {
        picker_efforts_for_provider(
            self.resolved_provider().unwrap_or(self.initial_provider),
            self.resolved_model().trim().eq_ignore_ascii_case("auto"),
        )
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

    fn clamp_model_selection(&mut self) {
        let count = self.model_row_count();
        if count == 0 {
            self.selected_model_idx = 0;
        } else if self.selected_model_idx >= count {
            self.selected_model_idx = count - 1;
        }
    }

    fn update_query(&mut self, next: String) {
        let effort = self.resolved_effort();
        self.query = next;
        self.selected_model_idx = 0;
        self.clamp_model_selection();
        self.select_effort_for_current_model(effort);
    }

    fn select_effort_for_current_model(&mut self, effort: ReasoningEffort) {
        let provider = self.resolved_provider().unwrap_or(self.initial_provider);
        let model_is_auto = self.resolved_model().trim().eq_ignore_ascii_case("auto");
        let normalized = normalize_picker_effort(effort, provider, model_is_auto);
        self.selected_effort_idx = picker_efforts_for_provider(provider, model_is_auto)
            .iter()
            .position(|candidate| *candidate == normalized)
            .unwrap_or_else(|| default_picker_effort_idx(provider, model_is_auto));
    }

    fn move_up(&mut self) -> bool {
        match self.focus {
            Pane::Model => {
                if self.selected_model_idx > 0 {
                    let effort = self.resolved_effort();
                    self.selected_model_idx -= 1;
                    self.select_effort_for_current_model(effort);
                    return true;
                }
            }
            Pane::Effort => {
                if self.selected_effort_idx > 0 {
                    self.selected_effort_idx -= 1;
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
                    let effort = self.resolved_effort();
                    self.selected_model_idx += 1;
                    self.select_effort_for_current_model(effort);
                    return true;
                }
            }
            Pane::Effort => {
                let max = self.current_efforts().len().saturating_sub(1);
                if self.selected_effort_idx < max {
                    self.selected_effort_idx += 1;
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

    fn build_event(&self) -> ViewEvent {
        let provider = self
            .resolved_provider()
            .filter(|provider| *provider != self.initial_provider);
        ViewEvent::ModelPickerApplied {
            model: self.resolved_model(),
            provider,
            effort: self.resolved_effort(),
            previous_model: self.initial_model.clone(),
            previous_effort: self.initial_effort,
        }
    }

    fn render_pane(
        &self,
        area: Rect,
        buf: &mut Buffer,
        title: &str,
        rows: Vec<(String, String)>,
        selected: usize,
        focused: bool,
    ) {
        let border_style = if focused {
            Style::default().fg(palette::DEEPSEEK_SKY)
        } else {
            Style::default().fg(palette::BORDER_COLOR)
        };
        let visible_height = usize::from(area.height.saturating_sub(2));
        let (start, end) = visible_row_window(selected, rows.len(), visible_height);
        let title = if rows.len() > visible_height && visible_height > 0 {
            format!(" {title} {}-{}/{} ", start + 1, end, rows.len())
        } else {
            format!(" {title} ")
        };
        let block = Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default().fg(palette::TEXT_PRIMARY).bold(),
            )))
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default());
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = Vec::with_capacity(end.saturating_sub(start));
        for (idx, (label, hint)) in rows.iter().enumerate().skip(start).take(end - start) {
            let is_selected = idx == selected;
            let marker = if is_selected { "▸" } else { " " };
            let label_style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            let spans = picker_row_spans(
                label,
                hint,
                marker,
                usize::from(inner.width),
                label_style,
                hint_style,
            );
            lines.push(Line::from(spans));
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

fn picker_row_spans<'a>(
    label: &'a str,
    hint: &'a str,
    marker: &'static str,
    width: usize,
    label_style: Style,
    hint_style: Style,
) -> Vec<Span<'a>> {
    let prefix_width = 3;
    let label_width = width.saturating_sub(prefix_width);
    let label = fit_text(label, label_width);
    let mut spans = vec![
        Span::styled(" ", label_style),
        Span::styled(marker, label_style),
        Span::styled(" ", label_style),
        Span::styled(label, label_style),
    ];

    if !hint.is_empty() {
        let hint_text = format!("  ({hint})");
        let used = prefix_width
            + unicode_width::UnicodeWidthStr::width(
                spans
                    .last()
                    .map(|span| span.content.as_ref())
                    .unwrap_or_default(),
            );
        if used + unicode_width::UnicodeWidthStr::width(hint_text.as_str()) <= width {
            spans.push(Span::styled(hint_text, hint_style));
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

#[cfg(test)]
fn picker_model_ids_for_provider(provider: ApiProvider) -> Vec<&'static str> {
    let mut models = vec!["auto"];
    for id in model_completion_names_for_provider(provider) {
        if id != "auto" && !models.contains(&id) {
            models.push(id);
        }
    }
    models
}

pub(crate) fn provider_scoped_model_completion_ids(app: &App) -> Vec<String> {
    // Slash completions inline the current custom model so `/model <current>`
    // stays visible even when it is outside the provider catalog.
    provider_scoped_model_ids_for_app(app, true)
}

fn picker_model_rows_for_app(app: &App) -> Vec<ModelPickerRow> {
    let mut rows = Vec::new();
    push_provider_model_rows(
        &mut rows,
        app.api_provider,
        provider_scoped_model_ids_for_app(app, false),
        app.api_provider,
    );

    for provider in ApiProvider::sorted_for_display() {
        if provider == app.api_provider {
            continue;
        }
        let mut model_ids = provider_catalog_model_ids(provider);
        if let Some(model) = app
            .provider_models
            .get(provider.as_str())
            .map(|model| model.trim())
            .filter(|model| !model.is_empty())
        {
            push_model_id(&mut model_ids, model);
        }
        push_provider_model_rows(&mut rows, provider, model_ids, app.api_provider);
    }

    rows
}

fn push_provider_model_rows(
    rows: &mut Vec<ModelPickerRow>,
    provider: ApiProvider,
    model_ids: Vec<String>,
    active_provider: ApiProvider,
) {
    for id in model_ids {
        if id == "auto" {
            push_model_row(rows, id, None, picker_model_hint("auto"));
        } else {
            let mut hint = picker_model_hint(&id);
            if provider != active_provider {
                hint = format!("switch route · {hint}");
            }
            push_model_row(rows, id.clone(), Some(provider), hint);
        }
    }
}

fn provider_catalog_model_ids(provider: ApiProvider) -> Vec<String> {
    let mut models = Vec::new();
    for id in model_completion_names_for_provider(provider) {
        if id != "auto" {
            push_model_id(&mut models, id);
        }
    }
    models
}

fn provider_scoped_model_ids_for_app(app: &App, include_current_model: bool) -> Vec<String> {
    // `include_current_model` is for completion surfaces that do not have a
    // separate custom/current-model row.
    let mut models = Vec::new();
    push_model_id(&mut models, "auto");
    for id in model_completion_names_for_provider(app.api_provider) {
        push_model_id(&mut models, id);
    }

    if let Some(model) = app
        .provider_models
        .get(app.api_provider.as_str())
        .map(|model| model.trim())
        .filter(|model| !model.is_empty())
    {
        push_model_id(&mut models, model);
    }

    if include_current_model && !app.auto_model {
        push_model_id(&mut models, app.model.trim());
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

fn push_model_row(
    rows: &mut Vec<ModelPickerRow>,
    id: String,
    provider: Option<ApiProvider>,
    hint: String,
) {
    if rows
        .iter()
        .any(|row| row.id == id && row.provider == provider)
    {
        return;
    }
    rows.push(ModelPickerRow { id, provider, hint });
}

fn model_row_matches_query(
    row: &ModelPickerRow,
    query: &str,
    initial_provider: ApiProvider,
) -> bool {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return true;
    }
    let provider_matches = row.provider.is_some_and(|provider| {
        provider.as_str().contains(&query)
            || provider
                .display_name()
                .to_ascii_lowercase()
                .contains(&query)
    });
    provider_matches
        || row.id.to_ascii_lowercase().contains(&query)
        || ((row.provider.is_none() || row.provider == Some(initial_provider))
            && row.hint.to_ascii_lowercase().contains(&query))
}

fn model_row_label(row: &ModelPickerRow, initial_provider: ApiProvider) -> String {
    match row.provider {
        Some(provider) if provider != initial_provider => {
            format!("{} · {}", provider.display_name(), row.id)
        }
        _ => row.id.clone(),
    }
}

/// Whether a model row shows up without the user typing a search query
/// (#3830): `auto`, the active provider's own rows, and any other
/// provider's rows once that provider is "configured" — same definition the
/// `/provider` manager's default view uses.
fn model_row_visible_by_default(
    row_provider: Option<ApiProvider>,
    initial_provider: ApiProvider,
    configured_providers: &[ApiProvider],
) -> bool {
    match row_provider {
        None => true,
        Some(provider) => provider == initial_provider || configured_providers.contains(&provider),
    }
}

/// Count of rows the default (empty-query) view shows — i.e. how many
/// `model_rows` entries satisfy [`model_row_visible_by_default`]. Used to
/// position the custom-model row right after them (#3830).
fn default_visible_model_row_count(
    rows: &[ModelPickerRow],
    initial_provider: ApiProvider,
    configured_providers: &[ApiProvider],
) -> usize {
    rows.iter()
        .filter(|row| {
            model_row_visible_by_default(row.provider, initial_provider, configured_providers)
        })
        .count()
}

/// Providers other than `active` that should be treated as "configured" for
/// the `/model` picker's default view (#3830). Reuses the same predicate the
/// `/provider` manager applies so the two pickers never disagree about what
/// "configured" means.
fn configured_providers_for(config: &Config, active: ApiProvider) -> Vec<ApiProvider> {
    ApiProvider::sorted_for_display()
        .into_iter()
        .filter(|provider| {
            *provider != active
                && crate::config::provider_is_configured_for_active(config, *provider, active)
        })
        .collect()
}

fn picker_model_hint(id: &str) -> String {
    if id == "auto" {
        return "select per turn".to_string();
    }
    let Some(metadata) = model_registry::lookup(id) else {
        return "provider model".to_string();
    };

    let mut parts = Vec::new();
    if let Some(context_window) = metadata.context_window {
        parts.push(format!(
            "{} ctx",
            format_picker_context_window(context_window)
        ));
    }
    if metadata.supports_reasoning {
        parts.push("reasoning".to_string());
    }
    parts.push(if crate::pricing::has_pricing_for_model(id) {
        "priced".to_string()
    } else {
        "price unknown".to_string()
    });
    parts.join(" · ")
}

fn format_picker_context_window(tokens: u32) -> String {
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

impl ModalView for ModelPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ModelPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Enter if self.model_row_count() == 0 => ViewAction::None,
            KeyCode::Enter => ViewAction::EmitAndClose(self.build_event()),
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
                        let effort = self.resolved_effort();
                        self.selected_model_idx = 0;
                        self.select_effort_for_current_model(effort);
                    }
                    Pane::Effort => self.selected_effort_idx = 0,
                }
                ViewAction::None
            }
            KeyCode::End => {
                match self.focus {
                    Pane::Model => {
                        let effort = self.resolved_effort();
                        self.selected_model_idx = self.model_row_count().saturating_sub(1);
                        self.select_effort_for_current_model(effort);
                    }
                    Pane::Effort => {
                        self.selected_effort_idx = self.current_efforts().len().saturating_sub(1);
                    }
                }
                ViewAction::None
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Left | KeyCode::BackTab => {
                self.toggle_focus();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.move_up();
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.move_down();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_classic(area, buf);
    }
}

impl ModelPickerView {
    fn render_classic(&self, area: Rect, buf: &mut Buffer) {
        let desired_height = (self.model_row_count().max(self.current_efforts().len()) as u16)
            .saturating_add(4)
            .clamp(10, 22);
        let popup_area = centered_modal_area(area, 96, desired_height, 60, 10);

        render_modal_surface(area, popup_area, buf);

        // Outer chrome with title; the action footer moves into the body so it
        // wraps instead of clipping at narrow widths (#3732).
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Model & thinking ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK));
        let inner = outer.inner(popup_area);
        outer.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("↑↓", "move"),
                ActionHint::new("Tab", "switch"),
                ActionHint::new("Type", "filter"),
                ActionHint::new("Enter", "apply"),
                ActionHint::new("Esc", "cancel"),
            ],
        );

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
            .split(content);

        let mut model_rows: Vec<(String, String)> = self
            .visible_model_rows()
            .iter()
            .map(|row| {
                (
                    model_row_label(row, self.initial_provider),
                    row.hint.clone(),
                )
            })
            .collect();
        if let Some((model, provider)) = self.custom_model_row() {
            let label = if self.query.trim().is_empty() {
                model
            } else {
                format!("{} · {}", provider.display_name(), model)
            };
            let hint = if self.query.trim().is_empty() {
                "current (custom)".to_string()
            } else {
                "custom route".to_string()
            };
            model_rows.push((label, hint));
        }
        let model_title = if self.query.trim().is_empty() {
            "Model".to_string()
        } else {
            format!("Model: {}", self.query.trim())
        };
        self.render_pane(
            columns[0],
            buf,
            &model_title,
            model_rows,
            self.selected_model_idx,
            self.focus == Pane::Model,
        );

        let effort_provider = self.resolved_provider().unwrap_or(self.initial_provider);
        let current_efforts = self.current_efforts();
        let selected_effort_idx = self
            .selected_effort_idx
            .min(current_efforts.len().saturating_sub(1));
        let effort_rows: Vec<(String, String)> = current_efforts
            .iter()
            .map(|effort| {
                let label = effort
                    .display_label_for_provider(effort_provider)
                    .to_string();
                let hint = match effort {
                    ReasoningEffort::Auto => "choose per turn".to_string(),
                    ReasoningEffort::Off => "no extra reasoning".to_string(),
                    ReasoningEffort::Low => "lighter reasoning".to_string(),
                    ReasoningEffort::Medium => "balanced reasoning".to_string(),
                    ReasoningEffort::High => "deeper reasoning".to_string(),
                    ReasoningEffort::Max => {
                        if effort_provider == ApiProvider::OpenaiCodex {
                            "extra-high reasoning".to_string()
                        } else {
                            "maximum reasoning".to_string()
                        }
                    }
                };
                (label, hint)
            })
            .collect();
        self.render_pane(
            columns[1],
            buf,
            "Thinking",
            effort_rows,
            selected_effort_idx,
            self.focus == Pane::Effort,
        );
    }
}

fn picker_efforts_for_provider(
    provider: ApiProvider,
    model_is_auto: bool,
) -> &'static [ReasoningEffort] {
    if model_is_auto {
        return AUTO_MODEL_PICKER_EFFORTS;
    }
    match provider {
        ApiProvider::OpenaiCodex => CODEX_PICKER_EFFORTS,
        _ => DEFAULT_PICKER_EFFORTS,
    }
}

fn normalize_picker_effort(
    effort: ReasoningEffort,
    provider: ApiProvider,
    model_is_auto: bool,
) -> ReasoningEffort {
    if model_is_auto {
        return ReasoningEffort::Auto;
    }
    if provider == ApiProvider::OpenaiCodex {
        return effort.normalize_for_provider(provider);
    }
    match effort {
        ReasoningEffort::Low | ReasoningEffort::Medium => ReasoningEffort::High,
        other => other,
    }
}

fn default_picker_effort_idx(provider: ApiProvider, model_is_auto: bool) -> usize {
    let default_effort = if model_is_auto {
        ReasoningEffort::Auto
    } else if provider == ApiProvider::OpenaiCodex {
        ReasoningEffort::Medium
    } else {
        ReasoningEffort::High
    };
    picker_efforts_for_provider(provider, model_is_auto)
        .iter()
        .position(|effort| *effort == default_effort)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, TuiOptions};
    use std::path::PathBuf;

    /// `_lock` bundles the process-wide test-env mutex with a guard that
    /// neutralizes the real Codex CLI OAuth login on disk (`~/.codex/auth.json`),
    /// if any — `has_api_key_for` checks `crate::oauth::auth_file_path().exists()`
    /// unconditionally for `OpenaiCodex` (#3830), so without this, "default view
    /// shows only configured providers" tests would pass or fail depending on
    /// whether the machine running them happens to have a prior Codex login.
    /// Declared in this order so the env var is restored (dropped first) while
    /// the mutex is still held, before the mutex itself is released.
    fn create_test_app() -> (
        App,
        Config,
        (
            crate::test_support::EnvVarGuard,
            std::sync::MutexGuard<'static, ()>,
        ),
    ) {
        let lock = crate::test_support::lock_test_env();
        let codex_auth_guard = crate::test_support::EnvVarGuard::set(
            "OPENAI_CODEX_AUTH_FILE",
            "/nonexistent/codewhale-test-codex-auth.json",
        );
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let config = Config::default();
        let mut app = App::new(options, &config);
        // App::new merges in the user's persisted settings.toml, which can override
        // the model, effort, and provider with whatever the developer
        // happens to have saved. Pin all three back to known values so
        // the picker tests below exercise the picker logic, not the
        // user's environment. In particular `api_provider` matters because
        // pass-through providers (Ollama, OpenAI) hide the DeepSeek model
        // rows and leave only `auto` + custom — Down has nowhere to go.
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Max;
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model_ids_passthrough = false;
        app.provider_models.clear();
        (app, config, (codex_auth_guard, lock))
    }

    fn type_model_query(view: &mut ModelPickerView, query: &str) {
        for ch in query.chars() {
            view.handle_key(KeyEvent::new(
                KeyCode::Char(ch),
                crossterm::event::KeyModifiers::NONE,
            ));
        }
    }

    fn buffer_row_text(buf: &Buffer, area: Rect, y: u16) -> String {
        (area.x..area.x.saturating_add(area.width))
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    fn row_containing(buf: &Buffer, area: Rect, needle: &str) -> Option<u16> {
        (area.y..area.y.saturating_add(area.height))
            .find(|&y| buffer_row_text(buf, area, y).contains(needle))
    }

    #[test]
    fn model_picker_hint_uses_model_registry_metadata() {
        let hint = picker_model_hint("minimax/minimax-m3");
        assert!(
            hint.contains("1M ctx"),
            "hint should include registry context window: {hint}"
        );
        assert!(
            hint.contains("reasoning"),
            "hint should include registry reasoning support: {hint}"
        );
        assert!(
            hint.contains("priced"),
            "hint should include pricing availability: {hint}"
        );
    }

    #[test]
    fn picker_main_rows_are_scoped_to_active_provider() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Together;
        app.model = crate::config::DEFAULT_TOGETHER_MODEL.to_string();
        app.provider_models.insert(
            "openrouter".to_string(),
            crate::config::DEFAULT_OPENROUTER_MODEL.to_string(),
        );

        let view = ModelPickerView::new(&app, &config);

        assert!(
            view.visible_model_rows()
                .iter()
                .all(|row| row.provider.is_none()
                    || row.provider == Some(crate::config::ApiProvider::Together))
        );
        assert!(
            !view
                .visible_model_ids()
                .contains(&crate::config::DEFAULT_OPENROUTER_MODEL),
            "OpenRouter saved rows must not appear as bare Together model choices"
        );
    }

    #[test]
    fn picker_default_view_includes_explicitly_configured_provider_rows() {
        // #3830: an explicit `[providers.together]` entry (base URL override,
        // no key) makes Together "configured," so its model rows surface in
        // the default (no-query) view alongside DeepSeek's own rows and
        // `auto` — not just when the user types a search query.
        let (mut app, _default_config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;

        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                together: crate::config::ProviderConfig {
                    base_url: Some("https://custom.together.example/v1".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };

        let view = ModelPickerView::new(&app, &config);
        let visible_ids = view.visible_model_ids();

        assert!(
            view.visible_model_rows()
                .iter()
                .any(|row| row.provider == Some(crate::config::ApiProvider::Together)),
            "explicitly configured Together should surface rows by default: {visible_ids:?}"
        );
        assert!(visible_ids.contains(&crate::config::DEFAULT_TOGETHER_MODEL));
        // Auto and the active provider's own rows are still present.
        assert!(visible_ids.contains(&"auto"));
        assert!(visible_ids.contains(&"deepseek-v4-pro"));
    }

    #[test]
    fn picker_default_view_excludes_self_hosted_provider_without_explicit_setup() {
        // #3830: `has_api_key_for` reports `true` unconditionally for
        // self-hosted providers (no auth required to route to them) — that
        // alone must not surface Sglang/Vllm in the default view for every
        // user. Sglang (unlike Ollama) has real catalog model ids, so it's a
        // meaningful row to check rather than an empty contribution.
        let (mut app, _default_config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        let config = Config::default();

        let view = ModelPickerView::new(&app, &config);
        assert!(
            !view
                .visible_model_rows()
                .iter()
                .any(|row| row.provider == Some(crate::config::ApiProvider::Sglang)),
            "self-hosted Sglang has no explicit setup and isn't active"
        );

        // Discoverability is preserved: typing a query still reveals it.
        let mut queried = ModelPickerView::new(&app, &config);
        type_model_query(&mut queried, "sglang");
        assert!(
            queried
                .visible_model_rows()
                .iter()
                .any(|row| row.provider == Some(crate::config::ApiProvider::Sglang)),
            "searching should still surface unconfigured providers"
        );
    }

    #[test]
    fn custom_model_row_position_accounts_for_other_configured_providers() {
        // #3830 regression: `resolved_model`/`model_row_count` treat any
        // selection at or past `visible_model_rows().len()` as "the custom
        // row." Once other configured providers' rows are mixed into the
        // default view, the initial selection must still land past *all* of
        // them, not just past the active provider's own rows.
        let (mut app, _default_config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro-2026-04-XX".to_string();
        app.auto_model = false;

        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                together: crate::config::ProviderConfig {
                    base_url: Some("https://custom.together.example/v1".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };

        let view = ModelPickerView::new(&app, &config);
        assert!(view.show_custom_model_row);
        assert!(
            view.visible_model_rows()
                .iter()
                .any(|row| row.provider == Some(crate::config::ApiProvider::Together)),
            "sanity check: Together rows are actually in the default view"
        );
        assert_eq!(view.selected_model_idx, view.visible_model_rows().len());
        assert_eq!(view.resolved_model(), "deepseek-v4-pro-2026-04-XX");
    }

    #[test]
    fn picker_initial_selection_matches_app_state() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-flash".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Max;
        let view = ModelPickerView::new(&app, &config);
        assert_eq!(view.resolved_model(), "deepseek-v4-flash");
        assert_eq!(view.resolved_effort(), ReasoningEffort::Max);
    }

    #[test]
    fn picker_initial_selection_matches_auto_state() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "auto".to_string();
        app.auto_model = true;
        app.reasoning_effort = ReasoningEffort::Auto;

        let view = ModelPickerView::new(&app, &config);

        assert_eq!(view.resolved_model(), "auto");
        assert_eq!(view.resolved_effort(), ReasoningEffort::Auto);
    }

    #[test]
    fn picker_auto_model_forces_auto_effort_on_apply() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "auto".to_string();
        app.auto_model = true;
        app.reasoning_effort = ReasoningEffort::Off;

        let view = ModelPickerView::new(&app, &config);

        assert_eq!(view.resolved_model(), "auto");
        assert_eq!(view.resolved_effort(), ReasoningEffort::Auto);
    }

    #[test]
    fn picker_normalizes_low_medium_to_high() {
        let (mut app, config, _lock) = create_test_app();
        app.reasoning_effort = ReasoningEffort::Medium;
        app.auto_model = false;
        let view = ModelPickerView::new(&app, &config);
        assert_eq!(
            view.resolved_effort(),
            ReasoningEffort::High,
            "medium should map to high in the picker"
        );
    }

    #[test]
    fn picker_exposes_auto_and_distinct_thinking_tiers() {
        let model_labels = picker_model_ids_for_provider(crate::config::ApiProvider::Deepseek);
        assert_eq!(
            model_labels,
            vec!["auto", "deepseek-v4-pro", "deepseek-v4-flash"]
        );

        let effort_labels: Vec<_> =
            picker_efforts_for_provider(crate::config::ApiProvider::Deepseek, false)
                .iter()
                .map(|effort| effort.as_setting())
                .collect();
        assert_eq!(effort_labels, vec!["auto", "off", "high", "max"]);
    }

    #[test]
    fn codex_picker_exposes_responses_reasoning_tiers() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::OpenaiCodex;
        app.model = "gpt-5.5-codex".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Off;

        let view = ModelPickerView::new(&app, &config);

        assert_eq!(view.resolved_effort(), ReasoningEffort::Low);
        let labels: Vec<_> =
            picker_efforts_for_provider(crate::config::ApiProvider::OpenaiCodex, false)
                .iter()
                .map(|effort| {
                    effort.display_label_for_provider(crate::config::ApiProvider::OpenaiCodex)
                })
                .collect();
        assert_eq!(labels, vec!["low", "medium", "high", "xhigh"]);
    }

    #[test]
    fn picker_excludes_saved_codex_model_from_deepseek_main_section() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Off;
        app.provider_models
            .insert("openai-codex".to_string(), "gpt-5.5".to_string());

        let view = ModelPickerView::new(&app, &config);
        assert_eq!(view.resolved_effort(), ReasoningEffort::Off);
        assert!(
            view.visible_model_rows()
                .iter()
                .all(|row| row.provider.is_none()
                    || row.provider == Some(crate::config::ApiProvider::Deepseek))
        );
        assert!(!view.visible_model_ids().contains(&"gpt-5.5"));
    }

    #[test]
    fn picker_does_not_switch_provider_when_moving_through_model_rows() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Max;
        app.provider_models
            .insert("openai-codex".to_string(), "gpt-5.5".to_string());

        let mut view = ModelPickerView::new(&app, &config);
        while view.move_down() {
            assert_ne!(
                view.resolved_provider(),
                Some(crate::config::ApiProvider::OpenaiCodex)
            );
        }

        assert_eq!(view.initial_provider, crate::config::ApiProvider::Deepseek);
    }

    #[test]
    fn picker_query_reveals_cross_provider_route_rows() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;

        let mut view = ModelPickerView::new(&app, &config);
        assert!(
            view.visible_model_rows()
                .iter()
                .all(|row| row.provider.is_none()
                    || row.provider == Some(crate::config::ApiProvider::Deepseek))
        );

        type_model_query(&mut view, "openrouter");

        assert!(
            view.visible_model_rows()
                .iter()
                .any(|row| row.provider == Some(crate::config::ApiProvider::Openrouter)),
            "query should reveal explicit OpenRouter route rows"
        );
        assert_eq!(
            view.resolved_provider(),
            Some(crate::config::ApiProvider::Openrouter)
        );
    }

    #[test]
    fn picker_query_cross_provider_enter_emits_provider_switch() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;

        let mut view = ModelPickerView::new(&app, &config);
        type_model_query(&mut view, "openrouter");

        let action = view.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModelPickerApplied {
                model, provider, ..
            }) => {
                assert_eq!(provider, Some(crate::config::ApiProvider::Openrouter));
                assert!(
                    !model.trim().is_empty() && model != "auto",
                    "cross-provider row must carry a concrete wire model"
                );
            }
            other => panic!("expected ModelPickerApplied EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn picker_query_no_match_custom_row_stays_active_provider_scoped() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Openrouter;
        app.model_ids_passthrough = true;
        app.model = crate::config::DEFAULT_OPENROUTER_MODEL.to_string();
        app.auto_model = false;

        let mut view = ModelPickerView::new(&app, &config);
        type_model_query(&mut view, "custom-org/custom-model");

        assert_eq!(view.resolved_model(), "custom-org/custom-model");
        assert_eq!(
            view.resolved_provider(),
            Some(crate::config::ApiProvider::Openrouter)
        );
        let action = view.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModelPickerApplied {
                model, provider, ..
            }) => {
                assert_eq!(model, "custom-org/custom-model");
                assert_eq!(provider, None, "active-provider custom row is not a switch");
            }
            other => panic!("expected ModelPickerApplied EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn picker_query_no_match_strict_provider_enter_is_noop() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model_ids_passthrough = false;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;

        let mut view = ModelPickerView::new(&app, &config);
        type_model_query(&mut view, "definitely-not-a-deepseek-model");

        assert_eq!(view.model_row_count(), 0);
        let action = view.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(action, ViewAction::None));
    }

    #[test]
    fn picker_query_backspace_restores_active_provider_rows() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;

        let mut view = ModelPickerView::new(&app, &config);
        type_model_query(&mut view, "openrouter");
        assert!(
            view.visible_model_rows()
                .iter()
                .any(|row| row.provider == Some(crate::config::ApiProvider::Openrouter))
        );

        for _ in 0.."openrouter".len() {
            view.handle_key(KeyEvent::new(
                KeyCode::Backspace,
                crossterm::event::KeyModifiers::NONE,
            ));
        }

        assert!(view.query.is_empty());
        assert!(
            view.visible_model_rows()
                .iter()
                .all(|row| row.provider.is_none()
                    || row.provider == Some(crate::config::ApiProvider::Deepseek))
        );
    }

    #[test]
    fn picker_effort_pane_ignores_query_typing() {
        let (app, config, _lock) = create_test_app();
        let mut view = ModelPickerView::new(&app, &config);
        view.handle_key(KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));

        type_model_query(&mut view, "openrouter");

        assert_eq!(view.focus, Pane::Effort);
        assert!(view.query.is_empty());
        assert!(
            view.visible_model_rows()
                .iter()
                .all(|row| row.provider.is_none()
                    || row.provider == Some(crate::config::ApiProvider::Deepseek))
        );
    }

    #[test]
    fn picker_query_resyncs_effort_for_codex_rows() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Auto;

        let mut view = ModelPickerView::new(&app, &config);
        assert_eq!(view.resolved_effort(), ReasoningEffort::Auto);

        type_model_query(&mut view, "codex");

        assert_eq!(
            view.resolved_provider(),
            Some(crate::config::ApiProvider::OpenaiCodex)
        );
        assert_eq!(
            view.resolved_effort(),
            ReasoningEffort::Medium,
            "OpenAI Codex rows should normalize auto to medium"
        );
    }

    #[test]
    fn picker_preserves_unknown_model_via_custom_row() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-pro-2026-04-XX".to_string();
        app.auto_model = false;
        let view = ModelPickerView::new(&app, &config);
        assert!(view.show_custom_model_row);
        assert_eq!(view.resolved_model(), "deepseek-v4-pro-2026-04-XX");
    }

    #[test]
    fn picker_lists_openrouter_large_models() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Openrouter;
        app.model_ids_passthrough = true;
        app.model = "minimax/minimax-m3".to_string();
        app.auto_model = false;

        let view = ModelPickerView::new(&app, &config);
        let model_ids = view.visible_model_ids();

        assert!(model_ids.contains(&"arcee-ai/trinity-large-thinking"));
        assert!(model_ids.contains(&"xiaomi/mimo-v2.5-pro"));
        assert!(model_ids.contains(&"minimax/minimax-m3"));
        assert!(model_ids.contains(&"z-ai/glm-5.2"));
        assert!(
            model_ids
                .iter()
                .take(6)
                .any(|id| *id == "minimax/minimax-m3"),
            "MiniMax M3 should be visible in the first picker window on normal terminals"
        );
        assert!(!view.show_custom_model_row);
        assert_eq!(view.resolved_model(), "minimax/minimax-m3");
    }

    #[test]
    fn picker_lists_xiaomi_mimo_chat_models_without_speech_models() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::XiaomiMimo;
        app.model = "mimo-v2.5-pro".to_string();
        app.auto_model = false;

        let view = ModelPickerView::new(&app, &config);
        let model_ids = view.visible_model_ids();

        for expected in ["mimo-v2.5-pro", "mimo-v2.5-pro-ultraspeed", "mimo-v2.5"] {
            assert!(model_ids.contains(&expected), "missing {expected}");
        }
        for deprecated in ["mimo-v2-pro", "mimo-v2-omni", "mimo-v2-flash"] {
            assert!(
                !model_ids.contains(&deprecated),
                "{deprecated} is deprecated and should not be promoted"
            );
        }
        for speech_model in [
            "mimo-v2.5-tts",
            "mimo-v2.5-tts-voicedesign",
            "mimo-v2.5-tts-voiceclone",
            "mimo-v2-tts",
        ] {
            assert!(
                !model_ids.contains(&speech_model),
                "{speech_model} should not appear in the chat model picker"
            );
        }
    }

    #[test]
    fn picker_for_ollama_preserves_current_local_tag_without_hosted_static_rows() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Ollama;
        app.model_ids_passthrough = true;
        app.model = "qwen2.5-coder:7b".to_string();
        app.auto_model = false;

        let view = ModelPickerView::new(&app, &config);
        let model_ids = view.visible_model_ids();

        assert_eq!(model_ids, vec!["auto"]);
        assert!(view.show_custom_model_row);
        assert_eq!(view.resolved_model(), "qwen2.5-coder:7b");
    }

    #[test]
    fn visible_row_window_tracks_selection_in_short_panes() {
        assert_eq!(visible_row_window(0, 16, 8), (0, 8));
        assert_eq!(visible_row_window(7, 16, 8), (3, 11));
        assert_eq!(visible_row_window(15, 16, 8), (8, 16));
        assert_eq!(visible_row_window(3, 4, 8), (0, 4));
        assert_eq!(visible_row_window(3, 4, 0), (0, 0));
    }

    #[test]
    fn narrow_picker_rows_hide_hint_before_clipping_model_id() {
        let spans = picker_row_spans(
            "minimax/minimax-m3",
            "1M multimodal",
            "▸",
            24,
            Style::default(),
            Style::default(),
        );
        let rendered = spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("minimax/minimax-m3"));
        assert!(!rendered.contains("1M multimodal"));
        assert!(unicode_width::UnicodeWidthStr::width(rendered.as_str()) <= 24);
    }

    #[test]
    fn picker_preserves_custom_passthrough_model_ids() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Openrouter;
        app.model_ids_passthrough = true;
        app.model = "opencode-go/glm-5.1".to_string();
        app.auto_model = false;

        let view = ModelPickerView::new(&app, &config);

        assert!(view.show_custom_model_row);
        assert_eq!(view.resolved_model(), "opencode-go/glm-5.1");
    }

    #[test]
    fn picker_exposes_active_custom_provider_model_row() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Custom;
        app.model_ids_passthrough = true;
        app.model = "vendor/custom-model-v1".to_string();
        app.auto_model = false;

        let view = ModelPickerView::new(&app, &config);

        assert!(view.show_custom_model_row);
        assert_eq!(view.resolved_model(), "vendor/custom-model-v1");
        assert_eq!(
            view.resolved_provider(),
            Some(crate::config::ApiProvider::Custom)
        );
    }

    #[test]
    fn picker_exposes_saved_model_for_active_provider() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::XiaomiMimo;
        app.model = "mimo-v2.5-custom".to_string();
        app.auto_model = false;
        app.provider_models
            .insert("xiaomi-mimo".to_string(), "mimo-v2.5-custom".to_string());

        let mut view = ModelPickerView::new(&app, &config);
        view.selected_model_idx = view
            .visible_model_rows()
            .iter()
            .position(|row| {
                row.id == "mimo-v2.5-custom"
                    && row.provider == Some(crate::config::ApiProvider::XiaomiMimo)
            })
            .expect("saved Xiaomi MiMo model row");

        let action = view.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModelPickerApplied {
                model, provider, ..
            }) => {
                assert_eq!(model, "mimo-v2.5-custom");
                assert_eq!(provider, None);
            }
            other => panic!("expected ModelPickerApplied EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn picker_excludes_saved_models_from_other_providers() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::XiaomiMimo;
        app.model = "mimo-v2.5-pro".to_string();
        app.auto_model = false;
        app.provider_models
            .insert("deepseek".to_string(), "deepseek-v4-pro".to_string());
        app.provider_models
            .insert("moonshot".to_string(), "kimi-k2.6".to_string());
        app.provider_models
            .insert("openai".to_string(), "qwen-plus".to_string());
        app.provider_models.insert(
            "qianfan".to_string(),
            "custom-qianfan-service-id".to_string(),
        );

        let view = ModelPickerView::new(&app, &config);
        let model_ids = view.visible_model_ids();

        // Active provider's own model stays present (and ahead of the tail).
        assert!(model_ids.contains(&"mimo-v2.5-pro"));
        // Cross-provider saved models are kept out of the provider-scoped list.
        assert!(!model_ids.contains(&"deepseek-v4-pro"));
        assert!(!model_ids.contains(&"kimi-k2.6"));
        assert!(!model_ids.contains(&"qwen-plus"));
        assert!(!model_ids.contains(&"custom-qianfan-service-id"));
        assert!(!view.show_custom_model_row);
        assert!(
            view.visible_model_rows()
                .iter()
                .all(|row| row.provider.is_none()
                    || row.provider == Some(crate::config::ApiProvider::XiaomiMimo))
        );
    }

    #[test]
    fn picker_skips_unknown_provider_saved_models() {
        // A config key that maps to no known provider cannot be applied, so it
        // must not produce a picker row (#2596).
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::XiaomiMimo;
        app.model = "mimo-v2.5-pro".to_string();
        app.auto_model = false;
        app.provider_models
            .insert("totally-unknown".to_string(), "ghost-model".to_string());

        let view = ModelPickerView::new(&app, &config);
        assert!(!view.visible_model_ids().contains(&"ghost-model"));
    }

    #[test]
    fn picker_does_not_hijack_current_custom_model_with_saved_provider_row() {
        let (mut app, config, _lock) = create_test_app();
        app.api_provider = crate::config::ApiProvider::Openai;
        app.model_ids_passthrough = true;
        app.model = "kimi-k2.6".to_string();
        app.provider_models
            .insert("moonshot".to_string(), "kimi-k2.6".to_string());

        let mut view = ModelPickerView::new(&app, &config);

        assert!(view.show_custom_model_row);
        assert_eq!(view.resolved_model(), "kimi-k2.6");
        let action = view.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModelPickerApplied {
                model, provider, ..
            }) => {
                assert_eq!(model, "kimi-k2.6");
                assert_eq!(provider, None);
            }
            other => panic!("expected ModelPickerApplied EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn arrow_keys_move_within_focused_pane() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-pro".to_string();
        app.reasoning_effort = ReasoningEffort::High;
        let mut view = ModelPickerView::new(&app, &config);
        assert_eq!(view.selected_model_idx, 1);
        view.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(view.selected_model_idx, 2);
        view.handle_key(KeyEvent::new(
            KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(view.selected_model_idx, 1);

        view.handle_key(KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(view.focus, Pane::Effort);
        assert_eq!(view.selected_effort_idx, 2);
        view.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(view.selected_effort_idx, 3);
    }

    #[test]
    fn mouse_wheel_moves_focused_picker_pane() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-pro".to_string();
        let mut view = ModelPickerView::new(&app, &config);
        assert_eq!(view.selected_model_idx, 1);

        view.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert_eq!(view.selected_model_idx, 2);

        view.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert_eq!(view.selected_model_idx, 1);
    }

    #[test]
    fn tab_switches_between_model_and_thinking() {
        let (app, config, _lock) = create_test_app();
        let mut view = ModelPickerView::new(&app, &config);
        assert_eq!(view.focus, Pane::Model);
        view.handle_key(KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert_eq!(view.focus, Pane::Effort);
        view.handle_key(KeyEvent::new(
            KeyCode::BackTab,
            crossterm::event::KeyModifiers::SHIFT,
        ));
        assert_eq!(view.focus, Pane::Model);
    }

    #[test]
    fn enter_emits_current_model_and_thinking() {
        let (mut app, config, _lock) = create_test_app();
        app.reasoning_effort = ReasoningEffort::High;
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        let mut view = ModelPickerView::new(&app, &config);
        assert_eq!(view.selected_model_idx, 1);
        assert_eq!(view.selected_effort_idx, 2);

        // Move model from Pro to Flash, then switch to effort and move High to Max.
        view.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));
        view.handle_key(KeyEvent::new(
            KeyCode::Tab,
            crossterm::event::KeyModifiers::NONE,
        ));
        view.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));

        let action = view.handle_key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        ));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModelPickerApplied {
                model,
                effort,
                previous_effort,
                ..
            }) => {
                assert_eq!(model, "deepseek-v4-flash");
                assert_eq!(effort, ReasoningEffort::Max);
                assert_eq!(previous_effort, ReasoningEffort::High);
            }
            other => panic!("expected ModelPickerApplied EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn deepseek_provider_uses_neutral_two_pane_selection() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-flash".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Max;
        let view = ModelPickerView::new(&app, &config);
        assert_eq!(view.selected_model_idx, 2);
        assert_eq!(view.selected_effort_idx, 3);
        assert_eq!(view.focus, Pane::Model);
        assert_eq!(view.resolved_model(), "deepseek-v4-flash");
        assert_eq!(view.resolved_effort(), ReasoningEffort::Max);
    }

    #[test]
    fn model_picker_selected_row_renders_readable_selection_contrast() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-flash".to_string();
        app.auto_model = false;
        let view = ModelPickerView::new(&app, &config);
        let area = Rect::new(0, 0, 100, 28);
        let mut buf = Buffer::empty(area);

        view.render(area, &mut buf);

        let y = row_containing(&buf, area, "deepseek-v4-flash")
            .expect("selected model row should render");
        let highlighted_cells = (area.x..area.x.saturating_add(area.width))
            .filter(|&x| {
                let cell = &buf[(x, y)];
                !cell.symbol().trim().is_empty()
                    && cell.bg == palette::SELECTION_BG
                    && cell.fg == palette::SELECTION_TEXT
            })
            .count();

        assert!(
            highlighted_cells >= "deepseek-v4-flash".len(),
            "selected /model row should use readable selection text"
        );
        assert!(
            !(area.x..area.x.saturating_add(area.width))
                .any(|x| buf[(x, y)].bg == palette::WHALE_ACCENT_PRIMARY),
            "selected /model row should not use the bright accent background"
        );
    }

    #[test]
    fn known_model_with_auto_effort_preserves_explicit_model() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-pro".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Auto;
        let view = ModelPickerView::new(&app, &config);
        assert!(!view.show_custom_model_row);
        assert_eq!(view.selected_model_idx, 1);
        assert_eq!(view.selected_effort_idx, 0);
        assert_eq!(view.resolved_model(), "deepseek-v4-pro");
        assert_eq!(view.resolved_effort(), ReasoningEffort::Auto);
    }

    #[test]
    fn auto_model_selects_auto_row() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "auto".to_string();
        app.auto_model = true;
        app.reasoning_effort = ReasoningEffort::Auto;
        let view = ModelPickerView::new(&app, &config);
        assert_eq!(view.selected_model_idx, 0);
        assert_eq!(view.selected_effort_idx, 0);
        assert_eq!(view.resolved_model(), "auto");
        assert_eq!(view.resolved_effort(), ReasoningEffort::Auto);
    }

    #[test]
    fn custom_model_row_preserves_current_model_and_effort() {
        let (mut app, config, _lock) = create_test_app();
        app.model = "deepseek-v4-pro-2026-04-XX".to_string();
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::High;
        let view = ModelPickerView::new(&app, &config);
        assert!(view.show_custom_model_row);
        assert_eq!(view.selected_model_idx, view.visible_model_rows().len());
        assert_eq!(view.selected_effort_idx, 2);
        assert_eq!(view.resolved_model(), "deepseek-v4-pro-2026-04-XX");
        assert_eq!(view.resolved_effort(), ReasoningEffort::High);
    }

    #[test]
    fn move_down_from_last_model_is_noop() {
        let (app, config, _lock) = create_test_app();
        let mut view = ModelPickerView::new(&app, &config);
        view.selected_model_idx = view.model_row_count() - 1;
        let result = view.move_down();
        assert!(!result);
    }

    #[test]
    fn move_up_from_first_model_is_noop() {
        let (app, config, _lock) = create_test_app();
        let mut view = ModelPickerView::new(&app, &config);
        view.selected_model_idx = 0;
        let result = view.move_up();
        assert!(!result);
    }

    #[test]
    fn immediate_esc_closes_without_apply() {
        let (app, config, _lock) = create_test_app();
        let mut view = ModelPickerView::new(&app, &config);
        let action = view.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn esc_after_selection_move_closes_without_apply() {
        let (mut app, config, _lock) = create_test_app();
        app.reasoning_effort = ReasoningEffort::High;
        let mut view = ModelPickerView::new(&app, &config);
        view.handle_key(KeyEvent::new(
            KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ));

        let action = view.handle_key(KeyEvent::new(
            KeyCode::Esc,
            crossterm::event::KeyModifiers::NONE,
        ));

        assert!(matches!(action, ViewAction::Close));
    }

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires every
    /// overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn model_picker_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        let (app, config, _lock) = create_test_app();
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            // Pre-fill with a sentinel so any cell the composited modal fails to
            // paint (bleed-through) is detectable as a surviving 'X'. The default
            // test app uses DeepSeek model ids, so 'X' never appears legitimately.
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            // Render through the ViewStack so the shared opaque backdrop is
            // painted exactly as it is in production.
            let mut stack = ViewStack::new();
            stack.push(ModelPickerView::new(&app, &config));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            // Footer keeps every action (it wraps instead of clipping).
            for label in ["move", "switch", "filter", "apply", "cancel"] {
                assert!(text.contains(label), "{w}x{h}: missing '{label}' hint");
            }
            // Composited frame is fully opaque: no sentinel survives and the
            // center cell carries the modal ink background.
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::DEEPSEEK_INK,
                "{w}x{h}: modal interior must be opaque"
            );
            // No row exceeds the frame width (no horizontal overflow).
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    unicode_width::UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }

    #[test]
    fn deepseek_picker_exposes_auto_off_high_max() {
        let labels: Vec<&str> =
            picker_efforts_for_provider(crate::config::ApiProvider::Deepseek, false)
                .iter()
                .map(|effort| effort.short_label())
                .collect();
        assert_eq!(labels, vec!["auto", "off", "high", "max"]);
    }
}
