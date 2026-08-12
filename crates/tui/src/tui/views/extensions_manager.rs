//! Installed extension lifecycle manager opened by bare `/plugins`.
//!
//! The view projects immutable registry state only. Every write is emitted as
//! [`ViewEvent::PluginManagerActionRequested`] and executed by the host via
//! [`crate::plugins::controller::PluginController`].

use std::cell::RefCell;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use super::{
    ActionHint, EmptyState, ListDetailLayout, ModalKind, ModalView, ViewAction, ViewEvent,
    render_modal_footer, render_underwater_surface, truncate_view_text,
};
use crate::commands::{LegacyToolApproval, LegacyToolInventoryEntry, legacy_tool_inventory};
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::plugins::controller::PluginAction;
use crate::plugins::types::{
    LoadedPlugin, PluginDiagnosticLevel, PluginId, PluginOrigin, PluginScope,
};
use crate::tui::app::App;
use crate::tui::menu_style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagerTab {
    Installed,
    Marketplace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusFilter {
    All,
    Active,
    NeedsReview,
    Disabled,
    Error,
}

impl StatusFilter {
    const ALL: [Self; 5] = [
        Self::All,
        Self::Active,
        Self::NeedsReview,
        Self::Disabled,
        Self::Error,
    ];

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn label(self, locale: Locale) -> String {
        match self {
            Self::All => tr(locale, MessageId::ExtensionsManagerFilterAll).into_owned(),
            Self::Active => tr(locale, MessageId::ExtensionsManagerFilterActive).into_owned(),
            Self::NeedsReview => {
                tr(locale, MessageId::ExtensionsManagerFilterNeedsReview).into_owned()
            }
            Self::Disabled => tr(locale, MessageId::ExtensionsManagerFilterDisabled).into_owned(),
            Self::Error => tr(locale, MessageId::ExtensionsManagerFilterError).into_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingConfirm {
    Disable(String),
    Revoke(String),
    Update(String),
    Uninstall(String),
    Install(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ExtensionRowId {
    Bundle(PluginId),
    Legacy { name: String, path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionRowKind {
    Bundle,
    LegacyExecutable { approval: LegacyToolApproval },
}

/// A local projection of a loaded registry entry. Cloning here keeps modal
/// rendering independent from app borrows while still showing the exact digest
/// that must be confirmed for trust.
#[derive(Debug, Clone)]
struct PluginRow {
    id: ExtensionRowId,
    kind: ExtensionRowKind,
    group: String,
    name: String,
    version: String,
    source: String,
    scope: String,
    state: String,
    trust: String,
    description: Option<String>,
    inventory: String,
    permissions: String,
    mcp_servers: String,
    unsupported: String,
    content_hash: String,
    capability_hash: String,
    diagnostics: Vec<String>,
    has_error: bool,
    enabled: bool,
    trusted: bool,
    removable: bool,
    updateable: bool,
}

impl PluginRow {
    fn from_loaded(plugin: &LoadedPlugin, locale: Locale) -> Self {
        Self {
            id: ExtensionRowId::Bundle(plugin.id.clone()),
            kind: ExtensionRowKind::Bundle,
            group: format!(
                "{} ({})",
                origin_label(locale, plugin.origin),
                scope_label(locale, plugin.scope)
            ),
            name: safe_display_text(plugin.name()),
            version: safe_display_text(&plugin.manifest.plugin.version),
            source: origin_label(locale, plugin.origin),
            scope: scope_label(locale, plugin.scope),
            state: plugin.state_label().to_string(),
            trust: plugin.trust_status.as_str().to_string(),
            description: plugin
                .manifest
                .plugin
                .description
                .as_deref()
                .map(safe_display_text),
            inventory: format_inventory(plugin, locale),
            permissions: format_permissions(plugin, locale),
            mcp_servers: format_mcp_servers(plugin, locale),
            unsupported: format_unsupported(plugin, locale),
            content_hash: plugin.content_hash.clone(),
            capability_hash: plugin.capability_hash.clone(),
            has_error: plugin
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == PluginDiagnosticLevel::Error),
            diagnostics: plugin
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    let level = text(
                        locale,
                        match diagnostic.level {
                            PluginDiagnosticLevel::Warning => MessageId::ExtensionsManagerWarning,
                            PluginDiagnosticLevel::Error => MessageId::ExtensionsManagerError,
                        },
                    );
                    format!("{level}: {}", safe_display_text(&diagnostic.message))
                })
                .collect(),
            enabled: plugin.enabled,
            trusted: plugin.trusted(),
            removable: plugin.scope == PluginScope::User,
            updateable: plugin.scope == PluginScope::User,
        }
    }

    fn from_legacy(tool: LegacyToolInventoryEntry, locale: Locale) -> Self {
        let approval = legacy_approval_label(locale, tool.approval);
        Self {
            id: ExtensionRowId::Legacy {
                name: tool.name.clone(),
                path: tool.path.clone(),
            },
            kind: ExtensionRowKind::LegacyExecutable {
                approval: tool.approval,
            },
            group: text(locale, MessageId::ExtensionsManagerLegacyTools),
            name: safe_display_text(&tool.name),
            version: text(locale, MessageId::ExtensionsManagerNone),
            source: safe_display_text(&tool.path.display().to_string()),
            scope: text(locale, MessageId::ExtensionsManagerLegacyToolKind),
            state: text(locale, MessageId::ExtensionsManagerLegacyToolKind),
            trust: approval.clone(),
            description: Some(safe_display_text(&tool.description)),
            inventory: text(locale, MessageId::ExtensionsManagerLegacyToolKind),
            permissions: text(locale, MessageId::ExtensionsManagerLegacyToolApproval)
                .replace("{approval}", &approval),
            mcp_servers: text(locale, MessageId::ExtensionsManagerNone),
            unsupported: text(locale, MessageId::ExtensionsManagerNone),
            content_hash: String::new(),
            capability_hash: String::new(),
            diagnostics: Vec::new(),
            has_error: false,
            enabled: false,
            trusted: false,
            removable: false,
            updateable: false,
        }
    }
}

pub struct ExtensionsManagerView {
    locale: Locale,
    tab: ManagerTab,
    all_rows: Vec<PluginRow>,
    visible: Vec<ExtensionRowId>,
    selected: usize,
    selected_id: Option<ExtensionRowId>,
    query: String,
    filter: StatusFilter,
    search_active: bool,
    trust_token_input: Option<String>,
    install_source_input: Option<String>,
    detail_scroll: usize,
    pending: Option<PendingConfirm>,
    status: Option<String>,
    row_hitboxes: RefCell<Vec<(Rect, ExtensionRowId)>>,
}

impl ExtensionsManagerView {
    #[must_use]
    pub fn new(app: &App) -> Self {
        Self::from_registry(
            app,
            ManagerTab::Installed,
            String::new(),
            StatusFilter::All,
            None,
            None,
        )
    }

    #[must_use]
    pub fn rebuild_preserving(app: &App, previous: &Self, status: Option<String>) -> Self {
        Self::from_registry(
            app,
            previous.tab,
            previous.query.clone(),
            previous.filter,
            previous.selected_id.clone(),
            status,
        )
    }

    fn from_registry(
        app: &App,
        tab: ManagerTab,
        query: String,
        filter: StatusFilter,
        focus: Option<ExtensionRowId>,
        status: Option<String>,
    ) -> Self {
        let mut all_rows = app
            .plugin_registry
            .list()
            .into_iter()
            .map(|plugin| PluginRow::from_loaded(plugin, app.ui_locale))
            .collect::<Vec<_>>();
        all_rows.extend(
            legacy_tool_inventory(app)
                .into_iter()
                .map(|tool| PluginRow::from_legacy(tool, app.ui_locale)),
        );
        all_rows.sort_by(|left, right| {
            left.group
                .cmp(&right.group)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut view = Self {
            locale: app.ui_locale,
            tab,
            all_rows,
            visible: Vec::new(),
            selected: 0,
            selected_id: focus,
            query,
            filter,
            search_active: false,
            trust_token_input: None,
            install_source_input: None,
            detail_scroll: 0,
            pending: None,
            status,
            row_hitboxes: RefCell::new(Vec::new()),
        };
        view.refilter(None);
        view
    }

    fn text(&self, id: MessageId) -> String {
        tr(self.locale, id).into_owned()
    }

    fn refilter(&mut self, preserve: Option<&ExtensionRowId>) {
        if let Some(preserve) = preserve {
            self.selected_id = Some(preserve.clone());
        }
        let query = self.query.to_ascii_lowercase();
        self.visible = self
            .all_rows
            .iter()
            .filter(|row| {
                self.filter_matches(row)
                    && (query.is_empty()
                        || row.name.to_ascii_lowercase().contains(&query)
                        || row.id.to_string().to_ascii_lowercase().contains(&query)
                        || row.inventory.to_ascii_lowercase().contains(&query)
                        || row.description.as_deref().is_some_and(|description| {
                            description.to_ascii_lowercase().contains(&query)
                        }))
            })
            .map(|row| row.id.clone())
            .collect();
        self.selected = self
            .selected_id
            .as_ref()
            .and_then(|id| self.visible.iter().position(|visible| visible == id))
            .unwrap_or(0);
        if self.selected_id.is_none() {
            self.selected_id = self.visible.first().cloned();
        }
        self.clamp_selection();
    }

    fn filter_matches(&self, row: &PluginRow) -> bool {
        match self.filter {
            StatusFilter::All => true,
            StatusFilter::Active => row.state == "active",
            StatusFilter::NeedsReview => !row.trusted,
            StatusFilter::Disabled => !row.enabled,
            StatusFilter::Error => row.has_error,
        }
    }

    fn selected_row(&self) -> Option<&PluginRow> {
        let id = self.selected_id.as_ref()?;
        self.all_rows.iter().find(|row| &row.id == id)
    }

    fn selected_bundle_name(&self) -> Option<String> {
        self.selected_row()
            .and_then(|row| matches!(row.kind, ExtensionRowKind::Bundle).then(|| row.name.clone()))
    }

    fn clamp_selection(&mut self) {
        if self.visible.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.visible.len() - 1);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let len = self.visible.len() as isize;
        let current = self
            .selected_id
            .as_ref()
            .and_then(|id| self.visible.iter().position(|visible| visible == id))
            .unwrap_or(self.selected.min(self.visible.len() - 1));
        self.selected = (current as isize + delta).rem_euclid(len) as usize;
        self.selected_id = self.visible.get(self.selected).cloned();
        self.detail_scroll = 0;
        self.pending = None;
    }

    fn cycle_tab(&mut self) {
        self.tab = match self.tab {
            ManagerTab::Installed => ManagerTab::Marketplace,
            ManagerTab::Marketplace => ManagerTab::Installed,
        };
        self.pending = None;
        self.search_active = false;
    }

    fn selected_action(&mut self, key: char) -> ViewAction {
        let Some(row) = self.selected_row().cloned() else {
            return ViewAction::None;
        };
        if matches!(row.kind, ExtensionRowKind::LegacyExecutable { .. }) {
            self.status = Some(self.text(MessageId::ExtensionsManagerLegacyToolRunHint));
            return ViewAction::None;
        }
        match key {
            ' ' => {
                if row.enabled {
                    self.pending = Some(PendingConfirm::Disable(row.name.clone()));
                    self.status = Some(self.text(MessageId::ExtensionsManagerConfirmDisable));
                    ViewAction::None
                } else if row.trusted {
                    ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                        action: PluginAction::Enable { selector: row.name },
                    })
                } else {
                    self.status = Some(self.text(MessageId::ExtensionsManagerReviewRequired));
                    ViewAction::None
                }
            }
            't' | 'T' => {
                self.trust_token_input = Some(String::new());
                self.status = Some(format!(
                    "{} {}.{}",
                    self.text(MessageId::ExtensionsManagerTrustToken),
                    row.content_hash,
                    row.capability_hash
                ));
                ViewAction::None
            }
            'r' | 'R' => ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                action: PluginAction::Reload,
            }),
            'u' | 'U' if row.updateable => {
                self.pending = Some(PendingConfirm::Update(row.name));
                self.status = Some(self.text(MessageId::ExtensionsManagerConfirmUpdate));
                ViewAction::None
            }
            'd' | 'D' if row.removable => {
                self.pending = Some(PendingConfirm::Uninstall(row.name));
                self.status = Some(self.text(MessageId::ExtensionsManagerConfirmUninstall));
                ViewAction::None
            }
            'x' | 'X' if row.trusted => {
                self.pending = Some(PendingConfirm::Revoke(row.name));
                self.status = Some(self.text(MessageId::ExtensionsManagerConfirmRevoke));
                ViewAction::None
            }
            _ => {
                self.status = Some(self.text(MessageId::ExtensionsManagerActionUnavailable));
                ViewAction::None
            }
        }
    }

    fn confirm_pending(&mut self) -> ViewAction {
        match self.pending.take() {
            Some(PendingConfirm::Disable(selector)) => {
                ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                    action: PluginAction::Disable { selector },
                })
            }
            Some(PendingConfirm::Revoke(selector)) => {
                ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                    action: PluginAction::Revoke { selector },
                })
            }
            Some(PendingConfirm::Update(selector)) => {
                ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                    action: PluginAction::Update { selector },
                })
            }
            Some(PendingConfirm::Uninstall(selector)) => {
                ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                    action: PluginAction::Uninstall { selector },
                })
            }
            Some(PendingConfirm::Install(spec)) => {
                ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                    action: PluginAction::Install { spec },
                })
            }
            None => ViewAction::None,
        }
    }

    fn footer_hints(&self, width: u16) -> Vec<ActionHint> {
        if self.pending.is_some() {
            return vec![
                ActionHint::new("Enter", self.text(MessageId::ExtensionsManagerConfirm)),
                ActionHint::new("Esc", self.text(MessageId::ExtensionsManagerCancel)),
            ];
        }
        if width < 54 {
            return vec![
                ActionHint::new("↑/↓", self.text(MessageId::ExtensionsManagerMove)),
                ActionHint::new("Esc", self.text(MessageId::ExtensionsManagerClose)),
            ];
        }
        vec![
            ActionHint::new("↑/↓", self.text(MessageId::ExtensionsManagerMove)),
            ActionHint::new("Enter", self.text(MessageId::ExtensionsManagerEnterDetails)),
            ActionHint::new("/", self.text(MessageId::ExtensionsManagerSearch)),
            ActionHint::new("Space", self.text(MessageId::ExtensionsManagerToggle)),
            ActionHint::new("t", self.text(MessageId::ExtensionsManagerTrust)),
            ActionHint::new("f", self.text(MessageId::ExtensionsManagerFilter)),
            ActionHint::new("v", self.text(MessageId::ExtensionsManagerValidate)),
            ActionHint::new("r", self.text(MessageId::ExtensionsManagerReload)),
            ActionHint::new("i", self.text(MessageId::ExtensionsManagerInstall)),
            ActionHint::new("Tab", self.text(MessageId::ExtensionsManagerTab)),
            ActionHint::new("Esc", self.text(MessageId::ExtensionsManagerClose)),
        ]
    }

    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        let title = format!(
            " {} ({}) ",
            self.text(MessageId::ExtensionsManagerInstalled),
            self.visible.len()
        );
        let block = Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = block.inner(area);
        block.render(area, buf);
        self.row_hitboxes.borrow_mut().clear();
        if self.visible.is_empty() {
            EmptyState::new(
                self.text(MessageId::ExtensionsManagerNoPlugins),
                self.text(MessageId::ExtensionsManagerNoPluginsBody),
            )
            .render(inner, buf);
            return;
        }
        let mut rendered = Vec::new();
        let mut previous_group = None;
        for (index, id) in self.visible.iter().enumerate() {
            let Some(row) = self.all_rows.iter().find(|candidate| candidate.id == *id) else {
                continue;
            };
            if previous_group.replace(&row.group) != Some(&row.group) {
                rendered.push((None, format!("— {} —", row.group)));
            }
            let state = match row.kind {
                ExtensionRowKind::Bundle => state_label(self.locale, &row.state),
                ExtensionRowKind::LegacyExecutable { .. } => row.state.clone(),
            };
            rendered.push((
                Some(index),
                format!(
                    "{} {:<16} {}",
                    crate::tui::glyphs::selection_marker(
                        self.selected_id.as_ref() == Some(&row.id)
                    ),
                    truncate_view_text(&row.name, 16),
                    truncate_view_text(&state, 14),
                ),
            ));
        }
        let selected_line = rendered
            .iter()
            .position(|(index, _)| *index == Some(self.selected))
            .unwrap_or(0);
        let visible_rows = usize::from(inner.height).max(1);
        let offset = selected_line.saturating_add(1).saturating_sub(visible_rows);
        for (line, (index, line_text)) in
            rendered.iter().skip(offset).take(visible_rows).enumerate()
        {
            let y = inner.y.saturating_add(line as u16);
            let selected = index.is_some_and(|index| index == self.selected);
            let style = if index.is_some() && selected {
                menu_style::selected_row_style()
            } else if index.is_some() {
                Style::default().fg(palette::TEXT_PRIMARY)
            } else {
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD)
            };
            buf.set_stringn(inner.x, y, line_text, usize::from(inner.width), style);
            if let Some(index) = index {
                let id = self.visible[*index].clone();
                self.row_hitboxes.borrow_mut().push((
                    Rect {
                        x: inner.x,
                        y,
                        width: inner.width,
                        height: 1,
                    },
                    id,
                ));
            }
        }
    }

    fn render_detail(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(Line::from(Span::styled(
                format!(" {} ", self.text(MessageId::ExtensionsManagerDetails)),
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG));
        let inner = block.inner(area);
        block.render(area, buf);
        let Some(row) = self.selected_row() else {
            EmptyState::new(
                self.text(MessageId::ExtensionsManagerNoSelection),
                self.text(MessageId::ExtensionsManagerNoSelectionBody),
            )
            .render(inner, buf);
            return;
        };
        let token = format!("{}.{}", row.content_hash, row.capability_hash);
        let state = match row.kind {
            ExtensionRowKind::Bundle => state_label(self.locale, &row.state),
            ExtensionRowKind::LegacyExecutable { .. } => row.state.clone(),
        };
        let trust = match row.kind {
            ExtensionRowKind::Bundle => trust_label(self.locale, &row.trust),
            ExtensionRowKind::LegacyExecutable { approval } => {
                legacy_approval_label(self.locale, approval)
            }
        };
        let mut lines = vec![
            Line::from(Span::styled(
                row.name.clone(),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )),
            kv_line(
                &self.text(MessageId::ExtensionsManagerVersion),
                &row.version,
            ),
            kv_line(&self.text(MessageId::ExtensionsManagerSource), &row.source),
            kv_line(&self.text(MessageId::ExtensionsManagerScope), &row.scope),
            kv_line(&self.text(MessageId::ExtensionsManagerState), &state),
            kv_line(&self.text(MessageId::ExtensionsManagerTrustStatus), &trust),
            kv_line(
                &self.text(MessageId::ExtensionsManagerComponents),
                &row.inventory,
            ),
            kv_line(
                &self.text(MessageId::ExtensionsManagerPermissions),
                &row.permissions,
            ),
            kv_line(
                &self.text(MessageId::ExtensionsManagerMcpServers),
                &row.mcp_servers,
            ),
            kv_line(
                &self.text(MessageId::ExtensionsManagerInactiveCapabilities),
                &row.unsupported,
            ),
        ];
        match row.kind {
            ExtensionRowKind::Bundle => lines.push(kv_line(
                &self.text(MessageId::ExtensionsManagerReviewToken),
                &token,
            )),
            ExtensionRowKind::LegacyExecutable { approval } => {
                lines.push(Line::from(""));
                lines.push(Line::from(
                    self.text(MessageId::ExtensionsManagerLegacyToolRunHint),
                ));
                lines.push(kv_line(
                    &self.text(MessageId::ExtensionsManagerLegacyToolApproval),
                    &legacy_approval_label(self.locale, approval),
                ));
            }
        }
        if let Some(description) = row.description.as_deref() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.text(MessageId::ExtensionsManagerDescription),
                Style::default().fg(palette::TEXT_MUTED),
            )));
            lines.push(Line::from(description.to_string()));
        }
        if !row.diagnostics.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.text(MessageId::ExtensionsManagerDiagnostics),
                Style::default().fg(palette::STATUS_WARNING),
            )));
            lines.extend(
                row.diagnostics
                    .iter()
                    .map(|line| Line::from(format!("• {line}"))),
            );
        }
        if let Some(status) = &self.status {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                status.clone(),
                Style::default().fg(palette::WHALE_ACTION),
            )));
        }
        Paragraph::new(
            lines
                .into_iter()
                .skip(self.detail_scroll)
                .collect::<Vec<_>>(),
        )
        .style(Style::default().fg(palette::TEXT_PRIMARY))
        .wrap(Wrap { trim: false })
        .render(inner, buf);
    }

    fn render_marketplace(&self, area: Rect, buf: &mut Buffer) {
        EmptyState::new(
            self.text(MessageId::ExtensionsManagerMarketplaceUnavailable),
            self.text(MessageId::ExtensionsManagerMarketplaceBody),
        )
        .render(area, buf);
    }
}

