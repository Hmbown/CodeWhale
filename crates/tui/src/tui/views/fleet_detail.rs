//! Fleet detail — open a saved named Fleet and edit it.
//!
//! Row 0 is the Coordinator's own model; below it one row per member.
//! Editing a Fleet edits that Fleet's file — never the live session route and
//! never a global collection of role profiles. Every write goes through
//! [`crate::fleet::store`] with an atomic save and a receipt naming the exact
//! file and scope.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

use crate::config::{ApiProvider, Config};
use crate::fleet::role::public_role_label;
use crate::fleet::store::{
    FleetFile, FleetMember, FleetOperator, FleetScope, MemberCapability, load_fleet_in_scope,
    save_fleet, set_selected,
};
use crate::palette;
use crate::tui::app::App;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_footer,
};

/// The built-in role vocabulary offered when adding a member, in a useful
/// order. A Fleet member is a role; the user can name anything, these are the
/// known postures.
const KNOWN_ROLES: [&str; 8] = [
    "explore",
    "implement",
    "reviewer",
    "test",
    "manager",
    "advisor",
    "summarizer",
    "general",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailStep {
    Overview,
    PickRoute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickTarget {
    Operator,
    Member(usize),
}

/// One selectable route row in the picker step: inherit or a concrete
/// provider/model with its readiness label.
#[derive(Debug, Clone)]
struct RouteRow {
    label: String,
    summary: String,
    provider: Option<String>,
    model: Option<String>,
}

pub struct FleetDetailView {
    fleet: FleetFile,
    scope: FleetScope,
    source: PathBuf,
    workspace: PathBuf,
    /// 0 = operator row; 1.. = members.
    selected: usize,
    step: DetailStep,
    pick_target: PickTarget,
    routes: Vec<RouteRow>,
    pick_row: usize,
    // Inline rename.
    rename_mode: bool,
    rename_input: String,
    // Delete confirmation.
    pending_remove: bool,
    /// The resolved Scout route shown before a run (pinned / verified
    /// companion / inherited / unavailable), refreshed on route edits.
    scout_receipt: Option<String>,
    /// Session route at open, used to resolve the unpinned Scout.
    session_provider: String,
    session_model: String,
}

impl FleetDetailView {
    /// Open a saved Fleet by name and scope. The caller (the list view) names
    /// the scope explicitly, so ambiguity is impossible here.
    pub fn open(app: &App, config: &Config, name: &str, scope: FleetScope) -> Option<Self> {
        Self::open_for_member(app, config, name, scope, None)
    }

    /// Open the exact named Fleet and, when the request came from a roster
    /// member, focus that member in the v2 editor.
    pub(crate) fn open_for_member(
        app: &App,
        config: &Config,
        name: &str,
        scope: FleetScope,
        member_id: Option<&str>,
    ) -> Option<Self> {
        let (fleet, source) = load_fleet_in_scope(name, scope, &app.workspace).ok()?;
        let session_provider = if app.auto_model {
            app.last_effective_provider_identity
                .clone()
                .unwrap_or_else(|| app.provider_identity_for_persistence().to_string())
        } else {
            app.provider_identity_for_persistence().to_string()
        };
        let session_model = if app.auto_model {
            app.last_effective_model
                .clone()
                .unwrap_or_else(|| "auto".to_string())
        } else {
            app.model.clone()
        };
        let mut view = Self::from_parts(
            fleet,
            scope,
            source,
            app.workspace.clone(),
            config,
            &session_provider,
            &session_model,
        );
        if let Some(member_id) = member_id.map(str::trim).filter(|id| !id.is_empty())
            && let Some(index) = view
                .fleet
                .members
                .iter()
                .position(|member| member.id.eq_ignore_ascii_case(member_id))
        {
            view.selected = index + 1;
        }
        Some(view)
    }

    fn from_parts(
        fleet: FleetFile,
        scope: FleetScope,
        source: PathBuf,
        workspace: PathBuf,
        config: &Config,
        session_provider: &str,
        session_model: &str,
    ) -> Self {
        let routes = build_route_rows(config);
        let mut view = Self {
            fleet,
            scope,
            source,
            workspace,
            selected: 0,
            step: DetailStep::Overview,
            pick_target: PickTarget::Operator,
            routes,
            pick_row: 0,
            rename_mode: false,
            rename_input: String::new(),
            pending_remove: false,
            scout_receipt: None,
            session_provider: session_provider.to_string(),
            session_model: session_model.to_string(),
        };
        view.refresh_scout_receipt();
        view
    }

    /// Recompute the resolved Scout route from the current fleet draft and
    /// session route. Called at open and after every route edit.
    fn refresh_scout_receipt(&mut self) {
        self.scout_receipt = self.fleet.has_scout().then(|| {
            crate::fleet::scout::resolve_scout_route(
                self.fleet.member("scout"),
                &self.session_provider,
                &self.session_model,
            )
            .receipt_line()
        });
    }

    fn row_count(&self) -> usize {
        1 + self.fleet.members.len()
    }

    fn selected_member_idx(&self) -> Option<usize> {
        self.selected.checked_sub(1)
    }

    fn selected_member(&self) -> Option<&FleetMember> {
        self.selected_member_idx()
            .and_then(|idx| self.fleet.members.get(idx))
    }

    fn move_row(&mut self, delta: isize) {
        self.selected = crate::tui::list_nav::wrap_index(self.selected, self.row_count(), delta);
    }

    fn start_rename(&mut self) {
        self.rename_mode = true;
        self.rename_input = self.fleet.name.clone();
    }

    fn commit_rename(&mut self) -> Option<ViewAction> {
        let new_name = self.rename_input.trim().to_string();
        if new_name.is_empty() {
            self.rename_mode = false;
            return Some(ViewAction::None);
        }
        if new_name == self.fleet.name {
            self.rename_mode = false;
            return Some(ViewAction::None);
        }
        // The rename must not collide with a different Fleet of the same slug
        // in this scope (the store refuses that at save).
        let old_name = self.fleet.name.clone();
        self.fleet.name = new_name.clone();
        self.rename_mode = false;
        match save_fleet(&self.fleet, self.scope, &self.workspace) {
            Ok(path) => Some(ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged {
                message: format!(
                    "Renamed Fleet `{old_name}` → `{new_name}` ({}) — wrote {}",
                    self.scope.label(),
                    path.display()
                ),
            })),
            Err(err) => {
                self.fleet.name = old_name;
                Some(ViewAction::Emit(ViewEvent::OpenTextPager {
                    title: "Rename failed".to_string(),
                    content: format!("{err:#}"),
                }))
            }
        }
    }

    /// Enter the route-picker step for the target.
    fn open_route_picker(&mut self, target: PickTarget) {
        self.step = DetailStep::PickRoute;
        self.pick_target = target;
        // Preselect the row matching the current pin (or the inherit row).
        self.pick_row = 0;
        let current: Option<(&str, &str)> = match target {
            PickTarget::Operator => self
                .fleet
                .operator
                .as_ref()
                .map(|op| (op.provider.as_str(), op.model.as_str())),
            PickTarget::Member(idx) => self
                .fleet
                .members
                .get(idx)
                .and_then(|m| m.provider.as_deref().zip(m.model.as_deref())),
        };
        if let Some((provider, model)) = current {
            for (idx, route) in self.routes.iter().enumerate() {
                if route.provider.as_deref() == Some(provider)
                    && route.model.as_deref() == Some(model)
                {
                    self.pick_row = idx;
                    break;
                }
            }
        }
    }

    /// Open this member-focused editor directly on its model route picker.
    /// The roster uses this for its `m` shortcut so model switching does not
    /// require a second, undiscoverable key once the detail view opens.
    pub(crate) fn open_model_picker(&mut self) {
        let target = match self.selected_member_idx() {
            Some(idx) => PickTarget::Member(idx),
            None => PickTarget::Operator,
        };
        self.open_route_picker(target);
    }

    fn apply_route_pick(&mut self) -> Option<ViewAction> {
        let route = self.routes.get(self.pick_row)?;
        match self.pick_target {
            PickTarget::Operator => {
                self.fleet.operator = match (&route.provider, &route.model) {
                    (Some(provider), Some(model)) => Some(FleetOperator {
                        provider: provider.clone(),
                        model: model.clone(),
                        reasoning: self
                            .fleet
                            .operator
                            .as_ref()
                            .and_then(|op| op.reasoning.clone()),
                    }),
                    _ => None,
                };
            }
            PickTarget::Member(idx) => {
                if let Some(member) = self.fleet.members.get_mut(idx) {
                    match (&route.provider, &route.model) {
                        (Some(provider), Some(model)) => {
                            member.provider = Some(provider.clone());
                            member.model = Some(model.clone());
                        }
                        _ => {
                            member.provider = None;
                            member.model = None;
                        }
                    }
                }
            }
        }
        self.step = DetailStep::Overview;
        self.rename_mode = false;
        self.route_edit_needs_refresh();
        Some(ViewAction::None)
    }

    /// Cycle the reasoning level of the selected row through the supported
    /// tiers. The tier list is the provider's documented vocabulary; a tier a
    /// route cannot genuinely express is never offered.
    fn cycle_reasoning(&mut self) {
        let tiers: &[&str] = match self.selected {
            0 => {
                if let Some(op) = &self.fleet.operator {
                    reasoning_tiers_for_provider(&op.provider)
                } else {
                    &[]
                }
            }
            _ => {
                if let Some(member) = self.selected_member()
                    && let Some(provider) = &member.provider
                {
                    reasoning_tiers_for_provider(provider)
                } else {
                    &[]
                }
            }
        };
        if tiers.is_empty() {
            return;
        }
        let slot = match self.selected {
            0 => self.fleet.operator.as_mut().map(|op| &mut op.reasoning),
            _ => self
                .selected_member_idx()
                .and_then(|idx| self.fleet.members.get_mut(idx))
                .map(|m| &mut m.reasoning),
        };
        let Some(slot) = slot else { return };
        let current = slot.as_deref().unwrap_or("inherit");
        let next = match tiers.iter().position(|t| *t == current) {
            Some(pos) => tiers[(pos + 1) % tiers.len()],
            None => tiers[0],
        };
        *slot = if next == "inherit" {
            None
        } else {
            Some(next.to_string())
        };
    }

    /// The scout receipt depends on the member pin and the session route;
    /// reasoning edits don't affect it. Pins refresh it at the next open;
    /// the marker exists so route-edit call sites document that intent.
    fn route_edit_needs_refresh(&mut self) {}

    fn toggle_vision_requirement(&mut self) {
        if let Some(member) = self.selected_member_idx()
            && let Some(member) = self.fleet.members.get_mut(member)
        {
            if member.requires.iter().any(|r| r == "vision") {
                member.requires.retain(|r| r != "vision");
            } else {
                member
                    .requires
                    .push(MemberCapability::Vision.wire_name().to_string());
            }
        }
    }

    fn add_member(&mut self) {
        // First known role not already present.
        let existing: Vec<&str> = self.fleet.members.iter().map(|m| m.id.as_str()).collect();
        let Some(role) = KNOWN_ROLES.iter().find(|r| !existing.contains(r)) else {
            return;
        };
        self.fleet.members.push(FleetMember {
            id: role.to_string(),
            display_name: None,
            role: role.to_string(),
            provider: None,
            model: None,
            reasoning: None,
            instructions: None,
            requires: Vec::new(),
        });
    }

    fn remove_selected_member(&mut self) {
        if let Some(idx) = self.selected_member_idx() {
            self.fleet.members.remove(idx);
            self.pending_remove = false;
            if self.selected >= self.row_count() {
                self.selected = self.row_count().saturating_sub(1);
            }
        }
    }

    fn save(&self) -> Option<ViewAction> {
        match save_fleet(&self.fleet, self.scope, &self.workspace) {
            Ok(path) => Some(ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged {
                message: format!(
                    "Saved Fleet `{}` ({}) — wrote {}",
                    self.fleet.name,
                    self.scope.long_label(),
                    path.display()
                ),
            })),
            Err(err) => Some(ViewAction::Emit(ViewEvent::OpenTextPager {
                title: "Save failed".to_string(),
                content: format!(
                    "Nothing was written.\n\n{err:#}\n\nFix the issue and save again."
                ),
            })),
        }
    }

    fn copy_to_other_scope(&self) -> Option<ViewAction> {
        let target = self.scope.toggled();
        match save_fleet(&self.fleet, target, &self.workspace) {
            Ok(path) => Some(ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged {
                message: format!(
                    "Copied Fleet `{}` to {} scope — wrote {}",
                    self.fleet.name,
                    target.label(),
                    path.display()
                ),
            })),
            Err(err) => Some(ViewAction::Emit(ViewEvent::OpenTextPager {
                title: "Copy failed".to_string(),
                content: format!("{err:#}"),
            })),
        }
    }

    fn select_scope(&self, scope: FleetScope) -> Option<ViewAction> {
        match set_selected(&self.fleet.name, scope, &self.workspace) {
            Ok(path) => Some(ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged {
                message: format!(
                    "Selected Fleet `{}` as {} default — wrote {}",
                    self.fleet.name,
                    scope.label(),
                    path.display()
                ),
            })),
            Err(err) => Some(ViewAction::Emit(ViewEvent::OpenTextPager {
                title: "Selection failed".to_string(),
                content: format!("{err:#}"),
            })),
        }
    }

    fn footer_hints(&self) -> Vec<ActionHint> {
        match self.step {
            DetailStep::PickRoute => vec![
                ActionHint::new("↑/↓", "move"),
                ActionHint::new("Enter", "pick"),
                ActionHint::new("Esc", "back"),
            ],
            DetailStep::Overview => {
                let mut hints = vec![
                    ActionHint::new("↑/↓", "move"),
                    ActionHint::new("o", "Coordinator model"),
                    ActionHint::new("e", "member model"),
                    ActionHint::new("t", "reasoning"),
                    ActionHint::new("r", "rename"),
                    ActionHint::new("s", "save"),
                    ActionHint::new("c", "copy destination"),
                    ActionHint::new("u/w", "select"),
                ];
                if self.selected > 0 {
                    hints.push(ActionHint::new("v", "vision"));
                    hints.push(ActionHint::new("a/d", "add/remove"));
                }
                hints.push(ActionHint::new("Esc", "back"));
                hints
            }
        }
    }
}

