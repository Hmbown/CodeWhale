//! Modal prompt for selecting what to do after a plan is generated.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::palette;
use crate::tools::plan::PlanSnapshot;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

const PLAN_OPTIONS: [(&str, &str); 4] = [
    (
        "Accept plan (Agent)",
        "Start implementation in Agent mode with approvals",
    ),
    (
        "Accept plan (YOLO)",
        "Start implementation in YOLO mode (auto-approve)",
    ),
    ("Revise plan", "Ask follow-ups or request plan changes"),
    (
        "Exit Plan mode",
        "Return to Agent mode without implementation",
    ),
];

/// Shortcut letters for each plan option (used in the footer bar).
const PLAN_SHORTCUTS: [char; 4] = ['a', 'y', 'r', 'q'];

/// Compact labels for each plan option (used in the footer bar).
const PLAN_SHORT_LABELS: [&str; 4] = ["Accept", "YOLO", "Revise", "Exit"];

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
    /// Vertical scroll position (in lines).
    scroll: usize,
    /// Tracks a previous 'g' press for the 'gg' (jump to top) combo.
    pending_g: bool,
    /// When true, an "are you sure?" prompt is shown instead of the option list
    /// because the user pressed Esc after scrolling away from the top.
    confirming_exit: bool,
    /// The plan snapshot to display (if update_plan was called).
    plan: Option<PlanSnapshot>,
}

