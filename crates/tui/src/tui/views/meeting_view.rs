//! Meeting view for displaying multi-agent meeting state

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::tui::app::App;
use crate::tui::tab::meeting::MeetingMessageType;
use crate::tui::views::{ModalKind, ModalView, ViewAction};

/// Meeting view widget
/// Shows participants, messages, and decisions in a meeting
pub struct MeetingView {
    /// Meeting ID being displayed
    meeting_id: String,
    /// Currently selected message index (for navigation)
    message_cursor: usize,
    /// Which pane has focus
    focus: MeetingPane,
    /// New message draft
    draft: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeetingPane {
    Messages,
    Participants,
    Decisions,
}

impl MeetingView {
    /// Create a new meeting view
    pub fn new(meeting_id: String) -> Self {
        Self {
            meeting_id,
            message_cursor: 0,
            focus: MeetingPane::Messages,
            draft: String::new(),
        }
    }
}

impl ModalView for MeetingView {
    fn kind(&self) -> ModalKind {
        ModalKind::Meeting // Reuse meeting kind or add new
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ViewAction::Close
            }
            KeyCode::Tab => {
                // Cycle focus between panes
                self.focus = match self.focus {
                    MeetingPane::Messages => MeetingPane::Participants,
                    MeetingPane::Participants => MeetingPane::Decisions,
                    MeetingPane::Decisions => MeetingPane::Messages,
                };
                ViewAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.message_cursor > 0 {
                    self.message_cursor -= 1;
                }
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.message_cursor = self.message_cursor.saturating_add(1);
                ViewAction::None
            }
            KeyCode::Char(c) => {
                self.draft.push(c);
                ViewAction::None
            }
            KeyCode::Backspace => {
                self.draft.pop();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent) -> ViewAction {
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Border
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ratatui::style::Color::Magenta))
            .title(format!(" Meeting: {} ", self.meeting_id));
        block.render(area, buf);

        if area.height < 6 || area.width < 20 {
            return;
        }

        // Inner area
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );

        // Split: top pane (participants) | main area (messages) | bottom (decisions + input)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Participants header
                Constraint::Min(5),    // Messages
                Constraint::Length(5), // Decisions + input
            ])
            .split(inner);

        // Render participants pane
        self.render_participants(chunks[0], buf);

        // Render messages pane
        self.render_messages(chunks[1], buf);

        // Render decisions + input
        self.render_decisions_and_input(chunks[2], buf);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl MeetingView {
    fn render_participants(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Participants (Tab to switch) ");
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        block.render(area, buf);

        // We don't have app reference here, so show placeholder.
        // In a real implementation, take &App or use a callback.
        let placeholder = Line::from(vec![
            Span::styled("[Participants list - read from app]", Style::default().fg(ratatui::style::Color::DarkGray)),
        ]);
        buf.set_line(inner.x, inner.y, &placeholder, inner.width);
    }

    fn render_messages(&self, area: Rect, buf: &mut Buffer) {
        let title = format!(
            " Messages (focus: {}) ",
            match self.focus {
                MeetingPane::Messages => "Messages",
                MeetingPane::Participants => "Participants",
                MeetingPane::Decisions => "Decisions",
            }
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title);
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        block.render(area, buf);

        // Placeholder - real implementation would fetch from app
        let placeholder = Line::from(vec![Span::styled(
            "[Meeting messages - read from app]",
            Style::default().fg(ratatui::style::Color::DarkGray),
        )]);
        buf.set_line(inner.x, inner.y, &placeholder, inner.width);
    }

    fn render_decisions_and_input(&self, area: Rect, buf: &mut Buffer) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Decisions
                Constraint::Length(2), // Input
            ])
            .split(area);

        // Decisions
        let decisions_block = Block::default()
            .borders(Borders::ALL)
            .title(" Decisions ");
        let inner = Rect::new(
            chunks[0].x + 1,
            chunks[0].y + 1,
            chunks[0].width.saturating_sub(2),
            chunks[0].height.saturating_sub(2),
        );
        decisions_block.render(chunks[0], buf);

        let placeholder = Line::from(vec![Span::styled(
            "[Decisions log]",
            Style::default().fg(ratatui::style::Color::DarkGray),
        )]);
        buf.set_line(inner.x, inner.y, &placeholder, inner.width);

        // Input
        let input_block = Block::default()
            .borders(Borders::ALL)
            .title(" Your input (type to compose) ");
        let inner_input = Rect::new(
            chunks[1].x + 1,
            chunks[1].y + 1,
            chunks[1].width.saturating_sub(2),
            chunks[1].height.saturating_sub(2),
        );
        input_block.render(chunks[1], buf);

        let input_line = Line::from(vec![
            Span::styled("> ", Style::default().fg(ratatui::style::Color::Green)),
            Span::styled(
                if self.draft.is_empty() {
                    "(type to compose a message)".to_string()
                } else {
                    self.draft.clone()
                },
                if self.draft.is_empty() {
                    Style::default().fg(ratatui::style::Color::DarkGray)
                } else {
                    Style::default().fg(ratatui::style::Color::White)
                },
            ),
        ]);
        buf.set_line(inner_input.x, inner_input.y, &input_line, inner_input.width);
    }
}