impl ModalView for FleetDetailView {
    fn kind(&self) -> ModalKind {
        ModalKind::FleetDetail
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.step {
            DetailStep::PickRoute => match key.code {
                KeyCode::Esc => {
                    self.step = DetailStep::Overview;
                    ViewAction::None
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if !self.routes.is_empty() {
                        self.pick_row =
                            crate::tui::list_nav::wrap_index(self.pick_row, self.routes.len(), -1);
                    }
                    ViewAction::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !self.routes.is_empty() {
                        self.pick_row =
                            crate::tui::list_nav::wrap_index(self.pick_row, self.routes.len(), 1);
                    }
                    ViewAction::None
                }
                KeyCode::Enter => self.apply_route_pick().unwrap_or(ViewAction::None),
                _ => ViewAction::None,
            },
            DetailStep::Overview => {
                if self.rename_mode {
                    return match key.code {
                        KeyCode::Enter => self.commit_rename().unwrap_or(ViewAction::None),
                        KeyCode::Esc => {
                            self.rename_mode = false;
                            ViewAction::None
                        }
                        KeyCode::Char(c) => {
                            self.rename_input.push(c);
                            ViewAction::None
                        }
                        KeyCode::Backspace => {
                            self.rename_input.pop();
                            ViewAction::None
                        }
                        _ => ViewAction::None,
                    };
                }
                if self.pending_remove {
                    return match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            self.remove_selected_member();
                            ViewAction::None
                        }
                        KeyCode::Char('n') | KeyCode::Esc => {
                            self.pending_remove = false;
                            ViewAction::None
                        }
                        _ => ViewAction::None,
                    };
                }
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.move_row(-1);
                        ViewAction::None
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.move_row(1);
                        ViewAction::None
                    }
                    KeyCode::Char('o') => {
                        self.open_route_picker(PickTarget::Operator);
                        ViewAction::None
                    }
                    KeyCode::Char('e') => {
                        if let Some(idx) = self.selected_member_idx() {
                            self.open_route_picker(PickTarget::Member(idx));
                        }
                        ViewAction::None
                    }
                    KeyCode::Char('t') => {
                        self.cycle_reasoning();
                        ViewAction::None
                    }
                    KeyCode::Char('v') => {
                        self.toggle_vision_requirement();
                        ViewAction::None
                    }
                    KeyCode::Char('a') => {
                        self.add_member();
                        ViewAction::None
                    }
                    KeyCode::Char('d') if self.selected > 0 => {
                        self.pending_remove = true;
                        ViewAction::None
                    }
                    KeyCode::Char('r') => {
                        self.start_rename();
                        ViewAction::None
                    }
                    KeyCode::Char('s') => self.save().unwrap_or(ViewAction::None),
                    KeyCode::Char('c') => self.copy_to_other_scope().unwrap_or(ViewAction::None),
                    KeyCode::Char('u') => self
                        .select_scope(FleetScope::Personal)
                        .unwrap_or(ViewAction::None),
                    KeyCode::Char('w') => self
                        .select_scope(FleetScope::Workspace)
                        .unwrap_or(ViewAction::None),
                    _ => ViewAction::None,
                }
            }
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        Block::default()
            .style(Style::default().bg(palette::WHALE_BG))
            .render(area, buf);

        let hints = self.footer_hints();
        let content = render_modal_footer(area, buf, &hints);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(1)])
            .split(content);

        // Header.
        let title = if self.rename_mode {
            format!("Renaming: {}▏", self.rename_input)
        } else {
            format!(
                "Fleet `{}` · {} scope · {}",
                self.fleet.name,
                self.scope.label(),
                self.source.display()
            )
        };
        let mut header = vec![
            Line::from(vec![
                Span::styled(
                    "─ Fleet ",
                    Style::default().fg(palette::WHALE_ACTION).bold(),
                ),
                Span::styled(title, Style::default().fg(palette::TEXT_SECONDARY)),
            ]),
            Line::from(""),
        ];
        let operator_line = match &self.fleet.operator {
            Some(op) => format!("  Coordinator: {}/{}", op.provider, op.model),
            None => "  Coordinator: uses the session's model".to_string(),
        };
        header.push(Line::from(Span::styled(
            operator_line,
            Style::default().fg(palette::TEXT_DIM),
        )));
        if let Some(scout) = &self.scout_receipt {
            header.push(Line::from(Span::styled(
                format!("  scout → {scout}"),
                Style::default().fg(palette::TEXT_DIM),
            )));
        }
        Paragraph::new(header)
            .wrap(Wrap { trim: false })
            .render(chunks[0], buf);

        match self.step {
            DetailStep::Overview => self.render_overview(chunks[1], buf),
            DetailStep::PickRoute => self.render_pick_route(chunks[1], buf),
        }
    }
}

