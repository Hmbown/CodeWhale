//! `/provider` picker modal — pick a provider (DeepSeek / NVIDIA NIM /
//! hosted providers / self-hosted providers) and, if it lacks credentials, type the API key
//! inline before completing the switch (#52).
//!
//! The picker is intentionally a single modal with two visible states:
//!
//! 1. **List** — pick a provider; each row shows the active provider arrow
//!    and an "API key configured" / "needs API key" hint. Enter on a
//!    configured provider applies the switch immediately
//!    ([`ViewEvent::ProviderPickerApplied`]). Enter on an un-configured one
//!    transitions the same modal into the key-entry state.
//! 2. **Key entry** — masked input box pre-filled with the provider's
//!    canonical env-var name as a hint. Enter submits
//!    [`ViewEvent::ProviderPickerApiKeySubmitted`], which the UI handler
//!    persists via `save_api_key_for` before switching.
//!
//! Pressing Esc backs out: from key entry returns to the list; from the
//! list closes the modal without changes.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

use crate::config::{ApiProvider, Config, has_api_key_for, kimi_cli_credentials_present};
use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use codewhale_config::provider::WireFormat;
use codewhale_config::route::{
    LogicalModelRef, PricingSku, RequestProtocol, RouteRequest, RouteResolver, bundled_offerings,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    List,
    KeyEntry,
}

