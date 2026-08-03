//! `/fleet` roster — the barracks view of the saved agent party.
//!
//! The roster view is the read side of the Fleet profile surface. The first
//! row is the **operator** — the live session route (your main model): when a
//! user picks a session model they are picking the operator, and the roster
//! is that operator's team. Below it the merged [`FleetRoster`] (built-in <
//! `[fleet.profiles]` config < `$CODEWHALE_HOME/agents/*.toml` personal <
//! `.codewhale/agents/*.toml` project members)
//! renders as a scrollable list with a detail pane for the selected row. The
//! view never writes anything; `s` / Enter on a member hands off to the
//! `/fleet setup` wizard for authoring and overrides (the operator row is
//! display-only — its route changes via `/model` or `/provider`).
//!
//! NOTE: like `fleet_setup.rs`, the copy below is intentionally English for
//! now (#3167 reworks Fleet UI localization); the command entry
//! (`CmdFleetDescription`) is already localized.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::fleet::profile::AgentProfile;
use crate::fleet::roster::{FleetRoster, ProfileOrigin};
use crate::fleet::worker_runtime::roster_member_agent_type;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::App;
use crate::tui::menu_style;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_footer,
    truncate_view_text,
};
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

pub struct FleetRosterView {
    operator: OperatorInfo,
    members: Vec<AgentProfile>,
    /// Shadow records from the roster load (#5098): which lower-precedence
    /// files the displayed members are ignoring.
    shadowed: Vec<crate::fleet::roster::ShadowedProfile>,
    /// Selected row: 0 is the pinned operator row, members follow at 1..
    selected: usize,
    detail_scroll: usize,
    /// UI locale captured from the app at construction (#4057 wave 2).
    locale: Locale,
}

impl FleetRosterView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let mut view = Self::from_parts(
            OperatorInfo::from_app(app),
            FleetRoster::load(&config.fleet_config(), &app.workspace),
        );
        view.locale = app.ui_locale;
        view
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
            shadowed: roster.shadowed().to_vec(),
            selected: 0,
            detail_scroll: 0,
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
    }

    fn move_down(&mut self) {
        self.selected = crate::tui::list_nav::wrap_index(self.selected, self.row_count(), 1);
        self.detail_scroll = 0;
    }

    fn footer_hints(&self) -> Vec<ActionHint> {
        vec![
            ActionHint::new("↑/↓", "move"),
            ActionHint::new("s/Enter", "setup"),
            ActionHint::new("w", tr(self.locale, MessageId::FleetRosterWorkers)),
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
            KeyCode::Enter | KeyCode::Char('s') => {
                if let Some(member) = self.selected_member() {
                    let role = member.profile.role.name.clone();
                    // Carry the role the operator already chose. The setup
                    // wizard can still step back to Role when they want to
                    // change it, but does not force a duplicate selection.
                    ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenSetupRequested { role })
                } else {
                    // The operator is not a wizard-authored profile; its
                    // route changes via /model or /provider (the detail pane
                    // says so).
                    ViewAction::None
                }
            }
            KeyCode::Char('w') => {
                ViewAction::EmitAndClose(ViewEvent::FleetRosterOpenWorkersRequested)
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

    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        Block::default()
            .style(Style::default().bg(palette::WHALE_BG))
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
                    Style::default().fg(palette::WHALE_INFO).bold(),
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
                    format!(
                        "  {}",
                        tr(self.locale, MessageId::FleetRosterMembersCount)
                            .replace("{count}", &(self.members.len() + 1).to_string())
                    ),
                    Style::default().fg(palette::TEXT_SECONDARY),
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
        for idx in first..(first + visible_rows).min(self.row_count()) {
            let is_selected = idx == self.selected;
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
                // #5098: badge rows whose winning layer is ignoring a
                // lower-precedence file, so a shadowed personal/project edit
                // is visible from the list, not just the detail pane.
                let shadow_badge = if self
                    .shadowed
                    .iter()
                    .any(|shadow| shadow.id.trim().eq_ignore_ascii_case(member.id.trim()))
                {
                    " ⚠shadows"
                } else {
                    ""
                };
                (
                    format!(
                        "{pointer}{mark} {}  {}{}",
                        member.id,
                        member_routing(member),
                        shadow_badge
                    ),
                    Style::default().fg(palette::TEXT_PRIMARY),
                )
            };
            let style = if is_selected {
                menu_style::selected_row_style()
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
            // Session model is the operator route so "fast" loadouts resolve
            // to the fast sibling the runtime will actually launch.
            member_detail_lines_with_session(
                member,
                Some(self.operator.model.as_str()),
                &self.shadowed,
            )
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

fn member_role_mark(member: &AgentProfile) -> &'static str {
    match member.id.as_str() {
        "manager" | "scout" => crate::tui::glyphs::ROLE_MANAGER,
        "builder" => crate::tui::glyphs::ROLE_BUILDER,
        "reviewer" => crate::tui::glyphs::ROLE_REVIEWER,
        "verifier" => crate::tui::glyphs::ROLE_VERIFIER,
        "synthesizer" => crate::tui::glyphs::ROLE_SYNTHESIZER,
        _ => match roster_member_agent_type(member).as_str() {
            "scout" | "manager" => crate::tui::glyphs::ROLE_MANAGER,
            "builder" => crate::tui::glyphs::ROLE_BUILDER,
            "reviewer" => crate::tui::glyphs::ROLE_REVIEWER,
            "verifier" => crate::tui::glyphs::ROLE_VERIFIER,
            "synthesizer" => crate::tui::glyphs::ROLE_SYNTHESIZER,
            _ => crate::tui::glyphs::NEUTRAL,
        },
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
        "The operator is your main session model; it dispatches Fleet workers via `agent` \
         profile spawns and Workflow task({profile}). Change its route with /model or /provider."
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

/// The routing truth for a member: explicit model pin, else route preset, else
/// same-route inheritance. `[subagents]` overrides still win at dispatch.
///
/// When the loadout is `fast`, show that the runtime resolves the **fast
/// sibling of the active session model** — not a stale on-disk profile name —
/// so the roster matches what Fleet will actually launch.
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
        return format!("model {model} (pinned)");
    }
    match member.profile.loadout.as_str() {
        "inherit" => "inherit session route".to_string(),
        "fast" => match session_model.map(str::trim).filter(|m| !m.is_empty()) {
            Some(session) => format!("fast sibling of {session} (resolved)"),
            None => "route preset fast (resolved at launch)".to_string(),
        },
        loadout => format!("route preset {loadout}"),
    }
}

fn member_detail_lines_with_session(
    member: &AgentProfile,
    session_model: Option<&str>,
    shadowed: &[crate::fleet::roster::ShadowedProfile],
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
        "Origin",
        match member.origin {
            ProfileOrigin::BuiltIn => "built-in (default party)".to_string(),
            _ => format!("{} · {}", member.origin, member.source.display()),
        },
    );
    // #5098: every layer this member is displacing, so a shadowed personal or
    // project file is visible instead of silently dropped from the merge.
    for shadow in shadowed
        .iter()
        .filter(|shadow| shadow.id.trim().eq_ignore_ascii_case(member.id.trim()))
    {
        detail_field(
            &mut lines,
            "Shadows",
            format!(
                "{} copy at {} (ignored)",
                shadow.shadowed_origin,
                shadow.shadowed_source.display()
            ),
        );
    }
    detail_field(&mut lines, "Slot", member.profile.slot.as_str().to_string());
    detail_field(&mut lines, "Posture", member_posture(member));
    detail_field(
        &mut lines,
        "Routing",
        member_routing_with_session(member, session_model),
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
