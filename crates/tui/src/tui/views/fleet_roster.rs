//! `/fleet` roster — the barracks view of the saved agent party.
//!
//! The roster view is the primary `/fleet` face. The first row is the
//! **operator** — the Fleet leader (your live session model). When a user
//! picks a session model they are picking the operator, and every member
//! below is that leader's team. The header names the selected saved Fleet and
//! whether it is user-global or folder-scoped, so scope is never ambiguous.
//! Below the operator sits the merged [`FleetRoster`] (built-in <
//! `[fleet.profiles]` config < `$CODEWHALE_HOME/agents/*.toml` personal <
//! `.codewhale/agents/*.toml` project members)
//! as a scrollable list with a detail pane for the selected row. The view
//! never writes anything; `s` / Enter on a selected-v2 member opens that
//! Fleet's exact editor, while the legacy profile wizard is used only when no
//! named Fleet is selected (the operator row is display-only). Switch named
//! saved Fleets with `/fleet fleets` (`/fleet fleets` remains compatible).
//!
//! NOTE: like `fleet_setup.rs`, the copy below is intentionally English for
//! now (#3167 reworks Fleet UI localization); the command entry
//! (`CmdFleetDescription`) is already localized.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::fleet::profile::AgentProfile;
use crate::fleet::role::public_role_label;
use crate::fleet::roster::{FleetRoster, ProfileLayer, ProfileOrigin, layers_from_parts};
use crate::fleet::worker_runtime::roster_member_agent_type;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::App;
use crate::tui::menu_style;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_footer,
    truncate_view_text,
};
use crate::tui::whales;
use crate::worker_profile::{ShellPolicy, WorkerRuntimeProfile};

/// The live session route — the operator the roster works for. Read once at
/// open, the same way [`super::fleet_setup::FleetSetupSnapshot`] snapshots it.
#[derive(Debug, Clone)]
struct OperatorInfo {
    provider: String,
    /// Exact canonical route key, kept separate from the display label so
    /// capability lookup can use provider-scoped catalog facts.
    provider_id: String,
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
        let route_provider = if app.auto_model {
            app.last_effective_provider.unwrap_or(app.api_provider)
        } else {
            app.api_provider
        };
        let provider_id = if app.auto_model {
            app.last_effective_provider_identity
                .clone()
                .unwrap_or_else(|| {
                    if route_provider == crate::config::ApiProvider::Custom {
                        app.provider_identity_for_persistence().to_string()
                    } else {
                        route_provider.as_str().to_string()
                    }
                })
        } else {
            app.provider_identity_for_persistence().to_string()
        };
        let provider = if route_provider == crate::config::ApiProvider::Custom {
            provider_id.clone()
        } else {
            route_provider.display_name().to_string()
        };
        Self {
            provider,
            provider_id,
            model,
            reasoning: app.reasoning_effort_display_label(),
        }
    }
}

/// Which named Fleet (if any) this session is using, and where that selection
/// is pinned — user-global vs this folder only.
#[derive(Debug, Clone)]
struct SelectedFleetSummary {
    name: String,
    scope: crate::fleet::store::FleetScope,
}

/// View-owned action attached to a painted saved-profile row.
///
/// This stays deliberately separate from Tideline's live-worker targets,
/// which are backed by `SubAgentStatus`, not editable profiles in this roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FleetRosterRowAction {
    SelectOrActivate { row: usize },
}

impl FleetRosterRowAction {
    const fn row(self) -> usize {
        match self {
            Self::SelectOrActivate { row } => row,
        }
    }
}

pub struct FleetRosterView {
    operator: OperatorInfo,
    members: Vec<AgentProfile>,
    /// Shadow records from the roster load (#5098): which lower-precedence
    /// files the displayed members are ignoring.
    shadowed: Vec<crate::fleet::roster::ShadowedProfile>,
    /// Selected named Fleet + scope, when one is active for this session.
    selected_fleet: Option<SelectedFleetSummary>,
    /// A selected Fleet existed but could not become the runtime roster.
    load_error: Option<String>,
    /// Selected row: 0 is the pinned operator row, members follow at 1..
    selected: usize,
    detail_scroll: usize,
    /// Exact visible row geometry from the latest render. This is a
    /// frame-scoped projection, not a second roster or navigation owner.
    row_hitboxes: RefCell<Vec<(Rect, FleetRosterRowAction)>>,
    /// A first click selects/reveals details; a consecutive click on the same
    /// row activates the exact same handoff as Enter.
    last_mouse_selected: Option<usize>,
    /// Row under the pointer, tinted with the shared hover style. Hover
    /// never moves the keyboard selection; only painted rows answer.
    hovered_row: Cell<Option<usize>>,
    /// Canonical active-theme surface captured from `App`; Terminal owns
    /// `Color::Reset`, while explicit themes retain their resolved surface.
    surface_bg: Color,
    /// UI locale captured from the app at construction (#4057 wave 2).
    locale: Locale,
}

