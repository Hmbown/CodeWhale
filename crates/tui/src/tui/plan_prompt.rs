//! Modal prompt for selecting what to do after a plan is generated.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap};

use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

const PLAN_OPTIONS: [(&str, &str); 4] = [
    ("Accept plan", "Start implementation with approvals"),
    (
        "Accept plan (YOLO)",
        "Start implementation with auto-approval",
    ),
    ("Revise plan", "Ask follow-ups or request plan changes"),
    (
        "Exit to Agent",
        "Return to Agent mode without implementation",
    ),
];

fn modal_block() -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            " Plan Confirmation ",
            Style::default().fg(palette::DEEPSEEK_BLUE).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .padding(Padding::uniform(1))
}

fn render_modal_chrome(area: Rect, popup_area: Rect, buf: &mut Buffer) {
    let shadow_x = popup_area.x.saturating_add(1);
    let shadow_y = popup_area.y.saturating_add(1);
    let shadow_right = area.x.saturating_add(area.width);
    let shadow_bottom = area.y.saturating_add(area.height);
    let shadow_width = popup_area.width.min(shadow_right.saturating_sub(shadow_x));
    let shadow_height = popup_area
        .height
        .min(shadow_bottom.saturating_sub(shadow_y));

    if shadow_width > 0 && shadow_height > 0 {
        Block::default().render(
            Rect {
                x: shadow_x,
                y: shadow_y,
                width: shadow_width,
                height: shadow_height,
            },
            buf,
        );
    }

    Clear.render(popup_area, buf);
}

fn push_option_lines(
    lines: &mut Vec<Line<'static>>,
    selected: bool,
    number: usize,
    label: &str,
    description: &str,
) {
    let row_style = if selected {
        Style::default()
            .fg(palette::SELECTION_TEXT)
            .bg(palette::SELECTION_BG)
            .bold()
    } else {
        Style::default().fg(palette::TEXT_PRIMARY)
    };
    let detail_style = if selected {
        row_style
    } else {
        Style::default().fg(palette::TEXT_MUTED)
    };
    let prefix = if selected { ">" } else { " " };

    lines.push(Line::from(Span::styled(
        format!("{prefix} {number}) {label}"),
        row_style,
    )));
    lines.push(Line::from(Span::styled(
        format!("    {description}"),
        detail_style,
    )));
}

#[derive(Debug, Clone, Default)]
pub struct PlanPromptView {
    selected: usize,
}

impl PlanPromptView {
    pub fn new() -> Self {
        Self::default()
    }

    fn max_index(&self) -> usize {
        PLAN_OPTIONS.len().saturating_sub(1)
    }

    fn submit_selected(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::PlanPromptSelected {
            option: self.selected + 1,
        })
    }

    fn submit_number(number: u32) -> ViewAction {
        if (1..=u32::try_from(PLAN_OPTIONS.len()).unwrap_or(0)).contains(&number) {
            ViewAction::EmitAndClose(ViewEvent::PlanPromptSelected {
                option: usize::try_from(number).unwrap_or(1),
            })
        } else {
            ViewAction::None
        }
    }
}

impl ModalView for PlanPromptView {
    fn kind(&self) -> ModalKind {
        ModalKind::PlanPrompt
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.max_index());
                ViewAction::None
            }
            KeyCode::Char('1') => {
                self.selected = 0;
                self.submit_selected()
            }
            KeyCode::Char('2') => {
                self.selected = 1;
                self.submit_selected()
            }
            KeyCode::Char('3') => {
                self.selected = 2;
                self.submit_selected()
            }
            KeyCode::Char('4') => {
                self.selected = 3;
                self.submit_selected()
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.selected = 0;
                self.submit_selected()
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.selected = 1;
                self.submit_selected()
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.selected = 2;
                self.submit_selected()
            }
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('e') | KeyCode::Char('E') => {
                self.selected = 3;
                self.submit_selected()
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let number = ch.to_digit(10).unwrap_or(0);
                Self::submit_number(number)
            }
            KeyCode::Enter => self.submit_selected(),
            KeyCode::Esc => ViewAction::EmitAndClose(ViewEvent::PlanPromptDismissed),
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Action required",
            Style::default().fg(palette::DEEPSEEK_SKY).bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Choose what should happen after this plan.",
            Style::default().fg(palette::TEXT_PRIMARY).bold(),
        )]));
        lines.push(Line::from(""));

        for (idx, (label, description)) in PLAN_OPTIONS.iter().enumerate() {
            let number = idx + 1;
            push_option_lines(&mut lines, self.selected == idx, number, label, description);
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "1-4 / a / y / r / q",
                Style::default().fg(palette::DEEPSEEK_SKY).bold(),
            ),
            Span::styled(" quick pick", Style::default().fg(palette::TEXT_MUTED)),
            Span::raw("  "),
            Span::styled("Up/Down", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
            Span::styled(" move", Style::default().fg(palette::TEXT_MUTED)),
            Span::raw("  "),
            Span::styled("Enter", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
            Span::styled(" confirm", Style::default().fg(palette::TEXT_MUTED)),
            Span::raw("  "),
            Span::styled("Esc", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
            Span::styled(" close", Style::default().fg(palette::TEXT_MUTED)),
        ]));

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true })
            .block(modal_block());

        let popup_area = centered_rect(72, 52, area);
        render_modal_chrome(area, popup_area, buf);
        paragraph.render(popup_area, buf);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_view(view: &PlanPromptView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn plan_prompt_calls_out_required_action_and_controls() {
        let rendered = render_view(&PlanPromptView::new(), 110, 36);

        assert!(rendered.contains("Action required"));
        assert!(rendered.contains("Choose what should happen after this plan."));
        assert!(rendered.contains("1-4"));
        assert!(rendered.contains("Enter"));
    }

    #[test]
    fn plan_prompt_keeps_selected_option_and_description_together() {
        let mut view = PlanPromptView::new();
        view.selected = 1;

        let rendered = render_view(&view, 110, 36);

        assert!(rendered.contains("> 2) Accept plan (YOLO)"));
        assert!(rendered.contains("Start implementation with auto-approval"));
    }
}