pub struct ProviderPickerView {
    rows: Vec<ProviderDashboardRow>,
    selected_idx: usize,
    stage: Stage,
    api_key_input: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDashboardRow {
    pub provider: ApiProvider,
    pub provider_id: String,
    pub display_name: String,
    pub kind: String,
    pub base_url: String,
    pub auth_status: ProviderAuthStatus,
    pub catalog_status: ProviderCatalogStatus,
    pub supported_protocols: Vec<String>,
    pub available_model_count: usize,
    pub default_route: ProviderDefaultRoute,
    pub usage_meter: String,
    pub readiness: ProviderReadiness,
    pub messages: Vec<String>,
    pub is_active: bool,
    has_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderAuthStatus {
    Configured,
    Missing,
    Optional,
    OAuthReady,
    OAuthMissing,
    Local,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCatalogStatus {
    Bundled,
    DefaultOnly,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDefaultRoute {
    pub logical_model: String,
    pub wire_model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReadiness {
    Ready,
    NeedsAuth,
    LocalReady,
    Legacy,
    Invalid,
}

impl ProviderDashboardRow {
    fn from_config(provider: ApiProvider, active: ApiProvider, config: &Config) -> Self {
        let has_key = has_api_key_for(config, provider);
        let configured = config.provider_config_for(provider);
        let configured_base_url = configured
            .and_then(|entry| entry.base_url.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let configured_model = configured
            .and_then(|entry| entry.model.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let auth_status = auth_status_for(provider, has_key, configured);
        let usage_meter = usage_meter_for(provider);

        let Some(kind) = provider.kind() else {
            return Self {
                provider,
                provider_id: provider.as_str().to_string(),
                display_name: provider.display_name().to_string(),
                kind: "legacy".to_string(),
                base_url: configured_base_url
                    .unwrap_or_else(|| provider.default_base_url().to_string()),
                auth_status: ProviderAuthStatus::Legacy,
                catalog_status: ProviderCatalogStatus::Legacy,
                supported_protocols: vec![protocol_label(WireFormat::ChatCompletions).to_string()],
                available_model_count: 0,
                default_route: ProviderDefaultRoute {
                    logical_model: configured_model
                        .unwrap_or_else(|| "deepseek-v4-pro".to_string()),
                    wire_model: "legacy alias".to_string(),
                },
                usage_meter,
                readiness: ProviderReadiness::Legacy,
                messages: vec![
                    "legacy DeepSeek China alias; routing maps through DeepSeek compatibility"
                        .to_string(),
                ],
                is_active: provider == active,
                has_key,
            };
        };

        let available_model_count = bundled_offerings()
            .iter()
            .filter(|offering| offering.provider.as_str() == kind.as_str())
            .count();
        let catalog_status = if available_model_count == 0 {
            ProviderCatalogStatus::DefaultOnly
        } else {
            ProviderCatalogStatus::Bundled
        };
        let route_request = RouteRequest {
            explicit_provider: Some(kind),
            model_selector: configured_model.clone().map(LogicalModelRef::from),
            saved_provider_model: None,
            base_url_override: configured_base_url.clone(),
        };

        let mut messages = Vec::new();
        let route = RouteResolver::new().resolve(&route_request);
        let (base_url, supported_protocols, default_route, resolved_pricing, route_ok) = match route
        {
            Ok(candidate) => {
                if !candidate.validation.messages.is_empty() {
                    messages.extend(candidate.validation.messages.clone());
                }
                (
                    candidate.endpoint.base_url,
                    vec![protocol_label(candidate.protocol).to_string()],
                    ProviderDefaultRoute {
                        logical_model: candidate.logical_model.raw().to_string(),
                        wire_model: candidate.wire_model_id.as_str().to_string(),
                    },
                    pricing_label(provider, candidate.pricing.as_ref()),
                    candidate.validation.ok,
                )
            }
            Err(error) => {
                messages.push(format!("route validation failed: {error}"));
                (
                    configured_base_url.unwrap_or_else(|| provider.default_base_url().to_string()),
                    vec![
                        provider
                            .metadata()
                            .map(|metadata| protocol_label(metadata.wire()).to_string())
                            .unwrap_or_else(|| {
                                protocol_label(WireFormat::ChatCompletions).to_string()
                            }),
                    ],
                    ProviderDefaultRoute {
                        logical_model: configured_model.unwrap_or_else(|| "invalid".to_string()),
                        wire_model: "unresolved".to_string(),
                    },
                    usage_meter.clone(),
                    false,
                )
            }
        };

        if matches!(
            auth_status,
            ProviderAuthStatus::Missing | ProviderAuthStatus::OAuthMissing
        ) {
            messages.push(format!("missing {}", provider.env_vars_label()));
        }
        if catalog_status == ProviderCatalogStatus::DefaultOnly {
            messages.push("catalog snapshot missing; using provider default".to_string());
        }

        let readiness = readiness_for(provider, auth_status, route_ok);

        Self {
            provider,
            provider_id: kind.as_str().to_string(),
            display_name: provider.display_name().to_string(),
            kind: format!("{kind:?}"),
            base_url,
            auth_status,
            catalog_status,
            supported_protocols,
            available_model_count,
            default_route,
            usage_meter: resolved_pricing,
            readiness,
            messages,
            is_active: provider == active,
            has_key,
        }
    }

    fn compact_hint(&self) -> String {
        format!(
            "{} | auth:{} | {} | {} | base:{} | route:{}{} | catalog:{}",
            self.readiness.label(),
            self.auth_status.label(),
            self.usage_meter,
            self.supported_protocols.join("+"),
            compact_base_url(&self.base_url),
            self.default_route.logical_model,
            route_wire_suffix(&self.default_route),
            self.catalog_label()
        )
    }

    fn catalog_label(&self) -> String {
        match self.catalog_status {
            ProviderCatalogStatus::Bundled => format!("{} bundled", self.available_model_count),
            ProviderCatalogStatus::DefaultOnly => "default-only".to_string(),
            ProviderCatalogStatus::Legacy => "legacy".to_string(),
        }
    }
}

impl ProviderAuthStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Missing => "missing",
            Self::Optional => "optional",
            Self::OAuthReady => "oauth-ready",
            Self::OAuthMissing => "oauth-missing",
            Self::Local => "local",
            Self::Legacy => "legacy",
        }
    }
}

impl ProviderReadiness {
    fn label(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsAuth => "needs-auth",
            Self::LocalReady => "local-ready",
            Self::Legacy => "legacy",
            Self::Invalid => "invalid",
        }
    }
}

fn auth_status_for(
    provider: ApiProvider,
    has_key: bool,
    configured: Option<&crate::config::ProviderConfig>,
) -> ProviderAuthStatus {
    if matches!(provider, ApiProvider::Ollama) {
        return ProviderAuthStatus::Local;
    }
    if matches!(provider, ApiProvider::Sglang | ApiProvider::Vllm) {
        return if has_explicit_credential(provider, configured) {
            ProviderAuthStatus::Configured
        } else {
            ProviderAuthStatus::Optional
        };
    }
    if provider == ApiProvider::Moonshot && configured.is_some_and(config_uses_kimi_oauth) {
        return if has_key {
            ProviderAuthStatus::OAuthReady
        } else {
            ProviderAuthStatus::OAuthMissing
        };
    }
    if provider == ApiProvider::OpenaiCodex {
        return if has_key {
            ProviderAuthStatus::OAuthReady
        } else {
            ProviderAuthStatus::OAuthMissing
        };
    }
    if has_key {
        ProviderAuthStatus::Configured
    } else {
        ProviderAuthStatus::Missing
    }
}

fn has_explicit_credential(
    provider: ApiProvider,
    configured: Option<&crate::config::ProviderConfig>,
) -> bool {
    provider
        .env_vars()
        .iter()
        .any(|var| std::env::var(var).is_ok_and(|value| !value.trim().is_empty()))
        || configured.is_some_and(|entry| {
            entry
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || entry
                    .auth
                    .as_ref()
                    .is_some_and(|auth| auth.validate().is_ok())
        })
}

fn config_uses_kimi_oauth(config: &crate::config::ProviderConfig) -> bool {
    config.auth_mode.as_deref().is_some_and(|mode| {
        let normalized = mode.trim().to_ascii_lowercase().replace(['-', ' '], "_");
        matches!(normalized.as_str(), "kimi_oauth" | "kimi_cli" | "kimi_code")
    })
}

fn readiness_for(
    provider: ApiProvider,
    auth_status: ProviderAuthStatus,
    route_ok: bool,
) -> ProviderReadiness {
    if provider.kind().is_none() {
        return ProviderReadiness::Legacy;
    }
    if !route_ok {
        return ProviderReadiness::Invalid;
    }
    match auth_status {
        ProviderAuthStatus::Local | ProviderAuthStatus::Optional => ProviderReadiness::LocalReady,
        ProviderAuthStatus::Configured | ProviderAuthStatus::OAuthReady => ProviderReadiness::Ready,
        ProviderAuthStatus::Legacy => ProviderReadiness::Legacy,
        ProviderAuthStatus::Missing | ProviderAuthStatus::OAuthMissing => {
            ProviderReadiness::NeedsAuth
        }
    }
}

fn usage_meter_for(provider: ApiProvider) -> String {
    match provider {
        ApiProvider::Ollama | ApiProvider::Sglang | ApiProvider::Vllm => "cost: local".to_string(),
        ApiProvider::OpenaiCodex => "usage: Codex OAuth quota".to_string(),
        ApiProvider::Moonshot if kimi_cli_credentials_present() => {
            "usage: Kimi OAuth quota".to_string()
        }
        ApiProvider::XiaomiMimo => "cost: token-plan".to_string(),
        _ => "cost: unknown".to_string(),
    }
}

fn pricing_label(provider: ApiProvider, pricing: Option<&PricingSku>) -> String {
    match pricing {
        Some(PricingSku::Token {
            input_per_mtok,
            output_per_mtok,
        }) => match (input_per_mtok, output_per_mtok) {
            (Some(input), Some(output)) => format!("cost: ${input:.2}/${output:.2} mtok"),
            _ => "cost: token".to_string(),
        },
        Some(PricingSku::SubscriptionQuota { used_pct, .. }) => used_pct.map_or_else(
            || "usage: subscription quota".to_string(),
            |pct| format!("usage: subscription {pct:.0}%"),
        ),
        Some(PricingSku::AccountCredits { balance }) => balance.map_or_else(
            || "usage: account credits".to_string(),
            |balance| format!("usage: ${balance:.2} credits"),
        ),
        Some(PricingSku::LocalOrNotApplicable) => "cost: local".to_string(),
        Some(PricingSku::UnknownOrStale) | None => usage_meter_for(provider),
    }
}

fn protocol_label(protocol: RequestProtocol) -> &'static str {
    match protocol {
        WireFormat::ChatCompletions => "chat",
        WireFormat::Responses => "responses",
        WireFormat::AnthropicMessages => "anthropic",
    }
}

fn route_wire_suffix(route: &ProviderDefaultRoute) -> String {
    if route.logical_model == route.wire_model {
        String::new()
    } else {
        format!(" -> {}", route.wire_model)
    }
}

fn compact_base_url(base_url: &str) -> String {
    base_url
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

impl ProviderPickerView {
    #[must_use]
    pub fn new(active: ApiProvider, config: &Config) -> Self {
        // Present providers in the shared metadata display order (#3076). The
        // active provider is highlighted via `selected_idx` below, so it is
        // never lost in the list.
        let rows: Vec<ProviderDashboardRow> = ApiProvider::sorted_for_display()
            .into_iter()
            .map(|p| ProviderDashboardRow::from_config(p, active, config))
            .collect();
        let selected_idx = rows
            .iter()
            .position(|row| row.provider == active)
            .unwrap_or(0);
        Self {
            rows,
            selected_idx,
            stage: Stage::List,
            api_key_input: String::new(),
        }
    }

    fn move_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.selected_idx == 0 {
            self.selected_idx = self.rows.len() - 1;
        } else {
            self.selected_idx -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.selected_idx + 1 == self.rows.len() {
            self.selected_idx = 0;
        } else {
            self.selected_idx += 1;
        }
    }

    /// Type-ahead: move the selection to the next provider whose display name
    /// starts with the given character (case-insensitive), wrapping so repeated
    /// presses cycle through matches — e.g. pressing `z` jumps to "Z.ai".
    fn jump_to_letter(&mut self, c: char) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let target = c.to_ascii_lowercase();
        for offset in 1..=count {
            let idx = (self.selected_idx + offset) % count;
            if self.rows[idx]
                .display_name
                .to_ascii_lowercase()
                .starts_with(target)
            {
                self.selected_idx = idx;
                return;
            }
        }
    }

    fn selected_provider(&self) -> ApiProvider {
        self.rows[self.selected_idx].provider
    }

    fn selected_has_key(&self) -> bool {
        self.rows[self.selected_idx].has_key
    }

    fn enter_key_entry(&mut self) {
        self.stage = Stage::KeyEntry;
        self.api_key_input.clear();
    }

    fn env_var_for(provider: ApiProvider) -> String {
        provider.env_vars_label()
    }

    fn visible_start(&self, visible_rows: usize) -> usize {
        if visible_rows == 0 {
            return 0;
        }
        let max_start = self.rows.len().saturating_sub(visible_rows);
        self.selected_idx
            .saturating_add(1)
            .saturating_sub(visible_rows)
            .min(max_start)
    }

    fn selected_row_style(fg: Color) -> Style {
        Style::default()
            .fg(fg)
            .bg(palette::SURFACE_ELEVATED)
            .add_modifier(Modifier::BOLD)
    }

    fn selected_row_bg_style() -> Style {
        Style::default().bg(palette::SURFACE_ELEVATED)
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        let enter_action = if self.selected_has_key() {
            "apply"
        } else {
            "set key"
        };
        let outer = Block::default()
            .title(Line::from(Span::styled(
                " Provider ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" ↑↓ ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("move "),
                Span::styled(" a-z ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("jump "),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(format!("{enter_action} ")),
                Span::styled(" R ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("edit key "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("cancel "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = outer.inner(area);
        outer.render(area, buf);

        let visible_rows = usize::from(inner.height);
        let visible_start = self.visible_start(visible_rows);
        let mut lines: Vec<Line> = Vec::with_capacity(visible_rows);
        for (idx, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(visible_start)
            .take(visible_rows)
        {
            let is_selected = idx == self.selected_idx;
            let is_active = row.is_active;
            let arrow = if is_selected { "▸" } else { " " };
            let active_dot = if is_active { " *" } else { "  " };
            let spacer_style = if is_selected {
                Self::selected_row_bg_style()
            } else {
                Style::default()
            };
            let label_style = if is_selected {
                Self::selected_row_style(palette::TEXT_PRIMARY)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_selected {
                let hint_fg = if row.has_key {
                    palette::TEXT_MUTED
                } else {
                    palette::STATUS_WARNING
                };
                Self::selected_row_style(hint_fg)
            } else if row.has_key {
                Style::default().fg(palette::TEXT_MUTED)
            } else {
                Style::default().fg(palette::STATUS_WARNING)
            };
            let hint = row.compact_hint();
            let mut line = Line::from(vec![
                Span::styled(" ", spacer_style),
                Span::styled(arrow, label_style),
                Span::styled(" ", spacer_style),
                Span::styled(row.display_name.as_str(), label_style),
                Span::styled(active_dot, label_style),
                Span::styled("  ", spacer_style),
                Span::styled(hint, hint_style),
            ]);
            if is_selected {
                line.style = Self::selected_row_bg_style();
                let target_width = usize::from(inner.width);
                let line_width = line.width();
                if line_width < target_width {
                    line.spans.push(Span::styled(
                        " ".repeat(target_width - line_width),
                        Self::selected_row_bg_style(),
                    ));
                }
            }
            lines.push(line);
        }
        Paragraph::new(lines).render(inner, buf);
    }

    fn render_key_entry(&self, area: Rect, buf: &mut Buffer) {
        let provider = self.selected_provider();
        let outer = Block::default()
            .title(Line::from(Span::styled(
                format!(" API key — {} ", provider.display_name()),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("save & switch "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("back "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default());
        let inner = outer.inner(area);
        outer.render(area, buf);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

        let masked = mask_key(&self.api_key_input);
        let display = if masked.is_empty() {
            "(paste key here)".to_string()
        } else {
            masked
        };
        let key_lines = vec![Line::from(vec![
            Span::styled("Key: ", Style::default().fg(palette::TEXT_MUTED)),
            Span::styled(
                display,
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ])];
        Paragraph::new(key_lines).render(layout[0], buf);

        let hint = format!(
            "Or set the {} environment variable and re-open /provider.",
            Self::env_var_for(provider),
        );
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(palette::TEXT_MUTED),
        )))
        .render(layout[1], buf);
    }
}

fn mask_key(input: &str) -> String {
    let trimmed = input.trim();
    let len = trimmed.chars().count();
    if len == 0 {
        return String::new();
    }
    if len <= 4 {
        return "*".repeat(len);
    }
    let visible: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{}{}", "*".repeat(len - 4), visible)
}

impl ModalView for ProviderPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ProviderPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if self.stage == Stage::KeyEntry {
            let sanitized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            if !sanitized.is_empty() {
                self.api_key_input.push_str(&sanitized);
            }
            true
        } else {
            false
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.stage {
            Stage::List => match key.code {
                KeyCode::Esc => ViewAction::Close,
                KeyCode::Up => {
                    self.move_up();
                    ViewAction::None
                }
                KeyCode::Down => {
                    self.move_down();
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let provider = self.selected_provider();
                    if self.selected_has_key() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider })
                    } else if provider == ApiProvider::Moonshot && kimi_cli_credentials_present() {
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerKimiOAuthEnabled {
                            provider,
                        })
                    } else {
                        self.enter_key_entry();
                        ViewAction::None
                    }
                }
                KeyCode::Char(c) if key.modifiers.is_empty() && c.eq_ignore_ascii_case(&'r') => {
                    self.enter_key_entry();
                    ViewAction::None
                }
                // Type-ahead: any other letter jumps to the next provider whose
                // name starts with it (e.g. `z` -> "Z.ai").
                KeyCode::Char(c) if key.modifiers.is_empty() && c.is_ascii_alphabetic() => {
                    self.jump_to_letter(c);
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
            Stage::KeyEntry => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::List;
                    self.api_key_input.clear();
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    self.api_key_input.pop();
                    ViewAction::None
                }
                KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.api_key_input.pop();
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let key = self.api_key_input.trim().to_string();
                    if key.is_empty() {
                        // Stay in key-entry; the user can press Esc to abort.
                        ViewAction::None
                    } else {
                        let provider = self.selected_provider();
                        ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                            provider,
                            api_key: key,
                        })
                    }
                }
                KeyCode::Char(c) => {
                    // Reject ASCII whitespace so a stray space/tab doesn't slip
                    // into a credential; bracketed paste happens via the input
                    // path that already trims on submit.
                    if !c.is_whitespace() {
                        self.api_key_input.push(c);
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            },
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        if self.stage == Stage::List {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.move_up(),
                MouseEventKind::ScrollDown => self.move_down(),
                _ => {}
            }
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 120.min(area.width.saturating_sub(4)).max(64);
        let popup_height = match self.stage {
            Stage::List => (self.rows.len() as u16).saturating_add(2),
            Stage::KeyEntry => 10,
        }
        .min(area.height.saturating_sub(4))
        .max(8);
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        match self.stage {
            Stage::List => self.render_list(popup_area, buf),
            Stage::KeyEntry => self.render_key_entry(popup_area, buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use std::env;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn remove(key: &'static str) -> Self {
            let previous = env::var_os(key);
            // SAFETY: provider-picker tests that mutate environment variables
            // hold ENV_LOCK for the whole guard lifetime, so no sibling test in
            // this module can concurrently mutate/read this provider key.
            unsafe {
                env::remove_var(key);
            }
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: EnvVarGuard is used while ENV_LOCK is held; declaration
            // order in the test drops the guard before releasing the lock.
            unsafe {
                match self.previous.take() {
                    Some(value) => env::set_var(self.key, value),
                    None => env::remove_var(self.key),
                }
            }
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn move_to_provider(picker: &mut ProviderPickerView, provider: ApiProvider) {
        let max_steps = picker.rows.len();
        for _ in 0..max_steps {
            if picker.selected_provider() == provider {
                return;
            }
            picker.handle_key(key(KeyCode::Down));
        }
        panic!("provider {provider:?} not found in picker");
    }

    fn render_text(picker: &ProviderPickerView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        picker.render(area, &mut buf);
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn type_ahead_jumps_to_provider_by_first_letter() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // `z` is unique to Z.ai among provider display names.
        picker.handle_key(key(KeyCode::Char('z')));
        assert_eq!(picker.selected_provider(), ApiProvider::Zai);
    }

    #[test]
    fn mouse_scroll_moves_selection_in_list_stage() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let before = picker.selected_idx;
        picker.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_ne!(
            picker.selected_idx, before,
            "scroll down should advance the selection"
        );
    }

    #[test]
    fn picker_lists_all_providers() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let names: Vec<_> = picker
            .rows
            .iter()
            .map(|row| row.display_name.as_str())
            .collect();

        // Every built-in provider is present, none dropped (#3076 reorders, it
        // does not filter).
        assert_eq!(names.len(), ApiProvider::all().len());
        assert!(names.contains(&"DeepSeek"));

        // Providers are presented in neutral case-insensitive alphabetical
        // order by display name (#3076), not `ApiProvider::all()` order.
        let mut expected = names.clone();
        expected.sort_by_key(|name| name.to_ascii_lowercase());
        assert_eq!(
            names, expected,
            "provider picker must list providers in case-insensitive alphabetical order"
        );
        // DeepSeek is no longer hard-coded first.
        assert_ne!(names.first(), Some(&"DeepSeek"));
    }

    #[test]
    fn key_entry_hint_uses_metadata_env_vars() {
        assert_eq!(
            ProviderPickerView::env_var_for(ApiProvider::NvidiaNim),
            "NVIDIA_API_KEY / NVIDIA_NIM_API_KEY / DEEPSEEK_API_KEY"
        );
    }

    #[test]
    fn provider_dashboard_row_models_local_readiness_without_rendering() {
        let config = Config::default();
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Ollama, ApiProvider::Ollama, &config);

        assert_eq!(row.provider_id, "ollama");
        assert_eq!(row.auth_status, ProviderAuthStatus::Local);
        assert_eq!(row.readiness, ProviderReadiness::LocalReady);
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
        assert_eq!(row.usage_meter, "cost: local");
        assert!(row.base_url.contains("localhost:11434"));
        assert!(row.is_active);
    }

    #[test]
    fn provider_dashboard_row_uses_route_resolver_for_custom_openai_endpoint() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    api_key: Some("openai-key".to_string()),
                    base_url: Some("http://localhost:9000/v1".to_string()),
                    model: Some("custom-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row =
            ProviderDashboardRow::from_config(ApiProvider::Openai, ApiProvider::Openai, &config);

        assert_eq!(row.provider_id, "openai");
        assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
        assert_eq!(row.readiness, ProviderReadiness::Ready);
        assert_eq!(row.base_url, "http://localhost:9000/v1");
        assert_eq!(row.default_route.logical_model, "custom-model");
        assert_eq!(row.default_route.wire_model, "custom-model");
        assert_eq!(row.supported_protocols, vec!["chat".to_string()]);
    }

    #[test]
    fn provider_dashboard_row_surfaces_anthropic_wire_protocol() {
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Anthropic,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.provider_id, "anthropic");
        assert_eq!(row.supported_protocols, vec!["anthropic".to_string()]);
        assert_eq!(row.catalog_status, ProviderCatalogStatus::DefaultOnly);
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("catalog"))
        );
    }

    #[test]
    fn provider_dashboard_row_marks_missing_hosted_auth_as_needs_auth() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _openrouter_key = EnvVarGuard::remove("OPENROUTER_API_KEY");
        let config = Config::default();
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Openrouter,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.auth_status, ProviderAuthStatus::Missing);
        assert_eq!(row.readiness, ProviderReadiness::NeedsAuth);
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("missing OPENROUTER_API_KEY"))
        );
    }

