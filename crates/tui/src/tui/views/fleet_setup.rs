//! Fleet setup and loadout planner.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::config::Config;
use crate::palette;
use crate::tui::app::App;
use crate::tui::views::{
    CommandPaletteAction, ModalKind, ModalView, ViewAction, ViewEvent, truncate_view_text,
};

const PROFILE_DIR: &str = ".codewhale/agents";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowTone {
    Current,
    Ready,
    Info,
    Warning,
}

impl RowTone {
    fn style(self, selected: bool) -> Style {
        if selected {
            return Style::default()
                .fg(palette::SELECTION_TEXT)
                .bg(palette::SELECTION_BG)
                .add_modifier(Modifier::BOLD);
        }

        Style::default().fg(match self {
            Self::Current => palette::WHALE_ACCENT_PRIMARY,
            Self::Ready => palette::STATUS_SUCCESS,
            Self::Info => palette::TEXT_PRIMARY,
            Self::Warning => palette::STATUS_WARNING,
        })
    }
}

#[derive(Debug, Clone)]
struct FleetSetupRow {
    label: String,
    value: String,
    detail: String,
    tone: RowTone,
}

impl FleetSetupRow {
    fn new(label: impl Into<String>, value: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            detail: detail.into(),
            tone: RowTone::Info,
        }
    }

    fn tone(mut self, tone: RowTone) -> Self {
        self.tone = tone;
        self
    }
}

#[derive(Debug, Clone)]
struct FleetSetupLane {
    title: &'static str,
    subtitle: &'static str,
    rows: Vec<FleetSetupRow>,
}

#[derive(Debug, Clone)]
pub struct FleetSetupSnapshot {
    workspace: PathBuf,
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

        Self {
            workspace: app.workspace.clone(),
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
        }
    }
}

pub struct FleetSetupView {
    lanes: Vec<FleetSetupLane>,
    selected_lane: usize,
    selected_rows: Vec<usize>,
    scrolls: Vec<usize>,
    profile_prompt: String,
}

