//! Modal for request_user_input tool prompts.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

use crate::palette;
use crate::tools::user_input::{
    UserInputAnswer, UserInputQuestion, UserInputRequest, UserInputResponse,
};
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent, render_modal_surface};

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            title.to_string(),
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .style(Style::default().bg(palette::DEEPSEEK_INK))
        .padding(Padding::uniform(1))
}

fn render_modal_chrome(area: Rect, popup_area: Rect, buf: &mut Buffer) {
    render_modal_surface(area, popup_area, buf);
}

fn push_option_lines(
    lines: &mut Vec<Line<'static>>,
    selected: bool,
    number: usize,
    label: String,
    description: String,
    ticked: bool,
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
    // Multi-select rows get a check-mark gutter when toggled into the pending
    // set, mirroring the affordance used in other multi-option pickers.
    let mark = if ticked { "✔ " } else { "  " };

    lines.push(Line::from(Span::styled(
        format!("{prefix}{mark}{number}) {label}"),
        row_style,
    )));
    lines.push(Line::from(Span::styled(
        format!("      {description}"),
        detail_style,
    )));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputMode {
    Selecting,
    OtherInput,
}

#[derive(Debug, Clone)]
pub struct UserInputView {
    tool_id: String,
    request: UserInputRequest,
    question_index: usize,
    selected: usize,
    mode: InputMode,
    other_input: String,
    answers: Vec<UserInputAnswer>,
    /// Indices toggled into the pending multi-select set for the current
    /// question. Only used when `question.multi_select` is true.
    multi_pending: Vec<usize>,
}

impl UserInputView {
    pub fn new(tool_id: impl Into<String>, request: UserInputRequest) -> Self {
        Self {
            tool_id: tool_id.into(),
            request,
            question_index: 0,
            selected: 0,
            mode: InputMode::Selecting,
            other_input: String::new(),
            answers: Vec::new(),
            multi_pending: Vec::new(),
        }
    }

    fn current_question(&self) -> &UserInputQuestion {
        &self.request.questions[self.question_index]
    }

    /// Whether the "Other" free-text row is offered for the current question.
    fn offers_other(&self) -> bool {
        self.current_question().allow_free_text
    }

    fn option_count(&self) -> usize {
        // Options + conditional "Other" row + conditional "Confirm" row.
        let mut count = self.current_question().options.len();
        count += usize::from(self.offers_other());
        count += usize::from(self.is_multi_select());
        count
    }

    fn is_other_selected(&self) -> bool {
        // "Other" sits immediately before the Confirm row when both exist, and
        // is last otherwise.
        let other_last = !self.is_multi_select();
        if other_last {
            self.offers_other() && self.selected + 1 == self.option_count()
        } else {
            self.offers_other() && self.selected + 2 == self.option_count()
        }
    }

    /// True when the multi-select "Confirm selection" row is highlighted.
    fn is_confirm_selected(&self) -> bool {
        self.is_multi_select() && self.selected + 1 == self.option_count()
    }

    fn is_multi_select(&self) -> bool {
        self.current_question().multi_select
    }

    fn toggle_pending(&mut self, index: usize) {
        if let Some(pos) = self.multi_pending.iter().position(|i| *i == index) {
            self.multi_pending.remove(pos);
        } else {
            self.multi_pending.push(index);
        }
    }

    /// Build the answer(s) for the current question from a single selected
    /// option index (single-select and the confirm step of multi-select).
    fn answers_for_selection(&self, index: usize) -> Vec<UserInputAnswer> {
        let question = self.current_question();
        let option = &question.options[index];
        vec![UserInputAnswer {
            id: question.id.clone(),
            label: option.label.clone(),
            value: option.label.clone(),
        }]
    }

    fn advance_question(&mut self, new_answers: Vec<UserInputAnswer>) -> ViewAction {
        self.answers.extend(new_answers);
        if self.question_index + 1 >= self.request.questions.len() {
            let response = UserInputResponse {
                answers: self.answers.clone(),
            };
            return ViewAction::EmitAndClose(ViewEvent::UserInputSubmitted {
                tool_id: self.tool_id.clone(),
                response,
            });
        }
        self.question_index += 1;
        self.selected = 0;
        self.mode = InputMode::Selecting;
        self.other_input.clear();
        self.multi_pending.clear();
        ViewAction::None
    }

    fn handle_selecting_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(self.option_count().saturating_sub(1));
                ViewAction::None
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let Some(number) = ch.to_digit(10) else {
                    return ViewAction::None;
                };
                if number == 0 {
                    return ViewAction::None;
                }
                let index = usize::try_from(number - 1).unwrap_or(usize::MAX);
                if index >= self.option_count() {
                    return ViewAction::None;
                }
                self.selected = index;
                self.activate_or_confirm_selection()
            }
            KeyCode::Char(' ') if self.is_multi_select() => {
                // Space toggles the highlighted option in the pending set
                // without leaving the picker (standard multi-select affordance).
                if !self.is_other_selected() {
                    self.toggle_pending(self.selected);
                }
                ViewAction::None
            }
            KeyCode::Enter => self.activate_or_confirm_selection(),
            KeyCode::Esc => ViewAction::EmitAndClose(ViewEvent::UserInputCancelled {
                tool_id: self.tool_id.clone(),
            }),
            _ => ViewAction::None,
        }
    }

    /// Resolve a digit/Enter activation for the currently highlighted row.
    ///
    /// - "Other" row → enter free-text input mode.
    /// - multi-select option → toggle into the pending set (Enter confirms on
    ///   the dedicated "Confirm" step; here it just toggles, like Space).
    /// - single-select option → submit immediately (legacy behavior).
    fn activate_or_confirm_selection(&mut self) -> ViewAction {
        if self.is_other_selected() {
            self.mode = InputMode::OtherInput;
            self.other_input.clear();
            return ViewAction::None;
        }
        if self.is_multi_select() {
            if self.is_confirm_selected() {
                // Flush the pending set as this question's answers. An empty
                // set is allowed (skip-like) — the model is expected to offer a
                // sensible default, but we don't deadlock.
                let question = self.current_question();
                let answers: Vec<UserInputAnswer> = self
                    .multi_pending
                    .iter()
                    .filter_map(|i| question.options.get(*i))
                    .map(|opt| UserInputAnswer {
                        id: question.id.clone(),
                        label: opt.label.clone(),
                        value: opt.label.clone(),
                    })
                    .collect();
                return self.advance_question(answers);
            }
            // Enter/Space on a real option toggles it into the pending set.
            self.toggle_pending(self.selected);
            return ViewAction::None;
        }
        // Single-select: submit immediately.
        let answers = self.answers_for_selection(self.selected);
        self.advance_question(answers)
    }

    fn handle_other_input_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => {
                self.mode = InputMode::Selecting;
                self.other_input.clear();
                ViewAction::None
            }
            KeyCode::Enter => {
                let question = self.current_question();
                let answer = UserInputAnswer {
                    id: question.id.clone(),
                    label: "Other".to_string(),
                    value: self.other_input.trim().to_string(),
                };
                // In multi-select mode a free-text "Other" is still a single
                // answer appended to whatever options were toggled.
                let mut answers: Vec<UserInputAnswer> = self
                    .multi_pending
                    .iter()
                    .filter_map(|i| question.options.get(*i))
                    .map(|opt| UserInputAnswer {
                        id: question.id.clone(),
                        label: opt.label.clone(),
                        value: opt.label.clone(),
                    })
                    .collect();
                answers.push(answer);
                self.advance_question(answers)
            }
            KeyCode::Backspace => {
                self.other_input.pop();
                ViewAction::None
            }
            KeyCode::Char('h')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.other_input.pop();
                ViewAction::None
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.other_input.push(ch);
                }
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }
}