impl FleetDetailView {
    fn render_overview(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rows_visible = usize::from(area.height).max(1);
        let scroll = self.selected.saturating_sub(rows_visible.saturating_sub(1));
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Operator row.
        if self.selected == 0 {
            let selected = self.selected == 0;
            let base = if selected {
                Style::default().fg(palette::WHALE_ACTION).bold()
            } else {
                Style::default().fg(palette::TEXT_SECONDARY)
            };
            let operator_text = match &self.fleet.operator {
                Some(op) => format!("{}/{}", op.provider, op.model),
                None => "inherits session route".to_string(),
            };
            let reasoning = self
                .fleet
                .operator
                .as_ref()
                .and_then(|op| op.reasoning.as_deref())
                .unwrap_or("inherit");
            lines.push(Line::from(vec![
                Span::styled(if selected { "» " } else { "  " }, base),
                Span::styled("operator", base),
                Span::styled("  ", Style::default()),
                Span::styled(operator_text, Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(
                    format!(" · reasoning: {reasoning}"),
                    Style::default().fg(palette::TEXT_DIM),
                ),
            ]));
        }

        for (idx, member) in self.fleet.members.iter().enumerate() {
            let row = 1 + idx;
            if row < scroll || row >= scroll + rows_visible {
                continue;
            }
            let selected = row == self.selected;
            let base = if selected {
                Style::default().fg(palette::WHALE_ACTION).bold()
            } else {
                Style::default().fg(palette::TEXT_SECONDARY)
            };
            let route = match (&member.provider, &member.model) {
                (Some(p), Some(m)) => format!("model {p}/{m}"),
                _ => "same model as this session".to_string(),
            };
            let reasoning = member.reasoning.as_deref().unwrap_or("inherit");
            let vision = if member.requires.iter().any(|r| r == "vision") {
                " · vision"
            } else {
                ""
            };
            if self.pending_remove && selected {
                lines.push(Line::from(vec![Span::styled(
                    format!("  Remove member `{}`? y/n", member.id),
                    Style::default().fg(palette::WHALE_ERROR),
                )]));
            } else {
                let role = member.role.trim();
                let role = if role.is_empty() {
                    member.id.as_str()
                } else {
                    role
                };
                let role = public_role_label(role);
                let member_label = member
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(&member.id))
                    .map_or_else(
                        || member.id.clone(),
                        |name| format!("{name} ({})", member.id),
                    );
                lines.push(Line::from(vec![
                    Span::styled(if selected { "» " } else { "  " }, base),
                    Span::styled(member_label, base),
                    Span::styled(
                        format!(" · role {role}"),
                        Style::default().fg(palette::TEXT_SECONDARY),
                    ),
                    Span::styled("  ", Style::default()),
                    Span::styled(route, Style::default().fg(palette::TEXT_MUTED)),
                    Span::styled(
                        format!(" · reasoning: {reasoning}{vision}"),
                        Style::default().fg(palette::TEXT_DIM),
                    ),
                ]));
            }
        }
        Paragraph::new(ratatui::text::Text::from(lines)).render(area, buf);
    }