    #[test]
    fn provider_dashboard_row_marks_route_resolver_errors_as_invalid() {
        let config = Config {
            api_key: Some("deepseek-key".to_string()),
            providers: Some(crate::config::ProvidersConfig {
                deepseek: crate::config::ProviderConfig {
                    model: Some("anthropic/claude-foreign".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let row = ProviderDashboardRow::from_config(
            ApiProvider::Deepseek,
            ApiProvider::Deepseek,
            &config,
        );

        assert_eq!(row.auth_status, ProviderAuthStatus::Configured);
        assert_eq!(row.readiness, ProviderReadiness::Invalid);
        assert_eq!(row.default_route.wire_model, "unresolved");
        assert!(
            row.messages
                .iter()
                .any(|message| message.contains("route validation failed"))
        );
    }

    #[test]
    fn provider_dashboard_render_includes_route_protocol_usage_and_base_url() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                openai: crate::config::ProviderConfig {
                    api_key: Some("openai-key".to_string()),
                    base_url: Some("http://localhost:9000/v1".to_string()),
                    model: Some("custom-model".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Openai, &config);

        let rendered = render_text(&picker, 124, 18);

        assert!(rendered.contains("auth:configured"));
        assert!(rendered.contains("route:custom-model"));
        assert!(rendered.contains("chat"));
        assert!(rendered.contains("cost: unknown"));
        assert!(rendered.contains("localhost:9000/v1"));
    }

    #[test]
    fn ollama_is_selectable_without_key() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);
        assert_eq!(picker.selected_provider(), ApiProvider::Ollama);
        assert!(picker.selected_has_key());
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider }) => {
                assert_eq!(provider, ApiProvider::Ollama);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn picker_marks_active_provider_as_initial_selection() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Openrouter, &config);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        assert!(picker.rows[picker.selected_idx].is_active);
    }

    #[test]
    fn list_navigation_wraps_between_first_and_last_provider() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let first = picker.rows.first().expect("non-empty list").provider;
        let last = picker.rows.last().expect("non-empty list").provider;

        // Order-independent: jump to the first entry, wrap up to the last, back down.
        picker.selected_idx = 0;
        picker.handle_key(key(KeyCode::Up));
        assert_eq!(picker.selected_provider(), last);

        picker.handle_key(key(KeyCode::Down));
        assert_eq!(picker.selected_provider(), first);
    }

