//! `/fleet` roster — the barracks view of the saved agent party.
//!
//! The roster view is the read side of the Fleet profile surface. The first
//! row is the **operator** — the live session route (your main model): when a
//! user picks a session model they are picking the operator, and the roster
//! is that operator's team. Below it the merged [`FleetRoster`] (built-in <
//! `[fleet.profiles]` config < `.codewhale/agents/*.toml` project members)
//! renders as a scrollable list with a detail pane for the selected row. The
//! view never writes anything; `s` / Enter on a member hands off to the
//! `/fleet setup` wizard for authoring and overrides (the operator row is
//! display-only — its route changes via `/model` or `/provider`).
//!
//! NOTE: like `fleet_setup.rs`, the copy below is intentionally English for
//! now (#3167 reworks Fleet UI localization); the command entry
//! (`CmdFleetDescription`) is already localized.

use std::cell::RefCell;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::fleet::profile::AgentProfile;
use crate::fleet::roster::{FleetRoster, ProfileOrigin};
use crate::fleet::worker_runtime::roster_member_agent_type;
use crate::palette;
use crate::tui::app::App;
use crate::tui::hit_region::HitMap;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer, render_modal_surface, truncate_view_text,
};
use crate::worker_profile::{ShellPolicy, WorkerRuntimeProfile};

/// The live session route — the operator the roster works for. Read once at
/// open, the same way [`super::fleet_setup::FleetSetupSnapshot`] snapshots it.
#[derive(Debug, Clone)]
struct OperatorInfo {
    provider: String,
    model: String,
    reasoning: String,
}

impl OperatorInfo {
    fn from_app(app: &App) -> Self {
        let model = if app.auto_model {
            app.last_effective_model
                .as_deref()
                .map(|effective| format!("auto -> {effective}"))
                .unwrap_or_else(|| "auto".to_string())
        } else {
            app.model.clone()
        };
        Self {
            provider: app.api_provider.display_name().to_string(),
            model,
            reasoning: app.reasoning_effort_display_label(),
        }
    }
}

pub struct FleetRosterView {
    operator: OperatorInfo,
    members: Vec<AgentProfile>,
    /// Selected row: 0 is the pinned operator row, members follow at 1..
    selected: usize,
    detail_scroll: usize,
    /// Roster row hit regions from the most recent paint.
    list_hit_map: RefCell<HitMap>,
}

impl FleetRosterView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        Self::from_parts(
            OperatorInfo::from_app(app),
            FleetRoster::load(&config.fleet_config(), &app.workspace),
        )
    }

    fn from_parts(operator: OperatorInfo, roster: FleetRoster) -> Self {
        Self {
            operator,
            // The operator is pinned as its own row 0 (the live session route),
            // so exclude the built-in "operator" profile from the dispatchable
            // member list to avoid rendering it twice (#dogfood 0.8.67). The
            // engine's FleetRoster is untouched, so role/dispatch semantics are
            // unchanged; only this view drops the duplicate.
            members: roster
                .members()
                .iter()
                .filter(|m| !m.id.trim().eq_ignore_ascii_case("operator"))
                .cloned()
                .collect(),
            selected: 0,
            detail_scroll: 0,
            list_hit_map: RefCell::new(HitMap::new()),
        }
    }

    /// Total selectable rows: the operator plus every roster member.
    fn row_count(&self) -> usize {
        1 + self.members.len()
    }

    fn operator_selected(&self) -> bool {
        self.selected == 0
    }

    fn selected_member(&self) -> Option<&AgentProfile> {
        self.selected.checked_sub(1).and_then(|idx| {
            self.members
                .get(idx.min(self.members.len().saturating_sub(1)))
        })
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.detail_scroll = 0;
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1).min(self.row_count().saturating_sub(1));
        self.detail_scroll = 0;
    }

    fn focus_row_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.list_hit_map.borrow().row_at(column, row) else {
            return false;
        };
        if index >= self.row_count() {
            return false;
        }
        if self.selected != index {
            self.selected = index;
            self.detail_scroll = 0;
        }
        true
    }

    fn open_selected_member(&self) -> ViewAction {
        if self.operator_selected() {
            // The operator is not a wizard-authored profile; its route changes
            // via /model or /provider (the detail pane says so).
            ViewAction::None
        } else {
            // Hand off to the authoring wizard; the roster itself never writes
            // anything.
            ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenSetupRequested)
        }
    }

    fn footer_hints(&self) -> Vec<ActionHint> {
        vec![
            ActionHint::new("↑/↓", "select"),
            ActionHint::new("s/Enter", "setup"),
            ActionHint::new("click", "open row"),
            ActionHint::new("PgUp/PgDn", "scroll detail"),
            ActionHint::new("Esc", "close"),
        ]
    }
}