impl ModalView for ExtensionsManagerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ExtensionsManager
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if self.pending.is_some() {
            return match key.code {
                KeyCode::Enter => self.confirm_pending(),
                KeyCode::Esc => {
                    self.pending = None;
                    self.status = Some(self.text(MessageId::ExtensionsManagerCancelled));
                    ViewAction::None
                }
                _ => ViewAction::None,
            };
        }
        if self.search_active {
            match key.code {
                KeyCode::Esc => {
                    self.search_active = false;
                    ViewAction::None
                }
                KeyCode::Enter => {
                    self.search_active = false;
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.refilter(None);
                    ViewAction::None
                }
                KeyCode::Char(character)
                    if !character.is_control()
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
                {
                    self.query.push(character);
                    self.refilter(None);
                    ViewAction::None
                }
                _ => ViewAction::None,
            }
        } else if self.trust_token_input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.trust_token_input = None;
                    self.status = Some(self.text(MessageId::ExtensionsManagerCancelled));
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    if let Some(input) = self.trust_token_input.as_mut() {
                        input.pop();
                    }
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let Some(row) = self.selected_row().cloned() else {
                        return ViewAction::None;
                    };
                    let review_token = self.trust_token_input.take().unwrap_or_default();
                    ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                        action: PluginAction::Trust {
                            selector: row.name,
                            review_token,
                        },
                    })
                }
                KeyCode::Char(character)
                    if !character.is_control()
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
                {
                    if let Some(input) = self.trust_token_input.as_mut() {
                        input.push(character);
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            }
        } else if self.install_source_input.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.install_source_input = None;
                    self.status = Some(self.text(MessageId::ExtensionsManagerCancelled));
                    ViewAction::None
                }
                KeyCode::Backspace => {
                    if let Some(input) = self.install_source_input.as_mut() {
                        input.pop();
                    }
                    ViewAction::None
                }
                KeyCode::Enter => {
                    let spec = self.install_source_input.take().unwrap_or_default();
                    if spec.trim().is_empty() {
                        self.status =
                            Some(self.text(MessageId::ExtensionsManagerInstallSourceRequired));
                        ViewAction::None
                    } else {
                        self.pending = Some(PendingConfirm::Install(spec));
                        self.status = Some(self.text(MessageId::ExtensionsManagerConfirmInstall));
                        ViewAction::None
                    }
                }
                KeyCode::Char(character)
                    if !character.is_control()
                        && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
                {
                    if let Some(input) = self.install_source_input.as_mut() {
                        input.push(character);
                    }
                    ViewAction::None
                }
                _ => ViewAction::None,
            }
        } else {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => ViewAction::Close,
                KeyCode::Tab | KeyCode::BackTab => {
                    self.cycle_tab();
                    ViewAction::None
                }
                KeyCode::Char('/') => {
                    self.search_active = true;
                    ViewAction::None
                }
                KeyCode::Char('f') | KeyCode::Char('F') if self.tab == ManagerTab::Installed => {
                    self.filter = self.filter.next();
                    self.refilter(None);
                    ViewAction::None
                }
                KeyCode::Char('v') | KeyCode::Char('V') if self.tab == ManagerTab::Installed => {
                    match self.selected_bundle_name() {
                        Some(selector) => {
                            ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                                action: PluginAction::Validate {
                                    selector: Some(selector),
                                },
                            })
                        }
                        None => {
                            self.status =
                                Some(self.text(MessageId::ExtensionsManagerLegacyToolRunHint));
                            ViewAction::None
                        }
                    }
                }
                KeyCode::Char('i') | KeyCode::Char('I') if self.tab == ManagerTab::Installed => {
                    self.install_source_input = Some(String::new());
                    self.status = Some(self.text(MessageId::ExtensionsManagerInstallPrompt));
                    ViewAction::None
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                    self.move_selection(-1);
                    ViewAction::None
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                    self.move_selection(1);
                    ViewAction::None
                }
                KeyCode::PageUp => {
                    self.detail_scroll = self.detail_scroll.saturating_sub(8);
                    ViewAction::None
                }
                KeyCode::PageDown => {
                    self.detail_scroll = self.detail_scroll.saturating_add(8);
                    ViewAction::None
                }
                KeyCode::Enter if self.tab == ManagerTab::Installed => {
                    if matches!(
                        self.selected_row().map(|row| row.kind),
                        Some(ExtensionRowKind::LegacyExecutable { .. })
                    ) {
                        self.status =
                            Some(self.text(MessageId::ExtensionsManagerLegacyToolRunHint));
                    } else {
                        self.detail_scroll = 0;
                    }
                    ViewAction::None
                }
                KeyCode::Char(
                    character @ (' ' | 't' | 'T' | 'r' | 'R' | 'u' | 'U' | 'd' | 'D' | 'x' | 'X'),
                ) if self.tab == ManagerTab::Installed => self.selected_action(character),
                _ => ViewAction::None,
            }
        }
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if let Some(input) = self.trust_token_input.as_mut() {
            input.push_str(text.trim());
            return true;
        }
        if let Some(input) = self.install_source_input.as_mut() {
            input.push_str(text.trim());
            return true;
        }
        if self.search_active {
            self.query.push_str(text.trim());
            self.refilter(None);
            return true;
        }
        false
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(id) = self.row_hitboxes.borrow().iter().find_map(|(rect, id)| {
                    rect.contains(Position::new(mouse.column, mouse.row))
                        .then_some(id.clone())
                }) && let Some(index) = self.visible.iter().position(|visible| visible == &id)
                {
                    let was_selected = self.selected_id.as_ref() == Some(&id);
                    self.selected = index;
                    self.selected_id = Some(id);
                    self.detail_scroll = 0;
                    if was_selected {
                        self.status = Some(match self.selected_row().map(|row| row.kind) {
                            Some(ExtensionRowKind::LegacyExecutable { .. }) => {
                                self.text(MessageId::ExtensionsManagerLegacyToolRunHint)
                            }
                            _ => self.text(MessageId::ExtensionsManagerEnterDetails),
                        });
                    }
                }
            }
            _ => {}
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let body =
            render_underwater_surface(area, buf, self.text(MessageId::ExtensionsManagerTitle));
        let content = render_modal_footer(body, buf, &self.footer_hints(area.width));
        if content.height == 0 {
            return;
        }
        if area.width < 54 {
            if self.tab == ManagerTab::Marketplace {
                self.render_marketplace(content, buf);
                return;
            }
            let row = self.selected_row();
            let title = row.map_or_else(
                || self.text(MessageId::ExtensionsManagerNoSelection),
                |row| row.name.clone(),
            );
            let body = row.map_or_else(
                || self.text(MessageId::ExtensionsManagerNoSelectionBody),
                |row| {
                    format!(
                        "{} · {} · {}",
                        state_label(self.locale, &row.state),
                        trust_label(self.locale, &row.trust),
                        row.inventory
                    )
                },
            );
            Paragraph::new(vec![
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(palette::WHALE_INFO)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(body),
                Line::from(
                    self.status
                        .clone()
                        .unwrap_or_else(|| self.text(MessageId::ExtensionsManagerEnterDetails)),
                ),
            ])
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .wrap(Wrap { trim: false })
            .render(content, buf);
            return;
        }
        let header = Rect {
            x: content.x,
            y: content.y,
            width: content.width,
            height: 2.min(content.height),
        };
        let active_tab = match self.tab {
            ManagerTab::Installed => self.text(MessageId::ExtensionsManagerInstalled),
            ManagerTab::Marketplace => self.text(MessageId::ExtensionsManagerMarketplace),
        };
        let search = if let Some(source) = &self.install_source_input {
            format!(
                "{} {source}",
                self.text(MessageId::ExtensionsManagerInstallPrompt)
            )
        } else if let Some(token) = &self.trust_token_input {
            format!(
                "{} {token}",
                self.text(MessageId::ExtensionsManagerTrustToken)
            )
        } else if self.search_active {
            format!(
                "{}{}",
                self.text(MessageId::ExtensionsManagerSearch),
                self.query
            )
        } else if self.query.is_empty() {
            self.text(MessageId::ExtensionsManagerSearchIdle)
        } else {
            format!(
                "{}{}",
                self.text(MessageId::ExtensionsManagerSearch),
                self.query
            )
        };
        let header_line = format!(
            "{}  {} · {}  {} · {}: {}",
            self.text(MessageId::ExtensionsManagerTabs),
            active_tab,
            self.text(MessageId::ExtensionsManagerMarketplace),
            search,
            self.text(MessageId::ExtensionsManagerFilter),
            self.filter.label(self.locale),
        );
        buf.set_stringn(
            header.x,
            header.y,
            truncate_view_text(&header_line, usize::from(header.width)),
            usize::from(header.width),
            Style::default().fg(palette::TEXT_SECONDARY),
        );
        let panel = Rect {
            x: content.x,
            y: content.y.saturating_add(header.height),
            width: content.width,
            height: content.height.saturating_sub(header.height),
        };
        match self.tab {
            ManagerTab::Installed => {
                let layout = ListDetailLayout::split(panel, 40);
                self.render_list(layout.list, buf);
                self.render_detail(layout.detail, buf);
            }
            ManagerTab::Marketplace => self.render_marketplace(panel, buf),
        }
    }
}