impl PlanPromptView {
    pub fn new(plan: Option<PlanSnapshot>) -> Self {
        Self {
            selected: 0,
            scroll: 0,
            pending_g: false,
            confirming_exit: false,
            plan,
        }
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
                option: usize::from(number),
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
        // When the "confirm exit" prompt is active, only y / n / Esc matter.
        if self.confirming_exit {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    ViewAction::EmitAndClose(ViewEvent::PlanPromptDismissed)
                }
                KeyCode::Char('n')
                | KeyCode::Char('N')
                | KeyCode::Esc => {
                    self.confirming_exit = false;
                    ViewAction::None
                }
                _ => ViewAction::None,
            };
        }
        // Clear a pending 'g' when any other key is pressed so the gg combo
        // doesn't fire on a stray g followed by, say, an up-arrow 30 s later.
        let is_g = matches!(key.code, KeyCode::Char('g'));
        if self.pending_g && !is_g {
            self.pending_g = false;
        }
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
            KeyCode::Char('q')
            | KeyCode::Char('Q')
            | KeyCode::Char('e')
            | KeyCode::Char('E') => {
                self.selected = 3;
                self.submit_selected()
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let number = ch.to_digit(10).unwrap_or(0);
                Self::submit_number(number)
            }
            KeyCode::Enter => self.submit_selected(),
            KeyCode::Esc => {
                if self.confirming_exit {
                    // Second Esc: cancel the confirmation prompt.
                    self.confirming_exit = false;
                    ViewAction::None
                } else if self.scroll > 0 {
                    // User scrolled; ask for confirmation before discarding.
                    self.confirming_exit = true;
                    ViewAction::None
                } else {
                    ViewAction::EmitAndClose(ViewEvent::PlanPromptDismissed)
                }
            }
            // Scroll the plan content when it overflows the popup.
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(6);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(6);
                ViewAction::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll = self.scroll.saturating_sub(6);
                ViewAction::None
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll = self.scroll.saturating_add(6);
                ViewAction::None
            }
            // Vim-style scroll keys.
            KeyCode::Char('g') if self.pending_g => {
                // Second g in sequence → jump to top.
                self.scroll = 0;
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char('g') => {
                self.pending_g = true;
                ViewAction::None
            }
            KeyCode::Char('G') => {
                // Vim 'G' (Shift+g) → jump to bottom.  Render clamps overshoot.
                self.scroll = usize::MAX;
                ViewAction::None
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll = self.scroll.saturating_add(6);
                ViewAction::None
            }
            KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll = self.scroll.saturating_sub(6);
                ViewAction::None
            }
            KeyCode::Home => {
                self.scroll = 0;
                ViewAction::None
            }
            KeyCode::End => {
                self.scroll = usize::MAX;
                ViewAction::None
            }
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

        // v0.8.44: render plan details when update_plan was called (#834)
        if let Some(ref plan) = self.plan {
            if let Some(ref explanation) = plan.explanation {
                for line in wrap_text(explanation, 68) {
                    lines.push(Line::from(Span::styled(
                        line,
                        Style::default().fg(palette::TEXT_MUTED),
                    )));
                }
                lines.push(Line::from(""));
            }
            if !plan.items.is_empty() {
                lines.push(Line::from(Span::styled(
                    "Plan steps:",
                    Style::default().fg(palette::DEEPSEEK_SKY).bold(),
                )));
                for (i, item) in plan.items.iter().enumerate() {
                    let status_mark = match item.status {
                        crate::tools::plan::StepStatus::Pending => "\u{b7}",
                        crate::tools::plan::StepStatus::InProgress => "\u{25b6}",
                        crate::tools::plan::StepStatus::Completed => "\u{2713}",
                    };
                    let step_text = truncate_step(&item.step, 60);
                    lines.push(Line::from(Span::styled(
                        format!("  {status_mark} {}. {}", i + 1, step_text),
                        Style::default().fg(palette::TEXT_PRIMARY),
                    )));
                }
                lines.push(Line::from(""));
            }
        }

        for (idx, (label, description)) in PLAN_OPTIONS.iter().enumerate() {
            let number = idx + 1;
            push_option_lines(&mut lines, self.selected == idx, number, label, description);
        }

        let popup_area = centered_rect(72, 52, area);
        render_modal_chrome(area, popup_area, buf);

        // Calculate scroll bounds so long plan content doesn't clip the options.
        // Use wrapped_line_count to estimate post-wrap line count.
        let content_width = usize::from(popup_area.width.saturating_sub(4).max(1));
        let total_lines = wrapped_line_count(&lines, content_width);
        let visible_lines = usize::from(popup_area.height).saturating_sub(4).max(1);
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll = self.scroll.min(max_scroll);

        // Build footer: scroll indicator (left) + data-driven option shortcuts +
        // description of the currently selected option (right).
        let mut footer_spans: Vec<Span> = Vec::new();
        if total_lines > visible_lines {
            footer_spans.push(Span::styled(
                format!(" [{}/{} PgUp/Dn · Ctrl+U/D] ", scroll + 1, max_scroll + 1),
                Style::default().fg(palette::DEEPSEEK_SKY),
            ));
        }
        for (idx, _) in PLAN_OPTIONS.iter().enumerate() {
            let shortcut = PLAN_SHORTCUTS[idx];
            let short_label = PLAN_SHORT_LABELS[idx];
            let is_current = self.selected == idx;
            let shortcut_style = if is_current {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .bold()
            } else {
                Style::default().fg(palette::DEEPSEEK_SKY)
            };
            footer_spans.push(Span::styled(
                format!("[{}/{}] {}", idx + 1, shortcut, short_label),
                shortcut_style,
            ));
            footer_spans.push(Span::raw("  "));
        }
        // Selected option description, right-aligned by filling space.
        let desc = PLAN_OPTIONS[self.selected].1;
        let desc_span = Span::styled(
            format!(" → {desc}"),
            Style::default().fg(palette::TEXT_MUTED),
        );
        footer_spans.push(desc_span);

        // When the user pressed Esc after scrolling, show a confirmation prompt
        // instead of the normal plan + options.
        if self.confirming_exit {
            let confirm_lines = vec![
                Line::from(Span::styled(
                    "Exit without implementing?",
                    Style::default().fg(palette::DEEPSEEK_SKY).bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "You've scrolled through the plan content. Are you sure you want to exit?",
                    Style::default().fg(palette::TEXT_PRIMARY),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  y — Yes, exit Plan mode",
                    Style::default().fg(palette::DEEPSEEK_SKY),
                )),
                Line::from(Span::styled(
                    "  n / Esc — Cancel, go back to plan",
                    Style::default().fg(palette::TEXT_MUTED),
                )),
            ];
            let confirm_footer = Line::from(vec![
                Span::styled(" y ", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                Span::styled("confirm exit", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("  "),
                Span::styled("n / Esc", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
                Span::styled(" cancel", Style::default().fg(palette::TEXT_MUTED)),
            ]);
            let popup_area = centered_rect(66, 34, area);
            render_modal_chrome(area, popup_area, buf);
            let confirm = Paragraph::new(confirm_lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true })
                .block(modal_block().title_bottom(confirm_footer));
            confirm.render(popup_area, buf);
        } else {
            let paragraph = Paragraph::new(lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true })
                .block(modal_block().title_bottom(Line::from(footer_spans)))
                .scroll((scroll as u16, 0));

            paragraph.render(popup_area, buf);
        }
    }
}

/// Truncate a plan step description to `max_len` chars, breaking at a word
/// boundary and appending an ellipsis when truncation occurs.
fn truncate_step(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    // Walk back from the cutoff to find a natural word boundary.
    let cutoff = max_len.saturating_sub(3); // reserve room for "..."
    let truncated: String = text.chars().take(cutoff).collect();
    if let Some(last_space) = truncated.rfind(' ') {
        format!("{}...", &truncated[..last_space])
    } else {
        format!("{truncated}...")
    }
}

/// Wrap text into lines no wider than `width` characters.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let words: Vec<&str> = paragraph.split_whitespace().collect();
        let mut current = String::new();
        for word in words {
            let word_width = word.chars().count();
            if word_width > width {
                if !current.is_empty() {
                    lines.push(current.trim_end().to_string());
                    current.clear();
                }
                let mut chars = word.chars();
                loop {
                    let segment: String = chars.by_ref().take(width).collect();
                    if segment.is_empty() {
                        break;
                    }
                    lines.push(segment);
                }
            } else if current.chars().count() + 1 + word_width > width {
                lines.push(current.trim_end().to_string());
                current.clear();
                current.push_str(word);
            } else {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current.trim_end().to_string());
        }
    }
    lines
}