impl ModalView for FleetRosterView {
    fn kind(&self) -> ModalKind {
        ModalKind::FleetRoster
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
            KeyCode::Enter | KeyCode::Char('s') => self.open_selected_member(),
            KeyCode::Home => {
                self.detail_scroll = 0;
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
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.move_up();
            }
            MouseEventKind::ScrollDown => {
                self.move_down();
            }
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                let _ = self.focus_row_at(mouse.column, mouse.row);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.focus_row_at(mouse.column, mouse.row) {
                    return self.open_selected_member();
                }
            }
            _ => {}
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_area = centered_modal_area(area, 100, 30, 60, 16);
        render_modal_surface(area, popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Fleet — workers and orchestration ",
                Style::default()
                    .fg(palette::WHALE_ACCENT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(
                Line::from(Span::styled(
                    format!(" {} members ", self.members.len()),
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
        let content = render_modal_footer(inner, buf, &hints);

        // Header (framing) above the roster body.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(content);
        let header = vec![
            Line::from(Span::styled(
                "Worker readiness and status (Operate posture)",
                Style::default().fg(palette::WHALE_INFO).bold(),
            )),
            Line::from(Span::styled(
                "Built-ins < config [fleet.profiles] < project .codewhale/agents. s edits.",
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ];
        Paragraph::new(header)
            .wrap(Wrap { trim: true })
            .render(chunks[0], buf);

        self.render_body(chunks[1], buf);
    }
}

impl FleetRosterView {
    fn render_body(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Two columns when there is room, stacked otherwise — same responsive
        // shape as the setup wizard's choice step so nothing truncates at
        // 80x24.
        let (list_area, detail_area) = if area.width >= 56 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(28), Constraint::Min(20)])
                .split(area);
            (cols[0], cols[1])
        } else {
            let list_height =
                (self.row_count() as u16 + 1).min(area.height.saturating_sub(1).max(1));
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(list_height), Constraint::Min(1)])
                .split(area);
            (rows[0], rows[1])
        };

        // Row list: the pinned operator first, then one row per member,
        // scrolled so the selection stays visible when the party outgrows
        // the pane.
        let visible_rows = usize::from(list_area.height).max(1);
        let first = self
            .selected
            .saturating_sub(visible_rows.saturating_sub(1))
            .min(
                self.row_count()
                    .saturating_sub(visible_rows.min(self.row_count())),
            );
        let list_width = usize::from(list_area.width);
        let mut list_lines: Vec<Line> = Vec::with_capacity(visible_rows);
        let mut hit_map = HitMap::new();
        for (line_offset, idx) in (first..(first + visible_rows).min(self.row_count())).enumerate()
        {
            let y = list_area.y.saturating_add(line_offset as u16);
            hit_map.push_row(idx, list_area.x, y, list_area.width);
            let is_selected = idx == self.selected;
            let pointer = if is_selected { "> " } else { "  " };
            let (text, base_style) = if idx == 0 {
                (
                    format!("{pointer}operator  [route]"),
                    Style::default()
                        .fg(palette::WHALE_ACCENT_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                let member = &self.members[idx - 1];
                (
                    format!("{pointer}{}  [{}]  [open]", member.id, member.origin),
                    Style::default().fg(member_hue(member)),
                )
            };
            let style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                base_style
            };
            let text = truncate_view_text(&text, list_width);
            let padded = format!("{text:<list_width$}");
            list_lines.push(Line::from(Span::styled(padded, style)));
        }
        *self.list_hit_map.borrow_mut() = hit_map;
        Paragraph::new(list_lines).render(list_area, buf);

        // Detail pane for the selected row.
        let lines = if self.operator_selected() {
            operator_detail_lines(&self.operator)
        } else if let Some(member) = self.selected_member() {
            member_detail_lines(member)
        } else {
            vec![Line::from(Span::styled(
                "Roster is empty.",
                Style::default().fg(palette::TEXT_MUTED),
            ))]
        };

        // Same wrapped-row scroll bound as the setup review step: count
        // visual rows so the tail stays reachable.
        let wrap_width = usize::from(detail_area.width).max(1);
        let visual_rows: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(wrap_width).max(1))
            .sum();
        let max_scroll = visual_rows.saturating_sub(usize::from(detail_area.height).max(1));
        let scroll = self.detail_scroll.min(max_scroll);
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .scroll((scroll as u16, 0))
            .render(detail_area, buf);
    }
}

/// Shared field renderer for the detail pane.
fn detail_field(lines: &mut Vec<Line<'static>>, label: &str, body: String) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(palette::WHALE_INFO).bold(),
    )));
    lines.push(Line::from(Span::styled(
        body,
        Style::default().fg(palette::TEXT_PRIMARY),
    )));
    lines.push(Line::from(""));
}