fn kv_line(key: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{key:<12}"),
            Style::default().fg(palette::TEXT_MUTED),
        ),
        Span::raw(value.to_string()),
    ])
}

fn text(locale: Locale, id: MessageId) -> String {
    tr(locale, id).into_owned()
}

fn state_label(locale: Locale, state: &str) -> String {
    text(
        locale,
        match state {
            "active" => MessageId::ExtensionsManagerStateActive,
            "disabled" => MessageId::ExtensionsManagerStateDisabled,
            "enabled-untrusted" => MessageId::ExtensionsManagerStateEnabledUntrusted,
            "unstaged" => MessageId::ExtensionsManagerStateUnstaged,
            "inapplicable" => MessageId::ExtensionsManagerStateInapplicable,
            "unsupported" => MessageId::ExtensionsManagerStateUnsupported,
            _ => MessageId::ExtensionsManagerStateInactive,
        },
    )
}

fn trust_label(locale: Locale, trust: &str) -> String {
    text(
        locale,
        match trust {
            "trusted" => MessageId::ExtensionsManagerTrustTrusted,
            "not-reviewed" => MessageId::ExtensionsManagerTrustNeverReviewed,
            "content-changed" => MessageId::ExtensionsManagerTrustContentChanged,
            _ => MessageId::ExtensionsManagerTrustCapabilitiesChanged,
        },
    )
}