/// Format a meeting message with type-based color
pub fn format_meeting_message(msg_type: MeetingMessageType) -> &'static str {
    match msg_type {
        MeetingMessageType::Regular => "",
        MeetingMessageType::Question => "[?]",
        MeetingMessageType::Answer => "[A]",
        MeetingMessageType::Proposal => "[P]",
        MeetingMessageType::Agreement => "[+]",
        MeetingMessageType::Objection => "[!]",
        MeetingMessageType::Summary => "[S]",
    }
}

/// Render a meeting summary (used in non-modal contexts)
pub fn render_meeting_summary(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    meeting_id: &str,
) {
    let title = if let Some(meeting) = app.tab_manager.meeting(meeting_id) {
        format!(" Meeting Summary: {} ", meeting.topic)
    } else {
        format!(" Meeting Summary: {} ", meeting_id)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title);
    block.render(area, buf);

    if let Some(meeting) = app.tab_manager.meeting(meeting_id) {
        if area.height < 4 {
            return;
        }
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let line = Line::from(vec![Span::styled(
            format!(
                "{} participants, {} messages, {} decisions",
                meeting.participants.len(),
                meeting.message_count(),
                meeting.decision_count()
            ),
            Style::default().fg(ratatui::style::Color::White),
        )]);
        buf.set_line(inner.x, inner.y, &line, inner.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_meeting_message() {
        assert_eq!(format_meeting_message(MeetingMessageType::Regular), "");
        assert_eq!(format_meeting_message(MeetingMessageType::Question), "[?]");
        assert_eq!(format_meeting_message(MeetingMessageType::Answer), "[A]");
        assert_eq!(format_meeting_message(MeetingMessageType::Proposal), "[P]");
        assert_eq!(format_meeting_message(MeetingMessageType::Agreement), "[+]");
        assert_eq!(format_meeting_message(MeetingMessageType::Objection), "[!]");
        assert_eq!(format_meeting_message(MeetingMessageType::Summary), "[S]");
    }

    #[test]
    fn test_meeting_view_creation() {
        let view = MeetingView::new("meeting_1".to_string());
        assert_eq!(view.meeting_id, "meeting_1");
        assert_eq!(view.message_cursor, 0);
        assert!(view.draft.is_empty());
    }

    #[test]
    fn test_meeting_view_navigation() {
        let mut view = MeetingView::new("test".to_string());

        // Test typing
        let key = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE);
        view.handle_key(key);
        let key = KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE);
        view.handle_key(key);
        assert_eq!(view.draft, "hi");

        // Test backspace
        let key = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        view.handle_key(key);
        assert_eq!(view.draft, "h");

        // Test tab focus cycling
        assert_eq!(view.focus, MeetingPane::Messages);
        let key = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        view.handle_key(key);
        assert_eq!(view.focus, MeetingPane::Participants);
    }
}
