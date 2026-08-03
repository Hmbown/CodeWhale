//! Modal for request_user_input tool prompts.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

use crate::palette;
use crate::tools::user_input::{
    UserInputAnswer, UserInputQuestion, UserInputRequest, UserInputResponse,
};
use crate::tui::menu_style;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent, render_modal_surface};

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            title.to_string(),
            Style::default().fg(palette::WHALE_HUMAN).bold(),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .style(Style::default().bg(palette::WHALE_BG))
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
        menu_style::selected_row_style()
    } else {
        Style::default().fg(palette::TEXT_PRIMARY)
    };
    let detail_style = if selected {
        row_style
    } else {
        Style::default().fg(palette::TEXT_MUTED)
    };
    let prefix = crate::tui::glyphs::selection_marker(selected);
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
    /// Free text is ALWAYS available so the user can answer with their own
    /// words even when the model did not offer it. `allow_free_text` remains
    /// part of the wire request (backward-compatible) but no longer gates the
    /// row: a custom response must always be reachable alongside the options.
    fn offers_other(&self) -> bool {
        true
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
        self.confirm_index() == Some(self.selected)
    }

    fn confirm_index(&self) -> Option<usize> {
        self.is_multi_select()
            .then(|| self.option_count().saturating_sub(1))
    }

    fn is_multi_select(&self) -> bool {
        self.current_question().multi_select
    }

    /// Number of content lines the render path emits for the current state.
    /// Drives the content-sized popup height so the dialog hugs what it
    /// shows instead of claiming a fixed share of the screen.
    fn content_line_count(&self) -> usize {
        let question = self.current_question();
        // "Action required" banner, header line, blank, question, blank.
        let mut count = 5;
        count += question.options.len() * 2;
        if self.offers_other() {
            count += 2;
        }
        if self.is_multi_select() {
            count += 2;
        }
        if self.mode == InputMode::OtherInput {
            count += 2;
        }
        // Trailing blank line + controls hint.
        count += 2;
        count
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
                // The Other row and the Confirm row are not options: toggling
                // them would corrupt the pending set.
                let is_confirm = self.confirm_index() == Some(self.selected);
                if !self.is_other_selected() && !is_confirm {
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
    /// - multi-select option → add to the pending set (never remove — that is
    ///   Space's job) and move focus to the Confirm row, so the single-select
    ///   muscle memory of Enter-then-Enter submits the highlighted option
    ///   instead of toggling it back out and submitting an empty set.
    /// - multi-select Confirm row → submit the pending set.
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
            // Enter on a real option selects it and moves to Confirm. It
            // never toggles out: double-Enter must submit the highlighted
            // option, matching single-select on the same view.
            if !self.multi_pending.contains(&self.selected) {
                self.multi_pending.push(self.selected);
            }
            if let Some(confirm) = self.confirm_index() {
                self.selected = confirm;
            }
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
            Style::default().fg(palette::WHALE_INFO).bold(),
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
        if let Some(confirm_index) = self.confirm_index() {
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
                    Style::default().fg(palette::WHALE_HUMAN),
                ),
            ]));
        }

        lines.push(Line::from(""));
        if self.mode == InputMode::OtherInput {
            lines.push(Line::from(vec![
                Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled(" submit", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("  "),
                Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
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
                        Style::default().fg(palette::WHALE_INFO).bold(),
                    ),
                    Span::styled(" move", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Space", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" toggle", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" select/confirm", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" cancel", Style::default().fg(palette::TEXT_MUTED)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(
                        quick_pick_label,
                        Style::default().fg(palette::WHALE_INFO).bold(),
                    ),
                    Span::styled(" quick pick", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("↑/↓", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" move", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" confirm", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw("  "),
                    Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
                    Span::styled(" cancel", Style::default().fg(palette::TEXT_MUTED)),
                ]));
            }
        }

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true })
            .block(modal_block(&header));

        let popup_area = compact_popup_rect(area, self.content_line_count());
        render_modal_chrome(area, popup_area, buf);
        paragraph.render(popup_area, buf);
    }

    fn occupied_region(&self, area: Rect) -> Rect {
        // The dialog only occupies its compact centered card; blanking the
        // whole frame (the default) hid the live conversation the user is
        // being asked about (v0.9.4, FINISH-0.9.4 #13). Cover the card plus
        // the one-cell drop shadow `render_modal_surface` draws at +1/+1.
        let popup = compact_popup_rect(area, self.content_line_count());
        Rect {
            x: popup.x,
            y: popup.y,
            width: (popup.width.saturating_add(1)).min(area.right().saturating_sub(popup.x)),
            height: (popup.height.saturating_add(1)).min(area.bottom().saturating_sub(popup.y)),
        }
    }
}