    fn render_pick_route(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let rows_visible = usize::from(area.height).max(1);
        let pick_scroll = self.pick_row.saturating_sub(rows_visible.saturating_sub(1));
        let target_label = match self.pick_target {
            PickTarget::Operator => "operator",
            PickTarget::Member(idx) => self
                .fleet
                .members
                .get(idx)
                .map(|m| m.id.as_str())
                .unwrap_or("member"),
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!("  Model for {target_label} — Enter picks, Esc back."),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(""));
        for (idx, route) in self.routes.iter().enumerate() {
            if idx < pick_scroll || idx >= pick_scroll + rows_visible {
                continue;
            }
            let selected = idx == self.pick_row;
            let base = if selected {
                Style::default().fg(palette::WHALE_ACTION).bold()
            } else {
                Style::default().fg(palette::TEXT_SECONDARY)
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "» " } else { "  " }, base),
                Span::styled(route.label.clone(), base),
                Span::styled("  ", Style::default()),
                Span::styled(
                    route.summary.clone(),
                    Style::default().fg(palette::TEXT_DIM),
                ),
            ]));
        }
        Paragraph::new(ratatui::text::Text::from(lines)).render(area, buf);
    }
}

/// Build the model-picker rows: "same as session" first, then every
/// concrete model across configured providers, with its readiness label —
/// the same list the fleet setup wizard's Model step shows.
fn build_route_rows(config: &Config) -> Vec<RouteRow> {
    let mut rows = vec![RouteRow {
        label: "same as session".to_string(),
        summary: String::new(),
        provider: None,
        model: None,
    }];
    let health = crate::provider_readiness::ProviderReadinessSnapshot::default();
    let active = config
        .provider
        .as_deref()
        .and_then(ApiProvider::parse)
        .unwrap_or(ApiProvider::Deepseek);
    let routes = super::fleet_setup::cross_provider_model_routes(config, active, &health);
    for (provider, model, readiness) in routes {
        let provider_label = crate::tui::views::fleet_setup::provider_display_label(&provider);
        let readiness_label = readiness
            .blocked_reason()
            .map(|r| r.into_owned())
            .unwrap_or_else(|| readiness.label().into_owned());
        rows.push(RouteRow {
            label: format!("{provider_label}/{model}"),
            summary: readiness_label,
            provider: Some(provider),
            model: Some(model),
        });
    }
    rows
}

