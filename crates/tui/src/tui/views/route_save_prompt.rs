//! Route-save decision prompt.
//!
//! A `/model` or `/provider` change is temporary by default. This prompt is
//! the explicit choice: update the selected Fleet, save as a new Fleet,
//! remember as the default (no Fleet selected), or keep for this session
//! only. Nothing is written until the user picks; Esc is the same as
//! "session only".

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear, Padding, Paragraph, Widget, Wrap},
};

use crate::palette;
use crate::tui::app::PendingRouteSave;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer_with_gutter, render_modal_surface,
};

/// The explicit persistence choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteSaveChoice {
    /// Rewrite the selected Fleet's operator route to the session route.
    UpdateFleet,
    /// Save the session route as a brand-new Fleet (user-global) and select it.
    SaveAsNewFleet,
    /// Remember the session route as the startup default (settings; only
    /// offered when no Fleet is selected).
    SaveAsDefault,
    /// Write nothing; the change lives for this session only.
    SessionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChoiceRow {
    UpdateFleet,
    SaveAsNew,
    SaveAsDefault,
    SessionOnly,
}

pub struct RouteSavePromptView {
    pending: PendingRouteSave,
    fleet_selected: bool,
    rows: Vec<ChoiceRow>,
    selected: usize,
}

impl RouteSavePromptView {
    #[must_use]
    pub fn new(pending: PendingRouteSave) -> Self {
        let fleet_selected = pending.fleet.is_some();
        let mut rows = Vec::new();
        if fleet_selected {
            rows.push(ChoiceRow::UpdateFleet);
        }
        rows.push(ChoiceRow::SaveAsNew);
        if !fleet_selected {
            rows.push(ChoiceRow::SaveAsDefault);
        }
        rows.push(ChoiceRow::SessionOnly);
        Self {
            pending,
            fleet_selected,
            rows,
            selected: 0,
        }
    }

    fn footer_hints(&self) -> Vec<ActionHint> {
        vec![
            ActionHint::new("↑/↓", "move"),
            ActionHint::new("Enter", "choose"),
            ActionHint::new("Esc", "session only"),
        ]
    }

    fn move_row(&mut self, delta: isize) {
        self.selected = crate::tui::list_nav::wrap_index(self.selected, self.rows.len(), delta);
    }

    fn chosen(&self) -> RouteSaveChoice {
        match self.rows[self.selected.min(self.rows.len().saturating_sub(1))] {
            ChoiceRow::UpdateFleet => RouteSaveChoice::UpdateFleet,
            ChoiceRow::SaveAsNew => RouteSaveChoice::SaveAsNewFleet,
            ChoiceRow::SaveAsDefault => RouteSaveChoice::SaveAsDefault,
            ChoiceRow::SessionOnly => RouteSaveChoice::SessionOnly,
        }
    }
}

impl ModalView for RouteSavePromptView {
    fn kind(&self) -> ModalKind {
        ModalKind::RouteSavePrompt
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                ViewAction::EmitAndClose(ViewEvent::RouteSaveDecision {
                    choice: RouteSaveChoice::SessionOnly,
                })
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_row(-1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_row(1);
                ViewAction::None
            }
            KeyCode::Enter => ViewAction::EmitAndClose(ViewEvent::RouteSaveDecision {
                choice: self.chosen(),
            }),
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            // Rows start three lines below the popup top; approximate hit
            // testing is enough for a three-row decision card.
            let row_start = 4u16;
            if mouse.row >= row_start {
                let idx = usize::from(mouse.row - row_start);
                if idx < self.rows.len() {
                    self.selected = idx;
                    return ViewAction::EmitAndClose(ViewEvent::RouteSaveDecision {
                        choice: self.chosen(),
                    });
                }
            }
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup = centered_modal_area(area, 72, 9 + self.rows.len() as u16, 56, 8);
        render_modal_surface(area, popup, buf);
        let block = Block::default()
            .title(Line::from(Span::styled(
                " Save this route? ",
                Style::default().fg(palette::WHALE_ACTION).bold(),
            )))
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));
        let inner = block.inner(popup);
        block.render(popup, buf);

        let hints = self.footer_hints();
        let content = render_modal_footer_with_gutter(inner, buf, &hints);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(content);

        Paragraph::new(vec![
            Line::from(Span::styled(
                format!(
                    "  Route changed to {} — this is temporary until you choose.",
                    self.pending_label()
                ),
                Style::default().fg(palette::TEXT_SECONDARY),
            )),
            Line::from(""),
        ])
        .wrap(Wrap { trim: false })
        .render(chunks[0], buf);

        let mut lines = Vec::new();
        for (idx, row) in self.rows.iter().enumerate() {
            let selected = idx == self.selected;
            let base = if selected {
                Style::default().fg(palette::WHALE_ACTION).bold()
            } else {
                Style::default().fg(palette::TEXT_SECONDARY)
            };
            let (label, note) = match row {
                ChoiceRow::UpdateFleet => (
                    "Update this Fleet".to_string(),
                    format!(
                        "rewrite `{}`'s operator route",
                        self.pending.fleet_label().unwrap_or("")
                    ),
                ),
                ChoiceRow::SaveAsNew => (
                    "Save as a new Fleet".to_string(),
                    "user-global, then selected for new sessions".to_string(),
                ),
                ChoiceRow::SaveAsDefault => (
                    "Remember as my default".to_string(),
                    "startup default in settings.toml".to_string(),
                ),
                ChoiceRow::SessionOnly => (
                    "Keep for this session only".to_string(),
                    "nothing is written".to_string(),
                ),
            };
            lines.push(Line::from(vec![
                Span::styled(if selected { "» " } else { "  " }, base),
                Span::styled(label.to_string(), base),
                Span::styled(
                    format!("  — {note}"),
                    Style::default().fg(palette::TEXT_DIM),
                ),
            ]));
        }
        Paragraph::new(ratatui::text::Text::from(lines)).render(chunks[1], buf);
    }
}