/// Compact centered overlay: bounded width (max 110 columns) and a height
/// sized to the content (border + padding around `content_lines`, never more
/// than 22 rows or 60% of the screen) so the live conversation stays visible
/// behind the modal instead of being covered edge-to-edge.
fn compact_popup_rect(r: Rect, content_lines: usize) -> Rect {
    let width = r.width.min(110);
    // Border (2 rows) + uniform padding (2 rows) around the content lines.
    let desired = u16::try_from(content_lines)
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let height = desired
        .clamp(6, 22)
        .min((r.height.saturating_mul(60) / 100).clamp(6, 22))
        .min(r.height);
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(width),
            Constraint::Min(0),
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
    fn user_input_modal_keeps_other_row_when_free_text_disabled() {
        // v0.9.4: a custom free-text response is ALWAYS available alongside
        // the options, even when the model did not offer it. The wire field
        // `allow_free_text` stays for backward compatibility but no longer
        // gates the row (#3102 originally hid it).
        let mut view = sample_view();
        view.request.questions[0].allow_free_text = false;
        view.selected = 0;

        let rendered = render_view(&view, 110, 36);
        assert!(
            rendered.contains("Type a custom response"),
            "Other row must stay reachable even when allow_free_text is false"
        );
        assert!(rendered.contains("Other"));

        // Entering the row switches to free-text input mode regardless.
        view.selected = view.option_count() - 1;
        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.mode, InputMode::OtherInput);
    }

    #[test]
    fn user_input_modal_renders_multi_select_ticks_and_confirm() {
        // Issue #3102: multi_select=true renders a check-mark gutter on
        // toggled options plus a trailing "Confirm selection" row, and the
        // controls hint advertises Space/Enter toggle semantics. With the
        // v0.9.4 always-available Other row, confirm sits at index 3.
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
        assert!(
            rendered.contains("▸  4) Confirm selection"),
            "confirm row should display selected focus at its real quick-pick index"
        );
        assert!(
            !rendered.contains("5) Confirm selection"),
            "confirm row must not advertise an unreachable quick-pick number"
        );
    }

    #[test]
    fn user_input_modal_space_toggles_and_enter_confirms_multi_select() {
        // Keyboard-first multi-select: Space toggles the highlighted option
        // into the pending set without leaving the picker; Enter on the
        // confirm row flushes the set.
        let mut view = sample_view();
        view.request.questions[0].multi_select = true;
        view.selected = 0;

        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.multi_pending, vec![0], "Space toggles option 0 in");

        let _action = view.handle_selecting_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(view.multi_pending.is_empty(), "Space toggles option 0 out");

        // Space on the confirm row must not toggle it (it is not an option).
        view.selected = view.confirm_index().expect("confirm row present");
        let before = view.multi_pending.clone();
        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.multi_pending, before, "Space on confirm is a no-op");

        // Enter on the confirm row flushes the pending set as answers.
        view.multi_pending.push(0);
        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Enter));
        assert!(
            matches!(action, ViewAction::EmitAndClose(ViewEvent::UserInputSubmitted { tool_id, response })
                if tool_id == "tool-1" && response.answers.first().is_some_and(|a| a.value == "Ship it")),
            "Enter on confirm submits the toggled options"
        );
    }

    #[test]
    fn user_input_modal_double_enter_never_submits_empty_multi_select() {
        // Enter on a multi-select option used to toggle it into the pending
        // set, so a second Enter (single-select muscle memory) toggled it
        // back out — and Confirm then submitted an empty answer set.
        let mut view = sample_view();
        view.request.questions[0].multi_select = true;
        view.selected = 0;

        // First Enter: option 0 joins the pending set, focus moves to Confirm.
        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(
            view.multi_pending,
            vec![0],
            "Enter selects the highlighted option"
        );
        assert!(
            view.is_confirm_selected(),
            "focus moves to the Confirm row after Enter"
        );

        // Second Enter submits exactly that option — never an empty set.
        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Enter));
        assert!(
            matches!(action, ViewAction::EmitAndClose(ViewEvent::UserInputSubmitted { tool_id, response })
                if tool_id == "tool-1"
                    && response.answers.len() == 1
                    && response.answers[0].value == "Ship it"),
            "double-Enter must submit the highlighted option, not an empty set"
        );
    }

    #[test]
    fn user_input_modal_enter_never_deselects_multi_select_option() {
        // Deselecting remains Space's job: Enter on an already-toggled option
        // keeps it in the pending set.
        let mut view = sample_view();
        view.request.questions[0].multi_select = true;
        view.selected = 0;

        let _ = view.handle_selecting_key(KeyEvent::from(KeyCode::Char(' ')));
        assert_eq!(view.multi_pending, vec![0], "Space toggles option 0 in");

        let action = view.handle_selecting_key(KeyEvent::from(KeyCode::Enter));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(
            view.multi_pending,
            vec![0],
            "Enter must not toggle the option back out"
        );

        // Space still toggles both ways.
        view.selected = 0;
        let _ = view.handle_selecting_key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(view.multi_pending.is_empty(), "Space toggles option 0 out");
    }

    #[test]
    fn user_input_modal_popup_is_centered_and_sized_to_content() {
        let area = Rect::new(0, 0, 120, 40);
        let view = sample_view();
        let content = view.content_line_count();
        let popup = compact_popup_rect(area, content);

        // Height hugs the content (border + padding = 4 chrome rows), well
        // under the 22-row / 60% cap for a 40-row screen.
        assert_eq!(popup.height, u16::try_from(content).unwrap() + 4);
        assert!(popup.height < area.height / 2);
        assert_eq!(popup.width, 110);
        // Centered: breathing room above and below.
        assert!(popup.y > 0);
        assert!(popup.y + popup.height < area.height);

        // Long content is still bounded by the 22-row cap.
        let capped = compact_popup_rect(area, 100);
        assert_eq!(capped.height, 22);
    }

    #[test]
    fn user_input_modal_occupied_region_matches_painted_card_plus_shadow() {
        let area = Rect::new(0, 0, 120, 40);
        let view = sample_view();
        let popup = compact_popup_rect(area, view.content_line_count());
        let occupied = view.occupied_region(area);

        assert_eq!(occupied.x, popup.x);
        assert_eq!(occupied.y, popup.y);
        assert_eq!(occupied.width, popup.width + 1);
        assert_eq!(occupied.height, popup.height + 1);
        assert!(area.right() >= occupied.right());
        assert!(area.bottom() >= occupied.bottom());
    }

    #[test]
    fn user_input_modal_leaves_surrounding_frame_visible() {
        use crate::tui::views::ViewStack;

        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        // Pre-fill the frame as if the live transcript had painted it.
        for y in 0..area.height {
            for x in 0..area.width {
                buf[(x, y)].set_symbol("·");
            }
        }

        let mut stack = ViewStack::default();
        stack.push(sample_view());
        stack.render(area, &mut buf);

        // Cells outside the compact card survive untouched: the conversation
        // stays visible around the dialog (FINISH-0.9.4 #13).
        assert_eq!(buf[(0, 0)].symbol(), "·");
        assert_eq!(buf[(119, 0)].symbol(), "·");
        assert_eq!(buf[(0, 39)].symbol(), "·");
        assert_eq!(buf[(119, 39)].symbol(), "·");
        assert_eq!(buf[(60, 0)].symbol(), "·");
        assert_eq!(buf[(60, 39)].symbol(), "·");
        // The card itself is blanked + repainted by the modal surface.
        assert_ne!(buf[(60, 20)].symbol(), "·");
    }

    #[test]
    fn user_input_modal_numbers_confirm_after_other_row() {
        let mut view = sample_view();
        view.request.questions[0].multi_select = true;
        view.request.questions[0].allow_free_text = true;
        view.selected = view.option_count() - 1;

        let rendered = render_view(&view, 120, 40);
        assert!(rendered.contains("3) Other"));
        assert!(
            rendered.contains("▸  4) Confirm selection"),
            "confirm should follow the optional Other row with selected focus"
        );
        assert!(!rendered.contains("5) Confirm selection"));
    }
}