impl FleetRosterView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let selected_fleet =
            crate::fleet::store::selected_fleet(&app.workspace).map(|sel| SelectedFleetSummary {
                name: sel.name,
                scope: sel.scope,
            });
        let mut view = Self::from_parts(
            OperatorInfo::from_app(app),
            crate::fleet::identity::load_effective_roster(
                &config.fleet_config(),
                &app.workspace,
                Some(app.plugin_registry.as_ref()),
            ),
            selected_fleet,
        );
        view.locale = app.ui_locale;
        view.surface_bg = app.ui_theme.surface_bg;
        view
    }

    fn from_parts(
        operator: OperatorInfo,
        roster: FleetRoster,
        selected_fleet: Option<SelectedFleetSummary>,
    ) -> Self {
        let load_error = roster.load_error().map(str::to_string);
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
            shadowed: roster.shadowed().to_vec(),
            selected_fleet,
            load_error,
            selected: 0,
            detail_scroll: 0,
            row_hitboxes: RefCell::new(Vec::new()),
            last_mouse_selected: None,
            hovered_row: Cell::new(None),
            surface_bg: palette::UI_THEME.surface_bg,
            locale: Locale::En,
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
        self.selected = crate::tui::list_nav::wrap_index(self.selected, self.row_count(), -1);
        self.detail_scroll = 0;
        self.last_mouse_selected = None;
        self.hovered_row.set(None);
    }

    fn move_down(&mut self) {
        self.selected = crate::tui::list_nav::wrap_index(self.selected, self.row_count(), 1);
        self.detail_scroll = 0;
        self.last_mouse_selected = None;
        self.hovered_row.set(None);
    }

    fn select_row(&mut self, row: usize) {
        self.selected = row.min(self.row_count().saturating_sub(1));
        self.detail_scroll = 0;
    }

    fn activate_selected(&self) -> ViewAction {
        if let Some(member) = self.selected_member() {
            let member_id = member.id.clone();
            // Carry the exact member the operator already chose. The host
            // focuses it in the selected v2 Fleet editor, or starts legacy
            // setup from its member id when no Fleet is selected.
            ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenSetupRequested { member_id })
        } else {
            // The operator is not a wizard-authored profile; its route changes
            // via /model or /provider (the detail pane says so).
            ViewAction::None
        }
    }

    fn select_or_activate_mouse_row(&mut self, row: usize) -> ViewAction {
        let activate = self.last_mouse_selected == Some(row) && self.selected == row;
        self.select_row(row);
        self.last_mouse_selected = Some(row);
        if activate {
            self.activate_selected()
        } else {
            ViewAction::None
        }
    }

    fn footer_hints(&self) -> Vec<ActionHint> {
        let edit_label = if self.selected_fleet.is_some() {
            "edit Fleet"
        } else {
            "setup profile"
        };
        let mut hints = vec![
            ActionHint::new("↑/↓", "move"),
            ActionHint::new("s/Enter", edit_label),
            ActionHint::new("f", "saved Fleets"),
            ActionHint::new("w", tr(self.locale, MessageId::FleetRosterWorkers)),
            ActionHint::new("PgUp/PgDn", "scroll detail"),
            ActionHint::new("Esc", "close"),
        ];
        if self.selected_fleet.is_some() && !self.operator_selected() {
            hints.insert(2, ActionHint::new("m", "model"));
        }
        hints
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
        // A keyboard gesture ends any pending mouse double-click sequence so
        // a later single click can never activate a stale row.
        self.last_mouse_selected = None;
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
            KeyCode::Enter | KeyCode::Char('s') => self.activate_selected(),
            KeyCode::Char('m') if self.selected_fleet.is_some() => {
                let Some(member) = self.selected_member() else {
                    return ViewAction::None;
                };
                ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenModelRequested {
                    member_id: member.id.clone(),
                })
            }
            KeyCode::Char('w') => {
                ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenWorkersRequested)
            }
            KeyCode::Char('f') => {
                ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenFleetsRequested)
            }
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
            MouseEventKind::Moved => {
                let hovered = self
                    .row_hitboxes
                    .borrow()
                    .iter()
                    .find_map(|(rect, action)| {
                        rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                            .then_some(action.row())
                    });
                self.hovered_row.set(hovered);
                ViewAction::None
            }
            MouseEventKind::ScrollUp => {
                self.move_up();
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.move_down();
                ViewAction::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let action = self
                    .row_hitboxes
                    .borrow()
                    .iter()
                    .find_map(|(rect, action)| {
                        rect.contains(ratatui::layout::Position::new(mouse.column, mouse.row))
                            .then_some(*action)
                    });
                action.map_or(ViewAction::None, |action| {
                    self.select_or_activate_mouse_row(action.row())
                })
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        Block::default()
            .style(Style::default().bg(self.surface_bg))
            .render(area, buf);

        let hints = self.footer_hints();
        let content = render_modal_footer(area, buf, &hints);

        // Hairline shell shared with the HTML route/config/Fleet surfaces.
        // This replaces the centered legacy card: Fleet is a product room,
        // not a popup floating over an unrelated transcript.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(content);
        let header = vec![
            Line::from(vec![
                Span::styled(
                    format!("─ {} ", tr(self.locale, MessageId::FleetRosterHeaderLabel)),
                    Style::default().fg(palette::WHALE_ACTION).bold(),
                ),
                Span::styled(
                    "──────────────────────── ",
                    Style::default().fg(palette::BORDER_COLOR),
                ),
                Span::styled(
                    tr(self.locale, MessageId::FleetRosterTabRoster),
                    Style::default().fg(palette::WHALE_ACTION).bold(),
                ),
                Span::styled(
                    format!(
                        "  {}  {} ",
                        tr(self.locale, MessageId::FleetRosterTabSetup),
                        tr(self.locale, MessageId::FleetRosterWorkers)
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled("─".repeat(24), Style::default().fg(palette::BORDER_COLOR)),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    format!("  {}", self.selected_fleet_line()),
                    Style::default().fg(palette::TEXT_SECONDARY),
                ),
                Span::styled(
                    format!(
                        " · {}",
                        tr(self.locale, MessageId::FleetRosterMembersCount)
                            .replace("{count}", &(self.members.len() + 1).to_string())
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled(
                    format!(
                        " · {}",
                        tr(self.locale, MessageId::FleetRosterOperatorFirst)
                    ),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ]),
        ];
        Paragraph::new(header)
            .wrap(Wrap { trim: false })
            .render(chunks[0], buf);

        self.render_body(chunks[1], buf);
    }
}

impl FleetRosterView {
    /// Scope-explicit selected Fleet line. Paths stay out — receipts name them.
    fn selected_fleet_line(&self) -> String {
        if let Some(error) = &self.load_error {
            return format!("Fleet selection error — {error}");
        }
        match &self.selected_fleet {
            Some(sel) => format!("Fleet `{}` · {}", sel.name, sel.scope.long_label()),
            None => "No Fleet selected — built-in team".to_string(),
        }
    }

    fn render_body(&self, area: Rect, buf: &mut Buffer) {
        self.row_hitboxes.borrow_mut().clear();
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Two columns when there is room, stacked otherwise — same responsive
        // shape as the setup wizard's choice step so nothing truncates at
        // 80x24.
        let (list_area, detail_area) = if area.width >= 56 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(45),
                    Constraint::Length(2),
                    Constraint::Min(20),
                ])
                .split(area);
            (cols[0], cols[2])
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
        for (line_offset, idx) in (first..(first + visible_rows).min(self.row_count())).enumerate()
        {
            self.row_hitboxes.borrow_mut().push((
                Rect::new(
                    list_area.x,
                    list_area
                        .y
                        .saturating_add(u16::try_from(line_offset).unwrap_or(u16::MAX)),
                    list_area.width,
                    1,
                ),
                FleetRosterRowAction::SelectOrActivate { row: idx },
            ));
            let is_selected = idx == self.selected;
            // Hover tints but never steals the keyboard selection.
            let hovered = !is_selected && self.hovered_row.get() == Some(idx);
            let hover_tint = || menu_style::hovered_row_style();
            let pointer = format!("{} ", crate::tui::glyphs::selection_marker(is_selected));
            let (text, base_style) = if idx == 0 {
                (
                    format!(
                        "{pointer}@ {}  {}",
                        tr(self.locale, MessageId::FleetRosterOperatorRow),
                        self.operator.model
                    ),
                    Style::default()
                        .fg(palette::WHALE_ACTION)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                let member = &self.members[idx - 1];
                let mark = member_role_mark(member);
                // #5098: badge rows whose id exists in more than one layer
                // so a higher-layer win is visible from the list.
                let shadow_badge = member_shadow_badge(self.locale, member, &self.shadowed);
                // Whale Teams: the species badge sits between the charter role
                // mark and the id, so a Scout, Patch, or Lantern reads at a
                // glance even before the detail pane opens.
                let species = member_species(member);
                let badge_cells = whales::BADGE_WIDTH + 1;
                let edit_marker = if is_selected && self.selected_fleet.is_some() {
                    "[edit] "
                } else {
                    ""
                };
                let member_name = member
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(&member.id))
                    .map_or_else(
                        || member.id.clone(),
                        |name| format!("{name} ({})", member.id),
                    );
                let text = format!(
                    "{pointer}{edit_marker}{mark} {}{}  {}",
                    member_name,
                    shadow_badge.as_deref().unwrap_or(""),
                    member_routing(member)
                );
                let text = truncate_view_text(&text, list_width.saturating_sub(badge_cells));
                let base_style = if is_selected {
                    menu_style::selected_row_style()
                } else if hovered {
                    Style::default()
                        .fg(palette::TEXT_PRIMARY)
                        .patch(hover_tint())
                } else {
                    Style::default().fg(palette::TEXT_PRIMARY)
                };
                let split = pointer.len() + edit_marker.len() + mark.len() + 1;
                let (head, tail) = if text.len() >= split && text.is_char_boundary(split) {
                    text.split_at(split)
                } else {
                    (text.as_str(), "")
                };
                let mut spans = vec![Span::styled(head.to_string(), base_style)];
                for span in whales::badge(species, &palette::UI_THEME) {
                    spans.push(if is_selected {
                        Span::styled(
                            span.content,
                            span.style
                                .bg(palette::SELECTION_BG)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else if hovered {
                        Span::styled(span.content, span.style.patch(hover_tint()))
                    } else {
                        span
                    });
                }
                spans.push(Span::styled(format!(" {tail}"), base_style));
                list_lines.push(Line::from(spans));
                continue;
            };
            let style = if is_selected {
                menu_style::selected_row_style()
            } else if hovered {
                base_style.patch(hover_tint())
            } else {
                base_style
            };
            list_lines.push(Line::from(Span::styled(
                truncate_view_text(&text, list_width),
                style,
            )));
        }
        Paragraph::new(list_lines).render(list_area, buf);

        // Detail pane for the selected row.
        let lines = if self.operator_selected() {
            operator_detail_lines(&self.operator)
        } else if let Some(member) = self.selected_member() {
            // Whale Teams identity first: the species badge plus species and
            // job. Rendered without a state — a roster member is a profile,
            // not a runtime, so this claims nothing about whether anyone is
            // working. (The hand-drawn portrait that used to open this pane
            // was deleted per the 2026-08-29 founder directive.)
            let mut lines = whale_identity_lines(member, self.locale);
            // Session model is the operator route so "fast" loadouts resolve
            // to the fast sibling the runtime will actually launch.
            lines.extend(member_detail_lines_with_session(
                member,
                Some(self.operator.model.as_str()),
                &self.shadowed,
                self.locale,
            ));
            lines
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

/// Species for a roster member: the profile id first (built-in ids are role
/// names), then the resolved worker agent type. Unknown → the plain whale.
fn member_species(member: &AgentProfile) -> whales::WhaleSpecies {
    match whales::WhaleSpecies::for_role_id(&member.id) {
        whales::WhaleSpecies::Plain => {
            whales::WhaleSpecies::for_fleet_role(&roster_member_agent_type(member))
        }
        species => species,
    }
}

/// Identity block for the detail pane: the species badge, then
/// `Name · species · job`. No state is drawn or claimed — a roster member is
/// a profile, not a runtime.
fn whale_identity_lines(member: &AgentProfile, locale: Locale) -> Vec<Line<'static>> {
    let species = member_species(member);
    let theme = &palette::UI_THEME;
    let mut lines: Vec<Line> = Vec::new();
    let mut caption = whales::badge(species, theme);
    caption.push(Span::styled(
        format!(
            " {} · {} · {}",
            species.name(),
            species.animal(locale),
            species.job(locale)
        ),
        Style::default().fg(palette::TEXT_PRIMARY),
    ));
    lines.push(Line::from(caption));
    lines.push(Line::from(""));
    lines
}

fn member_role_mark(member: &AgentProfile) -> &'static str {
    let role = public_role_label(&member.id);
    match role.as_str() {
        "manager" | "explore" => crate::tui::glyphs::ROLE_MANAGER,
        "implement" => crate::tui::glyphs::ROLE_BUILDER,
        "reviewer" => crate::tui::glyphs::ROLE_REVIEWER,
        "test" => crate::tui::glyphs::ROLE_VERIFIER,
        "synthesizer" => crate::tui::glyphs::ROLE_SYNTHESIZER,
        _ => match roster_member_agent_type(member).as_str() {
            "explore" | "manager" => crate::tui::glyphs::ROLE_MANAGER,
            "implement" => crate::tui::glyphs::ROLE_BUILDER,
            "reviewer" => crate::tui::glyphs::ROLE_REVIEWER,
            "test" => crate::tui::glyphs::ROLE_VERIFIER,
            "synthesizer" => crate::tui::glyphs::ROLE_SYNTHESIZER,
            _ => crate::tui::glyphs::NEUTRAL,
        },
    }
}

/// Shared field renderer for the detail pane.
fn detail_field(lines: &mut Vec<Line<'static>>, label: &str, body: String) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(palette::WHALE_ACTION).bold(),
    )));
    lines.push(Line::from(Span::styled(
        body,
        Style::default().fg(palette::TEXT_PRIMARY),
    )));
    lines.push(Line::from(""));
}

/// Detail pane for the pinned operator row: the live session route, plus the
/// product truth that the operator is this Fleet's leader.
fn operator_detail_lines(operator: &OperatorInfo) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    detail_field(
        &mut lines,
        "Role",
        "Coordinator — this session's model leads the Fleet".to_string(),
    );
    detail_field(&mut lines, "Saved for", "this session only".to_string());
    detail_field(&mut lines, "Access", "full session access".to_string());
    detail_field(&mut lines, "Provider", operator.provider.clone());
    detail_field(&mut lines, "Model", operator.model.clone());
    // Session-route capability badges (#5038). Use the exact route key rather
    // than the display label so built-in routes get provider-scoped catalog
    // facts; custom routes still fall back conservatively to registry facts.
    if let Some(badges) = crate::fleet::capability_badges::resolve_route_capability_badges(
        Some(&operator.provider_id),
        &operator.model,
    ) {
        detail_field(&mut lines, "Capabilities", badges.summary());
    }
    detail_field(&mut lines, "Reasoning", operator.reasoning.clone());
    detail_field(
        &mut lines,
        "Description",
        "The Coordinator is this Fleet's leader — your main session model. Every \
         member below works for it. Change the model with /model or /provider; \
         persist with /fleet save."
            .to_string(),
    );
    lines
}