impl RouteSavePromptView {
    fn pending_label(&self) -> String {
        format!("{}/{}", self.pending.provider_identity, self.pending.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::PendingRouteSave;

    fn pending(fleet: Option<(&str, crate::fleet::store::FleetScope)>) -> PendingRouteSave {
        PendingRouteSave {
            provider_identity: "deepseek".to_string(),
            model: "deepseek-v4-pro".to_string(),
            fleet: fleet.map(|(name, scope)| (name.to_string(), scope)),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn fleet_selected_offers_update_save_new_and_session_only() {
        let view = RouteSavePromptView::new(pending(Some((
            "DeepSeek Flash",
            crate::fleet::store::FleetScope::Personal,
        ))));
        let labels: Vec<&str> = view
            .rows
            .iter()
            .map(|r| match r {
                ChoiceRow::UpdateFleet => "Update this Fleet",
                ChoiceRow::SaveAsNew => "Save as a new Fleet",
                ChoiceRow::SaveAsDefault => "Remember as my default",
                ChoiceRow::SessionOnly => "Keep for this session only",
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                "Update this Fleet",
                "Save as a new Fleet",
                "Keep for this session only"
            ]
        );
    }

    #[test]
    fn no_fleet_offers_save_new_default_and_session_only() {
        let view = RouteSavePromptView::new(pending(None));
        let labels: Vec<&str> = view
            .rows
            .iter()
            .map(|r| match r {
                ChoiceRow::UpdateFleet => "Update this Fleet",
                ChoiceRow::SaveAsNew => "Save as a new Fleet",
                ChoiceRow::SaveAsDefault => "Remember as my default",
                ChoiceRow::SessionOnly => "Keep for this session only",
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                "Save as a new Fleet",
                "Remember as my default",
                "Keep for this session only"
            ]
        );
    }

    #[test]
    fn enter_emits_the_chosen_decision_and_esc_is_session_only() {
        let mut view = RouteSavePromptView::new(pending(Some((
            "DeepSeek Flash",
            crate::fleet::store::FleetScope::Personal,
        ))));
        let ViewAction::EmitAndClose(ViewEvent::RouteSaveDecision { choice }) =
            view.handle_key(key(KeyCode::Enter))
        else {
            panic!("expected RouteSaveDecision");
        };
        assert_eq!(choice, RouteSaveChoice::UpdateFleet);

        // Navigate to the last row (session only) and confirm via Esc.
        view.handle_key(key(KeyCode::Down));
        view.handle_key(key(KeyCode::Down));
        let ViewAction::EmitAndClose(ViewEvent::RouteSaveDecision { choice }) =
            view.handle_key(key(KeyCode::Esc))
        else {
            panic!("expected RouteSaveDecision from Esc");
        };
        assert_eq!(choice, RouteSaveChoice::SessionOnly);
    }

    #[test]
    fn session_only_choice_never_offered_as_first_row_when_fleet_exists() {
        // The safest default must not be the accidental Enter. With a Fleet
        // selected, Enter applies the update — the user has to move to
        // session-only deliberately.
        let view = RouteSavePromptView::new(pending(Some((
            "F",
            crate::fleet::store::FleetScope::Workspace,
        ))));
        assert_eq!(view.rows[0], ChoiceRow::UpdateFleet);
    }
}