    #[test]
    fn enter_with_no_key_transitions_to_key_entry_stage() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Move to OpenRouter, which has no key in default config.
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        assert_eq!(picker.selected_provider(), ApiProvider::Openrouter);
        let action = picker.handle_key(key(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::KeyEntry);
    }

    #[test]
    fn enter_with_existing_key_emits_apply_and_closes() {
        let config = Config {
            api_key: Some("existing-deepseek-key".to_string()),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::NvidiaNim, &config);
        // Navigate to DeepSeek, which has a key from the top-level config.
        move_to_provider(&mut picker, ApiProvider::Deepseek);
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApplied { provider }) => {
                assert_eq!(provider, ApiProvider::Deepseek);
            }
            other => panic!("expected ProviderPickerApplied, got {other:?}"),
        }
    }

    #[test]
    fn configured_provider_can_reenter_key_entry_with_r() {
        let config = Config {
            providers: Some(crate::config::ProvidersConfig {
                xiaomi_mimo: crate::config::ProviderConfig {
                    api_key: Some("mimo-key".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Config::default()
        };
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::XiaomiMimo);

        let action = picker.handle_key(key(KeyCode::Char('r')));

        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::KeyEntry);
        assert!(picker.api_key_input.is_empty());
    }

    #[test]
    fn ctrl_r_does_not_trigger_key_entry() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        let action = picker.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));

        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::List);
    }

    #[test]
    fn configured_provider_footer_mentions_edit_key() {
        let config = Config {
            api_key: Some("existing-deepseek-key".to_string()),
            ..Config::default()
        };
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        let rendered = render_text(&picker, 80, 12);

        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("apply"));
        assert!(rendered.contains("edit key"));
    }

    #[test]
    fn key_entry_enter_submits_after_typing() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        // Navigate to Novita and trigger key entry.
        move_to_provider(&mut picker, ApiProvider::Novita);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        for c in "novita-key".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        let action = picker.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ProviderPickerApiKeySubmitted {
                provider,
                api_key,
            }) => {
                assert_eq!(provider, ApiProvider::Novita);
                assert_eq!(api_key, "novita-key");
            }
            other => panic!("expected ProviderPickerApiKeySubmitted, got {other:?}"),
        }
    }

    #[test]
    fn key_entry_esc_returns_to_list_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        picker.handle_key(key(KeyCode::Char('a')));
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(picker.stage, Stage::List);
        assert!(picker.api_key_input.is_empty());
    }

    #[test]
    fn list_esc_closes_without_emitting() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let action = picker.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn key_entry_strips_whitespace_chars() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Openrouter);
        picker.handle_key(key(KeyCode::Enter));
        assert_eq!(picker.stage, Stage::KeyEntry);
        for c in "abc def".chars() {
            picker.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(picker.api_key_input, "abcdef");
    }

    #[test]
    fn small_list_render_keeps_selected_provider_visible_after_down_navigation() {
        let config = Config::default();
        let mut picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        move_to_provider(&mut picker, ApiProvider::Ollama);

        let rendered = render_text(&picker, 80, 12);

        assert!(rendered.contains("Ollama"));
        assert!(!rendered.contains("DeepSeek *"));
    }

    #[test]
    fn small_list_render_keeps_initial_active_provider_visible() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Ollama, &config);

        let rendered = render_text(&picker, 80, 12);

        assert!(rendered.contains("Ollama *"));
    }

    #[test]
    fn tall_list_render_shows_all_providers_without_scrolling() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);

        let rendered = render_text(&picker, 80, 23);

        assert!(rendered.contains("DeepSeek *"));
        assert!(rendered.contains("Ollama"));
    }

    #[test]
    fn selected_provider_row_uses_strong_highlight() {
        let config = Config::default();
        let picker = ProviderPickerView::new(ApiProvider::Deepseek, &config);
        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);

        picker.render(area, &mut buf);

        let highlighted_cells = area
            .positions()
            .filter(|position| {
                let cell = &buf[*position];
                cell.bg == palette::SURFACE_ELEVATED
            })
            .count();
        assert!(
            highlighted_cells >= 32,
            "selected provider row should use a visible continuous highlight"
        );
    }
}