/// The resolved worker posture for a roster member: what the runtime would
/// actually grant when this member is dispatched (role posture, not the
/// profile's requested permissions).
/// Plain-Access summary for a roster member: what it may do, derived from the
/// same runtime profile dispatch would grant. No internal role/posture words.
fn member_access_summary(member: &AgentProfile) -> String {
    let agent_type = roster_member_agent_type(member);
    let runtime = WorkerRuntimeProfile::for_role(agent_type.clone());
    let write = if runtime.permissions.write {
        "can edit files"
    } else {
        "read-only files"
    };
    let shell = match runtime.shell {
        ShellPolicy::None => "cannot run commands",
        ShellPolicy::ReadOnly => "read-only commands",
        ShellPolicy::Full => "can run commands",
    };
    let network = if runtime.permissions.network {
        "network"
    } else {
        "no network"
    };
    format!("{write} · {shell} · {network}")
}

/// The model truth for a member: explicit model choice, else saved model set,
/// else the session's model. `[subagents]` overrides still win at dispatch.
///
/// When the loadout is `fast`, show that the runtime picks the **fast sibling
/// of the active session model** — not a stale on-disk profile name — so the
/// roster matches what Fleet will actually launch.
fn member_routing(member: &AgentProfile) -> String {
    member_routing_with_session(member, None)
}