fn scope_label(locale: Locale, scope: PluginScope) -> String {
    text(
        locale,
        match scope {
            PluginScope::Builtin => MessageId::ExtensionsManagerScopeBuiltin,
            PluginScope::User => MessageId::ExtensionsManagerScopeUser,
            PluginScope::Workspace => MessageId::ExtensionsManagerScopeWorkspace,
        },
    )
}

fn origin_label(locale: Locale, origin: PluginOrigin) -> String {
    text(
        locale,
        match origin {
            PluginOrigin::Builtin => MessageId::ExtensionsManagerOriginBuiltin,
            PluginOrigin::CodeWhaleHome => MessageId::ExtensionsManagerOriginHome,
            PluginOrigin::Workspace => MessageId::ExtensionsManagerOriginWorkspace,
        },
    )
}

fn legacy_approval_label(locale: Locale, approval: LegacyToolApproval) -> String {
    text(
        locale,
        match approval {
            LegacyToolApproval::Auto => MessageId::ExtensionsManagerLegacyToolApprovalAuto,
            LegacyToolApproval::Suggest => MessageId::ExtensionsManagerLegacyToolApprovalSuggest,
            LegacyToolApproval::Required => MessageId::ExtensionsManagerLegacyToolApprovalRequired,
        },
    )
}