/// Detail pane for the pinned operator row: the live session route, plus the
/// product truth that the roster is this operator's team.
fn operator_detail_lines(operator: &OperatorInfo) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    detail_field(&mut lines, "Member", "operator (session route)".to_string());
    detail_field(&mut lines, "Origin", "session".to_string());
    detail_field(&mut lines, "Posture", "full session authority".to_string());
    detail_field(&mut lines, "Provider", operator.provider.clone());
    detail_field(&mut lines, "Model", operator.model.clone());
    detail_field(&mut lines, "Reasoning", operator.reasoning.clone());
    detail_field(
        &mut lines,
        "Description",
        "Your main session model is the operator. Fleet members are the workers it dispatches \
         — via `agent` profile spawns and Workflow task({profile}). Change the operator's \
         route via /model or /provider."
            .to_string(),
    );
    lines
}

/// The resolved worker posture for a roster member: what the runtime would
/// actually grant when this member is dispatched (role posture, not the
/// profile's requested permissions).
fn member_posture(member: &AgentProfile) -> String {
    let agent_type = roster_member_agent_type(member);
    let runtime = WorkerRuntimeProfile::for_role(agent_type.clone());
    let write = if runtime.permissions.write {
        "write"
    } else {
        "read-only"
    };
    let shell = match runtime.shell {
        ShellPolicy::None => "shell none",
        ShellPolicy::ReadOnly => "shell read-only",
        ShellPolicy::Full => "shell full",
    };
    format!("{} worker · {write} · {shell}", agent_type.as_str())
}

fn member_hue(member: &AgentProfile) -> Color {
    let agent_type = roster_member_agent_type(member);
    match agent_type.as_str() {
        "explore" => palette::WHALE_INFO,
        "plan" => palette::MODE_PLAN,
        "implementer" => palette::STATUS_SUCCESS,
        "verifier" => palette::ACCENT_SECONDARY,
        "review" => palette::WHALE_ACCENT_PRIMARY,
        "general" => palette::MODE_OPERATE,
        _ => palette::TEXT_ACCENT,
    }
}

/// The routing truth for a member: explicit model pin, else route preset, else
/// same-route inheritance. `[subagents]` overrides still win at dispatch.
fn member_routing(member: &AgentProfile) -> String {
    if let Some(model) = member
        .profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return format!("model {model} (pinned)");
    }
    match member.profile.loadout.as_str() {
        "inherit" => "inherit session route".to_string(),
        loadout => format!("route preset {loadout}"),
    }
}