fn member_routing_with_session(member: &AgentProfile, session_model: Option<&str>) -> String {
    if let Some(model) = member
        .profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        if let Some(provider) = member
            .profile
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
        {
            return format!("model {provider}/{model}");
        }
        return format!("model {model}");
    }
    match member.profile.loadout.as_str() {
        "inherit" => "same model as this session".to_string(),
        "fast" => match session_model.map(str::trim).filter(|m| !m.is_empty()) {
            Some(session) => format!("fast model for {session}"),
            None => "fast model, picked at launch".to_string(),
        },
        loadout => format!("saved model set {loadout}"),
    }
}

fn member_shadow_badge(
    locale: Locale,
    member: &AgentProfile,
    shadowed: &[crate::fleet::roster::ShadowedProfile],
) -> Option<String> {
    let layers = layers_from_parts(member, shadowed);
    if layers.len() < 2 {
        return None;
    }
    let personal_ignored = layers
        .iter()
        .any(|layer| !layer.wins && layer.origin == ProfileOrigin::Personal);
    let id = if personal_ignored {
        MessageId::FleetRosterShadowBadgePersonalIgnored
    } else {
        match member.origin {
            ProfileOrigin::Workspace => MessageId::FleetRosterShadowBadgeProjectOverride,
            ProfileOrigin::Personal => MessageId::FleetRosterShadowBadgePersonalOverride,
            ProfileOrigin::Config => MessageId::FleetRosterShadowBadgeConfigOverride,
            ProfileOrigin::Plugin | ProfileOrigin::BuiltIn => return None,
        }
    };
    Some(format!("  {}", tr(locale, id)))
}