impl std::fmt::Display for ExtensionRowId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bundle(id) => id.fmt(formatter),
            Self::Legacy { name, path } => write!(formatter, "{name}:{}", path.display()),
        }
    }
}

fn format_inventory(plugin: &LoadedPlugin, locale: Locale) -> String {
    let inventory = &plugin.inventory;
    text(locale, MessageId::ExtensionsManagerInventorySummary)
        .replace("{skills}", &inventory.skills.to_string())
        .replace("{mcp}", &inventory.mcp_servers.to_string())
        .replace("{stdio}", &inventory.stdio_mcp_servers.to_string())
        .replace("{remote}", &inventory.remote_mcp_servers.to_string())
        .replace("{commands}", &inventory.commands.to_string())
        .replace("{agents}", &inventory.agents.to_string())
        .replace("{hooks}", &inventory.hooks.to_string())
        .replace("{lsp}", &inventory.lsp.to_string())
        .replace("{native}", &inventory.native.to_string())
}

fn format_unsupported(plugin: &LoadedPlugin, locale: Locale) -> String {
    let labels = plugin
        .inventory
        .unsupported_labels()
        .into_iter()
        .map(|label| {
            text(
                locale,
                match label {
                    "commands" => MessageId::ExtensionsManagerUnsupportedCommands,
                    "agents" => MessageId::ExtensionsManagerUnsupportedAgents,
                    "hooks" => MessageId::ExtensionsManagerUnsupportedHooks,
                    "lsp" => MessageId::ExtensionsManagerUnsupportedLsp,
                    "native" => MessageId::ExtensionsManagerUnsupportedNative,
                    "filesystem-roots" => MessageId::ExtensionsManagerUnsupportedFilesystemRoots,
                    "lifecycle-mutation" => {
                        MessageId::ExtensionsManagerUnsupportedLifecycleMutation
                    }
                    _ => MessageId::ExtensionsManagerNone,
                },
            )
        })
        .collect::<Vec<_>>();
    if labels.is_empty() {
        text(locale, MessageId::ExtensionsManagerNone)
    } else {
        labels.join(", ")
    }
}