impl ModalView for UserInputView {
    fn kind(&self) -> ModalKind {
        ModalKind::UserInput
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.mode {
            InputMode::Selecting => self.handle_selecting_key(key),
            InputMode::OtherInput => self.handle_other_input_key(key),
        }
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> ViewAction {
        // Wheel moves the option highlight, mirroring Up/Down. Free-text
        // entry mode has nothing to scroll.
        if matches!(self.mode, InputMode::Selecting) {
            match mouse.kind {
                crossterm::event::MouseEventKind::ScrollUp => {
                    self.selected = self.selected.saturating_sub(1);
                }
                crossterm::event::MouseEventKind::ScrollDown => {
                    self.selected = (self.selected + 1).min(self.option_count().saturating_sub(1));
                }
                _ => {}
            }
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let question = self.current_question();
        let total = self.request.questions.len();
        let header = format!(
            " {} ({}/{}) ",
            question.header,
            self.question_index + 1,
            total
        );

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Action required",
            Style::default().fg(palette::DEEPSEEK_SKY).bold(),
        )]));
        lines.push(Line::from(vec![
            Span::styled(
                question.header.clone(),
                Style::default().fg(palette::TEXT_PRIMARY).bold(),
            ),
            Span::styled(
                format!("  Question {} of {}", self.question_index + 1, total),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            question.question.clone(),
            Style::default().fg(palette::TEXT_PRIMARY).bold(),
        )]));
        lines.push(Line::from(""));

        for (idx, option) in question.options.iter().enumerate() {
            let number = idx + 1;
            let ticked = self.is_multi_select() && self.multi_pending.contains(&idx);
            push_option_lines(
                &mut lines,
                self.selected == idx,
                number,
                option.label.clone(),
                option.description.clone(),
                ticked,
            );
        }

        // The free-text "Other" row is now conditional on allow_free_text.
        if self.offers_other() {
            let other_index = question.options.len();
            let other_number = other_index + 1;
            push_option_lines(
                &mut lines,
                self.selected == other_index,
                other_number,
                "Other".to_string(),
                "Type a custom response".to_string(),
                false,
            );
        }

        // Multi-select gets a dedicated "Confirm selection" row after the
        // options (and after "Other" when present). Selecting and pressing
        // Enter on it flushes the pending set as the question's answers.
        if self.is_multi_select() {
            let confirm_index = self.option_count();
            let confirm_number = confirm_index + 1;
            push_option_lines(
                &mut lines,
                self.selected == confirm_index,
                confirm_number,
                "Confirm selection".to_string(),
                format!("Submit {} selected", self.multi_pending.len()),
                false,
            );
        }

        if self.mode == InputMode::OtherInput {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "> Custom response:",
                    Style::default().fg(palette::TEXT_PRIMARY).bold(),
                ),
                Span::raw(" "),
                Span::styled(
                    if self.other_input.is_empty() {
                        "(type your response)".to_string()
                    } else {
                        self.other_input.clone()
                    },
                    Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
                ),
            ]));
        }

        lines.push(Line::from(""));
        if self.mode == InputMode::OtherInput {
            lines.push(Line::from(vec![
                Span::styled("Enter", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                Span::styled(" submit", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("  "),
                Span::styled("Esc", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                Span::styled(" back", Style::default().fg(palette::TEXT_MUTED)),
            ]));
        } else {
            let opt_count = self.option_count();
            let quick_pick_label = if opt_count <= 9 {
                format!("1-{opt_count}")
            } else {
                "digit".to_string()
            };
            if self.is_multi_select() {
                lines.push(Line::from(vec![
                    Span::styled(
                        quick_pick_label,
                        Style::default().fg(palette::DEEPSEEK_SKY).bold(),
                    ),
                    Span::styled(" move", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Space", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                    Span::styled(" toggle", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Enter", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                    Span::styled(" toggle/confirm", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Esc", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                    Span::styled(" cancel", Style::default().fg(palette::TEXT_MUTED)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        quick_pick_label,
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
                    Span::styled(" cancel", Style::default().fg(palette::TEXT_MUTED)),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true })
            .block(modal_block(&header));

        let popup_area = centered_rect(82, 68, area);
        render_modal_chrome(area, popup_area, buf);
        paragraph.render(popup_area, buf);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1]);
    horizontal[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::user_input::{UserInputOption, UserInputQuestion, UserInputRequest};

    fn render_view(view: &UserInputView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample_view() -> UserInputView {
        UserInputView::new(
            "tool-1",
            UserInputRequest {
                questions: vec![UserInputQuestion {
                    header: "Confirm".to_string(),
                    id: "confirm".to_string(),
                    question: "What should happen next?".to_string(),
                    options: vec![
                        UserInputOption {
                            label: "Ship it".to_string(),
                            description: "Proceed with the current change set".to_string(),
                        },
                        UserInputOption {
                            label: "Revise it".to_string(),
                            description: "Return to editing before continuing".to_string(),
                        },
                    ],
                    allow_free_text: true,
                    multi_select: false,
                }],
            },
        )
    }

    #[test]
    fn user_input_modal_calls_out_required_action_and_controls() {
        let rendered = render_view(&sample_view(), 110, 36);

        assert!(rendered.contains("Action required"));
        assert!(rendered.contains("Question 1 of 1"));
        assert!(rendered.contains("quick pick"));
        // allow_free_text=true surfaces the Other row.
        assert!(rendered.contains("Other"));
    }

    #[test]
    fn user_input_modal_renders_custom_response_state() {
        let mut view = sample_view();
        view.selected = 2;
        view.mode = InputMode::OtherInput;
        view.other_input = "Need one more pass".to_string();

        let rendered = render_view(&view, 110, 36);

        assert!(rendered.contains("Custom response"));
        assert!(rendered.contains("Need one more pass"));
        assert!(rendered.contains("Enter"));
        assert!(rendered.contains("submit"));
    }

    #[test]
    fn user_input_modal_hides_other_row_when_free_text_disabled() {
        // Issue #3102: allow_free_text=false must NOT render the hardcoded
        // "Other" pseudo-option. Previously "Other" was always appended.
        let mut view = sample_view();
        view.request.questions[0].allow_free_text = false;
        // Reset selection to a valid option index (no Other row to land on).
        view.selected = 0;

        let rendered = render_view(&view, 110, 36);
        assert!(
            !rendered.contains("Type a custom response"),
            "Other row should be hidden when allow_free_text is false"
        );
        assert!(!rendered.contains("\nOther\n"));
    }

    #[test]
    fn user_input_modal_renders_multi_select_ticks_and_confirm() {
        // Issue #3102: multi_select=true renders a check-mark gutter on
        // toggled options plus a trailing "Confirm selection" row, and the
        // controls hint advertises Space/Enter toggle semantics.
        let mut view = sample_view();
        view.request.questions[0].multi_select = true;
        view.request.questions[0].allow_free_text = false;
        // Toggle the first option into the pending set.
        view.multi_pending.push(0);
        // Highlight the confirm row (last selectable row).
        view.selected = view.option_count() - 1;

        let rendered = render_view(&view, 120, 40);
        assert!(rendered.contains("✔"), "toggled option shows a check mark");
        assert!(
            rendered.contains("Confirm selection"),
            "multi-select renders a confirm row"
        );
        assert!(rendered.contains("Submit 1 selected"));
        assert!(rendered.contains("toggle"));
    }
}