fn format_profile_layer(layer: &ProfileLayer, locale: Locale) -> String {
    let mark = if layer.wins {
        tr(locale, MessageId::FleetRosterLayerWins)
    } else {
        tr(locale, MessageId::FleetRosterLayerIgnored)
    };
    format!("{} · {} ({mark})", layer.origin, layer.source.display())
}

fn member_detail_lines_with_session(
    member: &AgentProfile,
    session_model: Option<&str>,
    shadowed: &[crate::fleet::roster::ShadowedProfile],
    locale: Locale,
) -> Vec<Line<'static>> {
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
        "Role",
        member.profile.role.name.trim().to_string(),
    );
    detail_field(
        &mut lines,
        "Saved for",
        match member.origin {
            ProfileOrigin::BuiltIn => "all projects (built-in team)".to_string(),
            ProfileOrigin::Workspace => "this project".to_string(),
            _ => format!("{} · {}", member.origin, member.source.display()),
        },
    );
    // #5098: every layer found for this id, with the winner named. The
    // Origin field still shows the effective copy; this list is the full
    // stack so a personal/config edit is visible when project wins.
    let layers = layers_from_parts(member, shadowed);
    if layers.len() > 1 {
        let body = layers
            .iter()
            .map(|layer| format_profile_layer(layer, locale))
            .collect::<Vec<_>>()
            .join("\n");
        detail_field(
            &mut lines,
            &tr(locale, MessageId::FleetRosterLayersLabel),
            body,
        );
    }
    // Slot is internal dispatch vocabulary and duplicates Role — never shown.
    detail_field(&mut lines, "Access", member_access_summary(member));
    if let Some(provider) = member
        .profile
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
    {
        detail_field(&mut lines, "Provider", provider.to_string());
    }
    detail_field(
        &mut lines,
        "Model",
        match (
            member.profile.model.as_deref(),
            crate::fleet::identity::friendly_model_name(member),
        ) {
            (Some(model), Some(name)) if !name.eq_ignore_ascii_case(model.trim()) => {
                format!("{name} ({})", model.trim())
            }
            _ => member_routing_with_session(member, session_model),
        },
    );

    // Capability badges for a pinned model, from the shared Fleet resolver
    // (#5038). Unknown models omit the field rather than fabricating facts.
    if let Some(model) = member
        .profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        && let Some(badges) = crate::fleet::capability_badges::resolve_route_capability_badges(
            member.profile.provider.as_deref(),
            model,
        )
    {
        detail_field(&mut lines, "Capabilities", badges.summary());
    }

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
                ProfileOrigin::Personal => {
                    format!("personal overlay ({})", member.source.display())
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
mod tests;