fn format_permissions(plugin: &LoadedPlugin, locale: Locale) -> String {
    let filesystem = if plugin.inventory.filesystem_roots.is_empty() {
        text(locale, MessageId::ExtensionsManagerNone)
    } else {
        plugin
            .inventory
            .filesystem_roots
            .iter()
            .map(|value| safe_display_text(value))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let network = if plugin.inventory.network_hosts.is_empty() {
        text(locale, MessageId::ExtensionsManagerNone)
    } else {
        plugin
            .inventory
            .network_hosts
            .iter()
            .map(|value| safe_display_text(value))
            .collect::<Vec<_>>()
            .join(", ")
    };
    text(locale, MessageId::ExtensionsManagerPermissionsSummary)
        .replace("{filesystem}", &filesystem)
        .replace("{network}", &network)
        .replace(
            "{lifecycle_mutation}",
            &text(
                locale,
                if plugin.inventory.lifecycle_mutation {
                    MessageId::ExtensionsManagerYes
                } else {
                    MessageId::ExtensionsManagerNo
                },
            ),
        )
}

fn format_mcp_servers(plugin: &LoadedPlugin, locale: Locale) -> String {
    let Some(servers) = plugin.manifest.mcp_servers.as_ref() else {
        return text(locale, MessageId::ExtensionsManagerNone);
    };
    let mut servers = servers
        .iter()
        .map(|(name, server)| {
            let transport = if server.command.is_some() {
                text(locale, MessageId::ExtensionsManagerMcpStdio)
            } else {
                text(locale, MessageId::ExtensionsManagerMcpRemote)
            };
            let target = server
                .command
                .as_deref()
                .or(server.url.as_deref())
                .map(safe_display_text)
                .unwrap_or_else(|| text(locale, MessageId::ExtensionsManagerMcpInvalid));
            format!("{}: {transport} {target}", safe_display_text(name))
        })
        .collect::<Vec<_>>();
    servers.sort_unstable();
    servers.join("; ")
}

fn safe_display_text(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            character if character.is_control() => '�',
            character => character,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use crossterm::event::{KeyEventKind, KeyEventState};
    use std::fs;
    use tempfile::TempDir;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn app_with_plugins() -> (App, TempDir) {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        for name in ["alpha", "beta"] {
            let bundle = root.join(".codewhale/plugins").join(name);
            fs::create_dir_all(&bundle).unwrap();
            fs::write(
                bundle.join("plugin.toml"),
                format!("schema_version = 1\n[plugin]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )
            .unwrap();
        }
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(root)
        };
        let app = App::new_with_plugin_registry(
            options,
            &Config::default(),
            discovery.registry_for_workspace(root),
        );
        (app, temp)
    }

    fn app_with_user_plugin() -> (App, TempDir) {
        let temp = TempDir::new().unwrap();
        let home = temp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let bundle = home.join("plugins/demo");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("plugin.toml"),
            "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(temp.path())
        };
        let app = App::new_with_plugin_registry(
            options,
            &Config::default(),
            discovery.registry_for_workspace(temp.path()),
        );
        (app, temp)
    }

    fn app_with_legacy_tool() -> (App, TempDir) {
        let (mut app, temp) = app_with_plugins();
        let tools = temp.path().join("legacy-tools");
        fs::create_dir_all(&tools).unwrap();
        fs::write(
            tools.join("legacy.sh"),
            "#!/bin/sh\n# name: legacy-run\n# description: Legacy-only tool\n# approval: required\necho '{}'\n",
        )
        .unwrap();
        app.legacy_plugin_tools_dir = Some(tools);
        (app, temp)
    }

    fn rendered_text(view: &ExtensionsManagerView, area: Rect) -> String {
        let mut buffer = Buffer::empty(area);
        view.render(area, &mut buffer);
        (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn search_preserves_the_selected_plugin_identity_when_it_remains_visible() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        view.move_selection(1);
        let selected = view.selected_row().unwrap().id.clone();
        view.query = "b".into();
        view.refilter(None);
        assert_eq!(view.selected_row().unwrap().id, selected);
    }

    #[test]
    fn filtering_keeps_the_selected_detail_when_the_row_no_longer_matches() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        view.move_selection(1);
        let selected = view.selected_row().unwrap().id.clone();

        view.query = "alpha".into();
        view.refilter(None);

        assert!(view.visible.len() == 1);
        assert_eq!(view.selected_row().unwrap().id, selected);
    }

    #[test]
    fn filter_key_cycles_from_all_to_active_extensions() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);

        view.handle_key(key(KeyCode::Char('f')));

        assert!(view.visible.is_empty(), "test bundles are not active");
    }

    #[test]
    fn trust_status_displays_the_exact_digest_bound_confirmation_token() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        let row = view.selected_row().unwrap().clone();
        let action = view.handle_key(key(KeyCode::Char('t')));
        assert!(matches!(action, ViewAction::None));
        assert!(view.status.as_deref().is_some_and(|status| {
            status.contains(&format!("{}.{}", row.content_hash, row.capability_hash))
        }));
    }

    #[test]
    fn trust_emits_the_exact_token_that_the_operator_typed() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        let row = view.selected_row().unwrap().clone();
        let token = format!("{}.{}", row.content_hash, row.capability_hash);

        view.handle_key(key(KeyCode::Char('t')));
        for character in token.chars() {
            view.handle_key(key(KeyCode::Char(character)));
        }
        let action = view.handle_key(key(KeyCode::Enter));

        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                action: PluginAction::Trust { review_token, .. },
            }) if review_token == token
        ));
    }

    #[test]
    fn install_requires_a_second_explicit_confirmation_after_source_entry() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        let source = "path:/tmp/demo";

        view.handle_key(key(KeyCode::Char('i')));
        for character in source.chars() {
            view.handle_key(key(KeyCode::Char(character)));
        }
        let action = view.handle_key(key(KeyCode::Enter));

        assert!(matches!(action, ViewAction::None));
        assert!(matches!(view.pending, Some(PendingConfirm::Install(ref spec)) if spec == source));
        let action = view.handle_key(key(KeyCode::Enter));

        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                action: PluginAction::Install { spec },
            }) if spec == source
        ));
    }

    #[test]
    fn update_requires_an_explicit_confirmation_before_emitting() {
        let _lock = crate::test_support::lock_test_env();
        let (app, _temp) = app_with_user_plugin();
        let mut view = ExtensionsManagerView::new(&app);

        let action = view.handle_key(key(KeyCode::Char('u')));

        assert!(matches!(action, ViewAction::None));
        assert!(view.pending.is_some());
    }

    #[test]
    fn trust_review_consumes_a_pasted_full_token() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        let row = view.selected_row().unwrap().clone();
        let token = format!("{}.{}", row.content_hash, row.capability_hash);

        view.handle_key(key(KeyCode::Char('t')));
        assert!(view.handle_paste(&token));
        let action = view.handle_key(key(KeyCode::Enter));

        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::PluginManagerActionRequested {
                action: PluginAction::Trust { review_token, .. },
            }) if review_token == token
        ));
    }

    #[test]
    fn validate_key_emits_a_manager_lifecycle_action() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);

        let action = view.handle_key(key(KeyCode::Char('v')));

        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::PluginManagerActionRequested { .. })
        ));
    }

    #[test]
    fn narrow_render_keeps_selected_plugin_name_visible() {
        let (app, _temp) = app_with_plugins();
        let view = ExtensionsManagerView::new(&app);
        let selected_name = view.selected_row().unwrap().name.clone();
        let area = Rect::new(0, 0, 40, 18);
        let output = rendered_text(&view, area);
        assert!(output.contains(&selected_name), "{output}");
    }

    #[test]
    fn narrow_marketplace_keeps_the_unavailable_provenance_boundary_visible() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        view.cycle_tab();

        let output = rendered_text(&view, Rect::new(0, 0, 40, 18));

        assert!(output.contains("Marketplace unavailable"), "{output}");
        assert!(output.contains("curated publisher"), "{output}");
    }

    #[test]
    fn groups_are_rendered_as_distinct_non_selectable_headers() {
        let (app, _temp) = app_with_plugins();
        let view = ExtensionsManagerView::new(&app);

        let output = rendered_text(&view, Rect::new(0, 0, 100, 24));

        assert!(output.contains("— workspace (workspace) —"), "{output}");
    }

    #[test]
    fn legacy_executable_tools_are_listed_with_their_independent_approval_state() {
        let (app, _temp) = app_with_legacy_tool();
        let view = ExtensionsManagerView::new(&app);

        let output = rendered_text(&view, Rect::new(0, 0, 100, 24));

        assert!(output.contains("Legacy executable tools"), "{output}");
        assert!(output.contains("legacy-run"), "{output}");
    }

    #[test]
    fn legacy_details_keep_execution_approval_distinct_from_bundle_trust() {
        let (app, _temp) = app_with_legacy_tool();
        let mut view = ExtensionsManagerView::new(&app);
        let legacy = view
            .all_rows
            .iter()
            .find(|row| matches!(row.kind, ExtensionRowKind::LegacyExecutable { .. }))
            .unwrap()
            .id
            .clone();
        view.selected = view.visible.iter().position(|id| id == &legacy).unwrap();
        view.selected_id = Some(legacy);

        let output = rendered_text(&view, Rect::new(0, 0, 100, 24));

        assert!(output.contains("legacy executable"), "{output}");
        assert!(output.contains("required"), "{output}");
        assert!(!output.contains("capabilities changed"), "{output}");
    }

    #[test]
    fn rebuild_preserves_the_selected_extension_query_and_filter() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        view.move_selection(1);
        let selected = view.selected_row().unwrap().id.clone();
        view.query = "beta".into();
        view.filter = StatusFilter::Disabled;
        view.refilter(None);

        let rebuilt = ExtensionsManagerView::rebuild_preserving(&app, &view, None);

        assert_eq!(rebuilt.query, "beta");
        assert_eq!(rebuilt.filter, StatusFilter::Disabled);
        assert_eq!(rebuilt.selected_id.as_ref(), Some(&selected));
    }

    #[test]
    fn repeated_click_on_a_bundle_gives_detail_interaction_feedback() {
        let (app, _temp) = app_with_plugins();
        let mut view = ExtensionsManagerView::new(&app);
        let area = Rect::new(0, 0, 100, 24);
        let mut buffer = Buffer::empty(area);
        view.render(area, &mut buffer);
        let row = view.row_hitboxes.borrow().first().cloned().unwrap().0;
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: row.x,
            row: row.y,
            modifiers: KeyModifiers::NONE,
        };

        view.handle_mouse(click);

        assert_eq!(
            view.status.as_deref(),
            Some("show details"),
            "a repeated click provides the same detail interaction feedback as Enter"
        );
    }

    #[test]
    fn renders_the_active_filter_in_the_selected_locale() {
        let (mut app, _temp) = app_with_plugins();
        app.ui_locale = Locale::ZhHans;
        let mut view = ExtensionsManagerView::new(&app);
        view.filter = StatusFilter::Active;
        let area = Rect::new(0, 0, 100, 24);
        let mut buffer = Buffer::empty(area);

        view.render(area, &mut buffer);

        let output = (area.y..area.bottom())
            .map(|y| {
                (area.x..area.right())
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("活 动"), "{output}");
    }
}