/// Estimate the number of display lines after word-wrapping a set of logical
/// lines to `width` columns. Uses Unicode display widths so CJK characters
/// count as 2 columns.
fn wrapped_line_count(lines: &[Line<'_>], width: usize) -> usize {
    if width == 0 {
        return lines.len().max(1);
    }
    let mut total = 0usize;
    for line in lines {
        let text: String = line.iter().map(|s| s.content.as_ref()).collect();
        if text.is_empty() {
            total += 1;
            continue;
        }
        let display_width = UnicodeWidthStr::width(text.as_str());
        total += (display_width + width - 1) / width;
    }
    total
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
        let rendered = render_view(&PlanPromptView::new(None), 110, 36);

        assert!(rendered.contains("Action required"));
        assert!(rendered.contains("Choose what should happen after this plan."));
        // Data-driven footer shows per-option shortcut labels.
        assert!(rendered.contains("[1/a]"));
        assert!(rendered.contains("[4/q]"));
    }

    #[test]
    fn plan_prompt_keeps_selected_option_and_description_together() {
        let mut view = PlanPromptView::new(None);
        view.selected = 1;

        let rendered = render_view(&view, 110, 36);

        assert!(rendered.contains("> 2) Accept plan (YOLO)"));
        assert!(rendered.contains("Start implementation in YOLO mode (auto-approve)"));
    }

    #[test]
    fn plan_prompt_shows_scroll_indicator_when_content_overflows() {
        use crate::tools::plan::{PlanItemArg, PlanSnapshot, StepStatus};

        let plan = PlanSnapshot {
            explanation: Some("A".repeat(500)),
            items: vec![PlanItemArg {
                step: "Line 1".into(),
                status: StepStatus::Pending,
            }; 20],
        };
        let view = PlanPromptView::new(Some(plan));
        // Render into a small area so content overflows.
        let rendered = render_view(&view, 80, 24);

        assert!(
            rendered.contains("PgUp/Dn"),
            "scroll indicator should appear when content overflows"
        );
    }

    #[test]
    fn plan_prompt_page_up_decrements_scroll() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 12;

        let action = view.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_page_down_increments_scroll() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action = view.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_ctrl_u_decrements_scroll() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 12;

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_ctrl_d_increments_scroll() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_scroll_clamped_in_render() {
        use crate::tools::plan::{PlanItemArg, PlanSnapshot, StepStatus};

        let plan = PlanSnapshot {
            explanation: Some("x".repeat(600)),
            items: vec![PlanItemArg {
                step: "Step".into(),
                status: StepStatus::Pending,
            }; 30],
        };
        let mut view = PlanPromptView::new(Some(plan));
        // Set scroll far beyond content.
        view.scroll = 999;
        let rendered = render_view(&view, 80, 20);

        // The rendered view should still contain the last option.
        assert!(
            rendered.contains("Exit Plan mode"),
            "clamped scroll should keep last options visible"
        );
    }

    #[test]
    fn plan_prompt_gg_jumps_to_top() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 30;

        // First 'g' sets pending flag, no scroll change.
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(view.pending_g);
        assert_eq!(view.scroll, 30);

        // Second 'g' jumps to top.
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(!view.pending_g);
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn plan_prompt_capital_g_jumps_to_bottom() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        // set to MAX so render clamps it.
        assert_eq!(view.scroll, usize::MAX);
    }

    #[test]
    fn plan_prompt_ctrl_f_scrolls_down() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action =
            view.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_ctrl_b_scrolls_up() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 12;

        let action =
            view.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_home_jumps_to_top() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 30;

        let action = view.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn plan_prompt_end_jumps_to_bottom() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action = view.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, usize::MAX);
    }

    #[test]
    fn plan_prompt_pending_g_clears_on_other_key() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 10;

        // Press g → pending.
        view.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert!(view.pending_g);

        // Press Up → pending_g cleared, selected moves.
        view.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(!view.pending_g);

        // Follow-up g should now set pending again, not jump.
        view.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert!(view.pending_g);
        assert_eq!(view.scroll, 10);
    }

    #[test]
    fn plan_prompt_esc_after_scroll_confirms_then_cancels() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 5; // simulate user having scrolled

        // First Esc: enters confirmation mode, does not close.
        let action = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(view.confirming_exit);

        // 'n' cancels confirmation, returns to plan.
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(!view.confirming_exit);
    }

    #[test]
    fn plan_prompt_esc_then_esc_cancels_confirmation() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 3;

        // Enter confirmation.
        view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(view.confirming_exit);

        // Second Esc cancels.
        let action = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(!view.confirming_exit);
    }

    #[test]
    fn plan_prompt_esc_no_scroll_closes_immediately() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::EmitAndClose(_)));
    }

    #[test]
    fn plan_prompt_confirm_then_y_exits() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 2;

        view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::EmitAndClose(_)));
    }

    #[test]
    fn plan_prompt_other_keys_ignored_during_confirmation() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 2;

        view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(view.confirming_exit);

        // Random key (e.g. 'a') should be ignored — does not submit option.
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(view.confirming_exit);
    }
}