impl FleetSetupView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        Self::from_snapshot(FleetSetupSnapshot::from_app(app, config))
    }

    fn from_snapshot(snapshot: FleetSetupSnapshot) -> Self {
        let profile_prompt = profile_authoring_prompt(&snapshot);
        let lanes = build_lanes(&snapshot);
        let len = lanes.len();
        Self {
            lanes,
            selected_lane: 0,
            selected_rows: vec![0; len],
            scrolls: vec![0; len],
            profile_prompt,
        }
    }

    fn selected_row(&self) -> usize {
        self.selected_rows
            .get(self.selected_lane)
            .copied()
            .unwrap_or_default()
    }

    fn selected_row_mut(&mut self) -> &mut usize {
        &mut self.selected_rows[self.selected_lane]
    }

    fn selected_scroll_mut(&mut self) -> &mut usize {
        &mut self.scrolls[self.selected_lane]
    }

    fn move_lane_left(&mut self) {
        self.selected_lane = self.selected_lane.saturating_sub(1);
    }

    fn move_lane_right(&mut self) {
        if self.selected_lane + 1 < self.lanes.len() {
            self.selected_lane += 1;
        }
    }

    fn move_row_up(&mut self) {
        *self.selected_row_mut() = self.selected_row().saturating_sub(1);
        *self.selected_scroll_mut() = self.selected_row();
    }

    fn move_row_down(&mut self) {
        let max = self
            .lanes
            .get(self.selected_lane)
            .map(|lane| lane.rows.len().saturating_sub(1))
            .unwrap_or_default();
        let next = (self.selected_row() + 1).min(max);
        *self.selected_row_mut() = next;
        *self.selected_scroll_mut() = next.saturating_sub(4);
    }

    fn insert_profile_prompt_action(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
            action: CommandPaletteAction::InsertText {
                text: self.profile_prompt.clone(),
            },
        })
    }

    #[cfg(test)]
    fn profile_prompt(&self) -> &str {
        &self.profile_prompt
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
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_lane_left();
                ViewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_lane_right();
                ViewAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_row_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_row_down();
                ViewAction::None
            }
            KeyCode::Enter | KeyCode::Char('g') | KeyCode::Char('G') => {
                self.insert_profile_prompt_action()
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = area.width.saturating_sub(4).clamp(72, 116).min(area.width);
        let popup_height = area.height.saturating_sub(4).clamp(18, 32).min(area.height);
        let popup_area = Rect {
            x: area.x + area.width.saturating_sub(popup_width) / 2,
            y: area.y + area.height.saturating_sub(popup_height) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Fleet Setup ",
                Style::default()
                    .fg(palette::WHALE_ACCENT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Left/Right ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("lane "),
                Span::styled(" Up/Down ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("option "),
                Span::styled(" Enter/G ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("profile prompt "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("close "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let direction = if inner.width >= 94 {
            Direction::Horizontal
        } else {
            Direction::Vertical
        };
        let constraints = if direction == Direction::Horizontal {
            vec![Constraint::Percentage(25); self.lanes.len()]
        } else {
            vec![Constraint::Length(6); self.lanes.len()]
        };
        let areas = Layout::default()
            .direction(direction)
            .constraints(constraints)
            .split(inner);

        for (idx, lane) in self.lanes.iter().enumerate() {
            let focused = idx == self.selected_lane;
            let scroll = self.scrolls.get(idx).copied().unwrap_or_default();
            let selected = self.selected_rows.get(idx).copied().unwrap_or_default();
            render_lane(lane, areas[idx], buf, focused, selected, scroll);
        }
    }
}

fn build_lanes(snapshot: &FleetSetupSnapshot) -> Vec<FleetSetupLane> {
    let (profile_value, profile_detail) = profile_file_status(&snapshot.workspace);
    let token_budget = snapshot
        .token_budget
        .map(|budget| format!("{budget} tokens"))
        .unwrap_or_else(|| "unbounded".to_string());

    vec![
        FleetSetupLane {
            title: "1 Role",
            subtitle: "what the worker does",
            rows: vec![
                FleetSetupRow::new("manager", "plan/split", "coordinates queued Fleet work"),
                FleetSetupRow::new("scout", "read-first", "research and repo reconnaissance"),
                FleetSetupRow::new("builder", "write", "implements bounded changes"),
                FleetSetupRow::new("reviewer", "read-only", "checks regressions and tests"),
                FleetSetupRow::new("verifier", "test-runner", "runs focused validation"),
                FleetSetupRow::new("summarizer", "reduce", "turns receipts into handoff state"),
            ],
        },
        FleetSetupLane {
            title: "2 Profile",
            subtitle: "persona and prompt",
            rows: vec![
                FleetSetupRow::new(
                    "current route",
                    format!("{} / {}", snapshot.provider, snapshot.model),
                    format!("reasoning {}", snapshot.reasoning),
                )
                .tone(RowTone::Current),
                FleetSetupRow::new("workspace files", profile_value, profile_detail)
                    .tone(RowTone::Ready),
                FleetSetupRow::new(
                    "custom profile",
                    "generate TOML",
                    "Enter inserts a safe authoring prompt",
                )
                .tone(RowTone::Current),
            ],
        },
        FleetSetupLane {
            title: "3 Loadout",
            subtitle: "model class intent",
            rows: vec![
                FleetSetupRow::new("inherit", "session route", "reuse active provider/model"),
                FleetSetupRow::new("fast", "scout", "low-latency fanout and summaries"),
                FleetSetupRow::new("balanced", "default", "normal build/review work")
                    .tone(RowTone::Ready),
                FleetSetupRow::new("deep-reasoning", "hard", "debugging and architecture"),
                FleetSetupRow::new("code", "builder", "implementation-heavy tasks"),
                FleetSetupRow::new("review", "reviewer", "regression and test analysis"),
                FleetSetupRow::new("tool-heavy", "operator", "shell and artifact workflows"),
            ],
        },
        FleetSetupLane {
            title: "4 Policy",
            subtitle: "runtime and recursion",
            rows: vec![
                FleetSetupRow::new(
                    "sub-agents",
                    if snapshot.subagents_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    format!(
                        "{} concurrent, {} launch slots, {} admitted",
                        snapshot.max_subagents, snapshot.launch_concurrency, snapshot.max_admitted
                    ),
                )
                .tone(if snapshot.subagents_enabled {
                    RowTone::Ready
                } else {
                    RowTone::Warning
                }),
                FleetSetupRow::new(
                    "recursion",
                    format!(
                        "agent {} / fleet {}",
                        snapshot.subagent_spawn_depth, snapshot.fleet_spawn_depth
                    ),
                    format!("ceiling {}", codewhale_config::MAX_SPAWN_DEPTH_CEILING),
                )
                .tone(RowTone::Ready),
                FleetSetupRow::new(
                    "child safety",
                    "intersection",
                    "children narrow permissions, shell, tools, and depth",
                )
                .tone(RowTone::Ready),
                FleetSetupRow::new(
                    "budget",
                    token_budget,
                    format!(
                        "{}s api, {}s heartbeat",
                        snapshot.api_timeout_secs, snapshot.heartbeat_timeout_secs
                    ),
                ),
                FleetSetupRow::new(
                    "execution",
                    "Fleet -> agent",
                    "Fleet workers convert into AgentWorkerSpec",
                )
                .tone(RowTone::Current),
            ],
        },
    ]
}

fn render_lane(
    lane: &FleetSetupLane,
    area: Rect,
    buf: &mut Buffer,
    focused: bool,
    selected: usize,
    scroll: usize,
) {
    let border = if focused {
        palette::WHALE_ACCENT_PRIMARY
    } else {
        palette::BORDER_COLOR
    };
    let block = Block::default()
        .title(Line::from(Span::styled(
            format!(" {} ", lane.title),
            Style::default()
                .fg(if focused {
                    palette::WHALE_ACCENT_PRIMARY
                } else {
                    palette::TEXT_MUTED
                })
                .add_modifier(Modifier::BOLD),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(palette::DEEPSEEK_INK))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    block.render(area, buf);

    let width = inner.width.saturating_sub(1) as usize;
    let visible_rows = inner.height.saturating_sub(2) as usize;
    let max_scroll = lane.rows.len().saturating_sub(visible_rows);
    let scroll = scroll.min(max_scroll);

    let mut lines = Vec::with_capacity(visible_rows + 2);
    lines.push(Line::from(Span::styled(
        truncate_view_text(lane.subtitle, width),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    for (row_idx, row) in lane.rows.iter().enumerate().skip(scroll).take(visible_rows) {
        let is_selected = focused && row_idx == selected;
        let row_style = row.tone.style(is_selected);
        let muted_style = if is_selected {
            row_style
        } else {
            Style::default().fg(palette::TEXT_MUTED)
        };
        let pointer = if is_selected { ">" } else { " " };
        let summary = format!("{pointer} {}: {}", row.label, row.value);
        lines.push(Line::from(Span::styled(
            truncate_view_text(&summary, width),
            row_style,
        )));
        if inner.height >= 9 {
            lines.push(Line::from(Span::styled(
                truncate_view_text(&format!("  {}", row.detail), width),
                muted_style,
            )));
        }
    }

    Paragraph::new(lines).render(inner, buf);
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

fn profile_authoring_prompt(snapshot: &FleetSetupSnapshot) -> String {
    format!(
        "Create a safe CodeWhale Fleet persona file for this workspace.\n\n\
         Target path: {PROFILE_DIR}/reviewer.toml\n\
         Current route context only: provider = {provider}, model = {model}, reasoning = {reasoning}\n\n\
         Write TOML using only this schema:\n\
         - name\n\
         - display_name\n\
         - description\n\
         - role_hint\n\
         - model_class_hint (inherit, fast, balanced, deep-reasoning, code, review, or tool-heavy)\n\
         - [instructions].text\n\
         - [tools].posture = \"read-only\"\n\n\
         Do not include provider, model, base_url, api_key, auth, secrets, trust, allow_shell, or approval_required=false.\n\
         Keep the profile permission-narrowing and compatible with recursive Fleet/sub-agent workers.",
        provider = snapshot.provider,
        model = snapshot.model,
        reasoning = snapshot.reasoning
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn snapshot() -> FleetSetupSnapshot {
        FleetSetupSnapshot {
            workspace: PathBuf::from("/tmp/codewhale-test-workspace"),
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
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn arrow_keys_move_across_lanes_and_rows() {
        let mut view = FleetSetupView::from_snapshot(snapshot());

        view.handle_key(key(KeyCode::Right));
        view.handle_key(key(KeyCode::Down));

        assert_eq!(view.selected_lane, 1);
        assert_eq!(view.selected_row(), 1);

        view.handle_key(key(KeyCode::Left));

        assert_eq!(view.selected_lane, 0);
        assert_eq!(view.selected_row(), 0);
    }

    #[test]
    fn enter_inserts_profile_authoring_prompt() {
        let mut view = FleetSetupView::from_snapshot(snapshot());

        let action = view.handle_key(key(KeyCode::Enter));

        match action {
            ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
                action: CommandPaletteAction::InsertText { text },
            }) => {
                assert!(text.contains("Target path: .codewhale/agents/reviewer.toml"));
                assert!(text.contains("provider = DeepSeek"));
                assert!(text.contains("Do not include provider, model, base_url"));
            }
            other => panic!("expected profile prompt insertion, got {other:?}"),
        }
    }

    #[test]
    fn profile_prompt_uses_current_route_only_as_context() {
        let view = FleetSetupView::from_snapshot(snapshot());

        assert!(view.profile_prompt().contains("Current route context only"));
        assert!(view.profile_prompt().contains("permission-narrowing"));
    }

    #[test]
    fn render_mentions_fleet_to_agent_bridge_and_recursion() {
        let view = FleetSetupView::from_snapshot(snapshot());
        let mut buf = Buffer::empty(Rect::new(0, 0, 120, 32));

        assert!(
            view.lanes
                .iter()
                .flat_map(|lane| lane.rows.iter())
                .any(|row| row.value == "Fleet -> agent")
        );
        view.render(Rect::new(0, 0, 120, 32), &mut buf);
        let rendered = buf
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Fleet"));
        assert!(rendered.contains("recursion"));
    }
}