/// Reasoning tiers a route may genuinely express, keyed by provider class.
/// A tier a provider cannot wire is never offered (no `max` where a route
/// has none). `inherit` is always first.
fn reasoning_tiers_for_provider(provider: &str) -> &'static [&'static str] {
    // Tiers are keyed by the provider's exact id from the catalog, never
    // guessed from a display name. A tier a route cannot genuinely express
    // is not offered.
    match provider
        .to_ascii_lowercase()
        .replace(['_', '-'], "")
        .as_str()
    {
        "deepseek" | "deepseekcn" | "deepseekanthropic" => {
            &["inherit", "off", "low", "high", "max"]
        }
        "moonshot" | "kimi" | "kimicode" => &["inherit", "off", "low", "medium", "high"],
        "openaicodex" => &["inherit", "off", "minimal", "high"],
        _ => &["inherit", "off", "low", "medium", "high"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::fleet::store::FleetFile;
    use crate::tui::app::{App, TuiOptions};

    fn app_in(workspace: PathBuf) -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(workspace.clone())
        };
        let mut app = App::new(options, &Config::default());
        app.workspace = workspace;
        app
    }

    fn sample_fleet(name: &str) -> FleetFile {
        let mut fleet = FleetFile::new(name.to_string(), None).unwrap();
        fleet.operator = Some(FleetOperator {
            provider: "deepseek".to_string(),
            model: "deepseek-v4-flash".to_string(),
            reasoning: None,
        });
        fleet.members.push(FleetMember {
            id: "scout".to_string(),
            display_name: Some("Flash Scout".to_string()),
            role: "scout".to_string(),
            provider: None,
            model: None,
            reasoning: None,
            instructions: None,
            requires: Vec::new(),
        });
        fleet
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn open_loads_the_fleet_by_name_and_scope() {
        let ws = tempfile::TempDir::new().unwrap();
        let fleet = sample_fleet("DeepSeek Flash");
        let path = save_fleet(&fleet, FleetScope::Workspace, ws.path()).unwrap();

        let mut app = app_in(ws.path().to_path_buf());
        let view = FleetDetailView::open(
            &app,
            &Config::default(),
            "DeepSeek Flash",
            FleetScope::Workspace,
        )
        .expect("open");
        assert_eq!(view.fleet.name, "DeepSeek Flash");
        assert_eq!(view.scope, FleetScope::Workspace);
        assert_eq!(view.source, path);
        assert_eq!(view.row_count(), 2); // operator + scout

        let mut duplicate_roles = sample_fleet("Duplicate Roles");
        duplicate_roles.members.push(FleetMember {
            id: "fast-scout".to_string(),
            display_name: Some("Fast Scout".to_string()),
            role: "scout".to_string(),
            provider: None,
            model: None,
            reasoning: None,
            instructions: None,
            requires: Vec::new(),
        });
        save_fleet(&duplicate_roles, FleetScope::Workspace, ws.path())
            .expect("save duplicate-role Fleet");
        let focused = FleetDetailView::open_for_member(
            &app,
            &Config::default(),
            "Duplicate Roles",
            FleetScope::Workspace,
            Some("fast-scout"),
        )
        .expect("open focused member");
        assert_eq!(focused.selected, 2);
        assert_eq!(
            focused.selected_member().map(|member| member.id.as_str()),
            Some("fast-scout")
        );

        // A missing fleet fails to open (the host shows the error receipt).
        app.workspace = ws.path().to_path_buf();
        assert!(
            FleetDetailView::open(&app, &Config::default(), "Nope", FleetScope::Workspace)
                .is_none()
        );
    }

    #[test]
    fn rename_commits_and_names_the_receipt() {
        let ws = tempfile::TempDir::new().unwrap();
        let fleet = sample_fleet("Old Name");
        save_fleet(&fleet, FleetScope::Workspace, ws.path()).unwrap();

        let mut view = FleetDetailView::open(
            &app_in(ws.path().to_path_buf()),
            &Config::default(),
            "Old Name",
            FleetScope::Workspace,
        )
        .expect("open");

        view.handle_key(key(KeyCode::Char('r')));
        assert!(view.rename_mode);
        // The input starts filled with the current name; clear it, then type.
        for _ in "Old Name".chars() {
            view.handle_key(key(KeyCode::Backspace));
        }
        for ch in "New Name".chars() {
            view.handle_key(key(KeyCode::Char(ch)));
        }
        let action = view.handle_key(key(KeyCode::Enter));
        let ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged { message }) = action else {
            panic!("expected FleetStoreChanged, got {action:?}");
        };
        assert!(
            message.contains("Renamed Fleet `Old Name` → `New Name`"),
            "{message}"
        );
        // The on-disk file now carries the new name.
        let (loaded, _) =
            crate::fleet::store::load_fleet_in_scope("New Name", FleetScope::Workspace, ws.path())
                .expect("reload");
        assert_eq!(loaded.name, "New Name");
    }

    #[test]
    fn operator_route_pick_pins_and_inherit_clears() {
        let ws = tempfile::TempDir::new().unwrap();
        let fleet = sample_fleet("Fleet A");
        save_fleet(&fleet, FleetScope::Workspace, ws.path()).unwrap();

        let mut view = FleetDetailView::open(
            &app_in(ws.path().to_path_buf()),
            &Config::default(),
            "Fleet A",
            FleetScope::Workspace,
        )
        .expect("open");
        assert!(view.fleet.operator.is_some());

        // Enter the operator picker, choose the inherit row.
        view.handle_key(key(KeyCode::Char('o')));
        assert_eq!(view.step, DetailStep::PickRoute);
        view.pick_row = 0;
        view.handle_key(key(KeyCode::Enter));
        assert_eq!(view.step, DetailStep::Overview);
        assert!(view.fleet.operator.is_none(), "inherit row clears the pin");

        // Re-enter and pick the first concrete route row.
        view.handle_key(key(KeyCode::Char('o')));
        view.pick_row = 1;
        view.handle_key(key(KeyCode::Enter));
        let op = view.fleet.operator.as_ref().expect("pinned operator");
        assert!(!op.provider.is_empty() && !op.model.is_empty());
    }

    #[test]
    fn member_edit_pins_route_and_toggles_vision() {
        let ws = tempfile::TempDir::new().unwrap();
        let fleet = sample_fleet("Fleet B");
        save_fleet(&fleet, FleetScope::Workspace, ws.path()).unwrap();

        let mut view = FleetDetailView::open(
            &app_in(ws.path().to_path_buf()),
            &Config::default(),
            "Fleet B",
            FleetScope::Workspace,
        )
        .expect("open");

        // Select the scout member (row 1) and pin the first concrete route.
        view.selected = 1;
        view.handle_key(key(KeyCode::Char('e')));
        assert_eq!(view.step, DetailStep::PickRoute);
        view.pick_row = 1;
        view.handle_key(key(KeyCode::Enter));
        let member = view.fleet.member("scout").expect("scout");
        assert!(
            member.provider.is_some() && member.model.is_some(),
            "scout must be pinned: {member:?}"
        );

        // Vision requirement toggles on and off.
        view.handle_key(key(KeyCode::Char('v')));
        assert!(
            view.fleet
                .member("scout")
                .unwrap()
                .requires
                .contains(&"vision".to_string())
        );
        view.handle_key(key(KeyCode::Char('v')));
        assert!(view.fleet.member("scout").unwrap().requires.is_empty());
    }

    #[test]
    fn save_writes_the_file_and_receipt_names_the_path() {
        let ws = tempfile::TempDir::new().unwrap();
        let fleet = sample_fleet("Fleet C");
        save_fleet(&fleet, FleetScope::Workspace, ws.path()).unwrap();

        let mut view = FleetDetailView::open(
            &app_in(ws.path().to_path_buf()),
            &Config::default(),
            "Fleet C",
            FleetScope::Workspace,
        )
        .expect("open");

        // Change the operator model, then save.
        view.fleet.operator = Some(FleetOperator {
            provider: "deepseek".to_string(),
            model: "deepseek-v4-pro".to_string(),
            reasoning: Some("high".to_string()),
        });
        let action = view.handle_key(key(KeyCode::Char('s')));
        let ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged { message }) = action else {
            panic!("expected FleetStoreChanged, got {action:?}");
        };
        assert!(message.contains("Saved Fleet `Fleet C`"), "{message}");
        // The receipt names the path as this platform writes it, so build the
        // expected tail the same way instead of hard-coding `/` — on Windows
        // `Path::display` renders the separators as `\`.
        let expected_tail = std::path::Path::new(".codewhale")
            .join("fleets")
            .join("fleet-c.toml")
            .display()
            .to_string();
        assert!(message.contains(&expected_tail), "{message}");

        let (loaded, _) =
            crate::fleet::store::load_fleet_in_scope("Fleet C", FleetScope::Workspace, ws.path())
                .expect("reload");
        let op = loaded.operator.expect("operator");
        assert_eq!(op.model, "deepseek-v4-pro");
        assert_eq!(op.reasoning.as_deref(), Some("high"));
    }

    #[test]
    fn add_member_uses_an_unused_known_role() {
        let ws = tempfile::TempDir::new().unwrap();
        let fleet = sample_fleet("Fleet D");
        save_fleet(&fleet, FleetScope::Workspace, ws.path()).unwrap();

        let mut view = FleetDetailView::open(
            &app_in(ws.path().to_path_buf()),
            &Config::default(),
            "Fleet D",
            FleetScope::Workspace,
        )
        .expect("open");

        view.handle_key(key(KeyCode::Char('a')));
        let ids: Vec<&str> = view.fleet.members.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["scout", "explore"]);

        // Remove the new member with the confirmed delete flow.
        view.selected = 2;
        view.handle_key(key(KeyCode::Char('d')));
        view.handle_key(key(KeyCode::Char('y')));
        assert_eq!(view.fleet.members.len(), 1);
    }
}