fn member_detail_lines(member: &AgentProfile) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    let name = match member.display_name.as_deref().map(str::trim) {
        Some(display_name) if !display_name.is_empty() && display_name != member.id => {
            format!("{display_name} ({})", member.id)
        }
        _ => member.id.clone(),
    };
    detail_field(&mut lines, "Member", name);
    detail_field(
        &mut lines,
        "Origin",
        match member.origin {
            ProfileOrigin::BuiltIn => "built-in (default party)".to_string(),
            _ => format!("{} · {}", member.origin, member.source.display()),
        },
    );
    detail_field(&mut lines, "Slot", member.profile.slot.as_str().to_string());
    detail_field(&mut lines, "Posture", member_posture(member));
    detail_field(&mut lines, "Routing", member_routing(member));

    let delegation = &member.profile.delegation;
    if delegation.max_spawn_depth.is_some() || delegation.max_concurrency.is_some() {
        let mut bounds: Vec<String> = Vec::new();
        if let Some(depth) = delegation.max_spawn_depth {
            bounds.push(format!("spawn depth {depth}"));
        }
        if let Some(concurrency) = delegation.max_concurrency {
            bounds.push(format!("concurrency {concurrency}"));
        }
        detail_field(&mut lines, "Delegation", bounds.join(" · "));
    }

    detail_field(
        &mut lines,
        "Instructions",
        if member.profile.role.instructions.is_some() {
            match member.origin {
                ProfileOrigin::Workspace => {
                    format!("custom overlay ({})", member.source.display())
                }
                _ => "custom overlay".to_string(),
            }
        } else {
            "none (role posture only)".to_string()
        },
    );

    if let Some(description) = member
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        detail_field(&mut lines, "Description", description.to_string());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::hit_region::HitTarget;
    use crate::tui::views::ViewStack;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use unicode_width::UnicodeWidthStr;

    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn operator() -> OperatorInfo {
        OperatorInfo {
            provider: "DeepSeek".to_string(),
            model: "deepseek-v4-pro".to_string(),
            reasoning: "Auto".to_string(),
        }
    }

    fn built_in_view() -> FleetRosterView {
        FleetRosterView::from_parts(operator(), FleetRoster::built_ins_only())
    }

    fn view_with_overrides() -> FleetRosterView {
        let mut members = FleetRoster::built_ins_only()
            .members()
            .iter()
            .filter(|m| !m.id.trim().eq_ignore_ascii_case("operator"))
            .cloned()
            .collect::<Vec<_>>();
        // A project override of the built-in reviewer with a pinned model and
        // an instruction overlay.
        if let Some(reviewer) = members.iter_mut().find(|m| m.id == "reviewer") {
            reviewer.origin = ProfileOrigin::Workspace;
            reviewer.source = PathBuf::from(".codewhale/agents/reviewer.toml");
            reviewer.profile.model = Some("glm-5.2".to_string());
            reviewer.profile.role.instructions = Some("Review hard.".to_string());
            reviewer.profile.delegation.max_spawn_depth = Some(1);
        }
        FleetRosterView {
            operator: operator(),
            members,
            selected: 0,
            detail_scroll: 0,
            list_hit_map: RefCell::new(HitMap::new()),
        }
    }

    fn render_through_stack(make: impl Fn() -> FleetRosterView, w: u16, h: u16) -> Vec<String> {
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(area);
        for y in 0..h {
            for x in 0..w {
                buf[(x, y)].set_symbol("X");
            }
        }
        let mut stack = ViewStack::new();
        stack.push(make());
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
    fn operator_row_is_pinned_first_with_the_session_model() {
        let rows = render_through_stack(built_in_view, 100, 30);
        let text = rows.join("\n");
        // The operator row leads the list and the detail pane (row 0 is
        // selected on open) shows the live session route.
        let operator_row = rows
            .iter()
            .position(|row| row.contains("operator"))
            .expect("operator row rendered");
        let first_member_row = rows
            .iter()
            .position(|row| row.contains("manager"))
            .expect("first member rendered");
        assert!(
            operator_row < first_member_row,
            "operator must render above the first member"
        );
        assert!(text.contains("> operator"), "operator selected on open");
        assert!(text.contains("deepseek-v4-pro"), "session model shown");
        assert!(text.contains("full session authority"), "{text}");
    }

    #[test]
    fn arrows_move_selection_and_clamp() {
        let mut view = built_in_view();
        assert_eq!(view.selected, 0);
        view.handle_key(key(KeyCode::Up));
        assert_eq!(view.selected, 0, "clamps at the operator row");

        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.selected, 1, "first member follows the operator");
        for _ in 0..50 {
            view.handle_key(key(KeyCode::Down));
        }
        assert_eq!(
            view.selected,
            view.members.len(),
            "clamps at the last member"
        );
    }

    #[test]
    fn selection_change_resets_detail_scroll() {
        let mut view = built_in_view();
        view.handle_key(key(KeyCode::PageDown));
        assert_eq!(view.detail_scroll, 8);
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.detail_scroll, 0);
    }

    #[test]
    fn enter_and_s_open_the_setup_wizard_for_members_only() {
        for code in [KeyCode::Enter, KeyCode::Char('s')] {
            // Operator row: display-only, no wizard hand-off.
            let mut view = built_in_view();
            assert!(view.operator_selected());
            assert!(
                matches!(view.handle_key(key(code)), ViewAction::None),
                "{code:?} must be inert on the operator row"
            );

            // Member row: hands off to the setup wizard.
            view.handle_key(key(KeyCode::Down));
            let action = view.handle_key(key(code));
            assert!(
                matches!(
                    action,
                    ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenSetupRequested)
                ),
                "{code:?} should hand off to the setup wizard"
            );
        }
    }

    #[test]
    fn mouse_hover_focuses_and_click_opens_member_row() {
        let mut view = built_in_view();
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let hit = view
            .list_hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|region| matches!(region.target, HitTarget::Row { index } if index > 0))
            .copied()
            .expect("visible member row hit region");
        let HitTarget::Row { index } = hit.target;

        let hover = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(hover, ViewAction::None));
        assert_eq!(view.selected, index);

        view.selected = 0;
        let click = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            click,
            ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenSetupRequested)
        ));
        assert_eq!(view.selected, index);
    }

    #[test]
    fn mouse_click_on_operator_row_focuses_but_does_not_open_setup() {
        let mut view = built_in_view();
        view.selected = 1;
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let hit = view
            .list_hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|region| matches!(region.target, HitTarget::Row { index: 0 }))
            .copied()
            .expect("operator row hit region");

        let click = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(click, ViewAction::None));
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn esc_closes() {
        let mut view = built_in_view();
        assert!(matches!(
            view.handle_key(key(KeyCode::Esc)),
            ViewAction::Close
        ));
    }

    #[test]
    fn built_in_party_lists_all_members_in_canonical_order() {
        let view = built_in_view();
        let ids: Vec<&str> = view.members.iter().map(|m| m.id.as_str()).collect();
        // The operator is rendered as the pinned session row, not a member
        // (#dogfood 0.8.67), so it is intentionally absent from this list.
        assert_eq!(
            ids,
            [
                "manager",
                "scout",
                "builder",
                "reviewer",
                "verifier",
                "synthesizer",
                "general"
            ]
        );
    }

    #[test]
    fn detail_shows_posture_routing_and_origin() {
        // Built-in reviewer: read-only review worker, shell read-only,
        // inherits the session route.
        let reviewer = FleetRoster::built_ins_only()
            .get("reviewer")
            .unwrap()
            .clone();
        assert_eq!(
            member_posture(&reviewer),
            "review worker · read-only · shell read-only"
        );
        assert_eq!(member_routing(&reviewer), "inherit session route");

        // Built-in scout: fast route preset is the routing truth.
        let scout = FleetRoster::built_ins_only().get("scout").unwrap().clone();
        assert_eq!(
            member_posture(&scout),
            "explore worker · read-only · shell read-only"
        );
        assert_eq!(member_routing(&scout), "route preset fast");

        // Builder writes with full shell.
        let builder = FleetRoster::built_ins_only()
            .get("builder")
            .unwrap()
            .clone();
        assert_eq!(
            member_posture(&builder),
            "implementer worker · write · shell full"
        );

        // A pinned model beats the route preset label.
        let mut pinned = reviewer.clone();
        pinned.profile.model = Some("glm-5.2".to_string());
        assert_eq!(member_routing(&pinned), "model glm-5.2 (pinned)");
    }

    #[test]
    fn member_hues_distinguish_operate_roster_roles() {
        let roster = FleetRoster::built_ins_only();
        let scout = roster.get("scout").unwrap();
        let builder = roster.get("builder").unwrap();
        let reviewer = roster.get("reviewer").unwrap();

        assert_ne!(member_hue(scout), member_hue(builder));
        assert_ne!(member_hue(builder), member_hue(reviewer));
        assert_ne!(member_hue(scout), member_hue(reviewer));
    }

    #[test]
    fn detail_lines_carry_overlay_source_for_project_members() {
        let view = view_with_overrides();
        let reviewer = view.members.iter().find(|m| m.id == "reviewer").unwrap();
        let text = member_detail_lines(reviewer)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.clone().into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("project"), "{text}");
        assert!(
            text.contains("custom overlay (.codewhale/agents/reviewer.toml)"),
            "{text}"
        );
        assert!(text.contains("model glm-5.2 (pinned)"), "{text}");
        assert!(text.contains("spawn depth 1"), "{text}");
    }

    #[test]
    fn roster_loads_config_members_through_the_shared_merge() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "docs-writer".to_string(),
            codewhale_config::FleetProfile {
                slot: codewhale_config::FleetSlot::from_name("scout"),
                role: codewhale_config::FleetRole {
                    name: "scout".to_string(),
                    description: Some("Writes docs.".to_string()),
                    instructions: None,
                },
                loadout: codewhale_config::FleetLoadout::Fast,
                model: None,
                provider: None,
                reasoning_effort: None,
                permissions: codewhale_config::FleetProfilePermissions::default(),
                delegation: codewhale_config::FleetDelegationHints::default(),
            },
        );
        let config = codewhale_config::FleetConfigToml {
            profiles,
            ..codewhale_config::FleetConfigToml::default()
        };
        let view = FleetRosterView::from_parts(operator(), FleetRoster::load(&config, tmp.path()));
        let extra = view.members.iter().find(|m| m.id == "docs-writer").unwrap();
        assert_eq!(extra.origin, ProfileOrigin::Config);
        assert_eq!(member_routing(extra), "route preset fast");
    }

    #[test]
    fn fleet_roster_is_usable_and_opaque_at_blocker_sizes() {
        type Builder = (&'static str, fn() -> FleetRosterView);
        let builders: [Builder; 3] = [
            ("built-ins", built_in_view),
            ("overrides", view_with_overrides),
            ("last-selected", || {
                let mut v = built_in_view();
                v.selected = v.row_count() - 1;
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
                assert!(text.contains("close"), "{label} {w}x{h}: missing footer");
                // The first impression names Fleet as the worker/orchestration surface.
                assert!(
                    text.contains("Fleet") && text.contains("workers"),
                    "{label} {w}x{h}: missing framing"
                );
                // The selected row's detail is on screen.
                assert!(
                    text.contains("Posture"),
                    "{label} {w}x{h}: missing detail pane"
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
    fn selection_stays_visible_when_list_scrolls() {
        // Select the last member and render short: the pointer row must be
        // in the frame.
        let rows = render_through_stack(
            || {
                let mut v = built_in_view();
                v.selected = v.row_count() - 1;
                v
            },
            80,
            24,
        );
        let text = rows.join("\n");
        assert!(text.contains("> general"), "{text}");
    }
}
