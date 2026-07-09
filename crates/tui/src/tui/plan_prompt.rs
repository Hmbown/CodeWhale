//! Modal prompt for selecting what to do after a plan is generated.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Alignment, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};
use unicode_width::UnicodeWidthStr;

use crate::palette;
use crate::tools::plan::{PlanSnapshot, StepStatus};
use crate::tools::todo::{TodoListSnapshot, TodoStatus};
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent, render_modal_surface};

struct PlanOption {
    label: &'static str,
    description: &'static str,
    shortcut: char,
}

const PLAN_OPTIONS: [PlanOption; 4] = [
    PlanOption {
        label: "Accept plan (Act)",
        description: "Start implementation in Act mode with approvals",
        shortcut: 'a',
    },
    PlanOption {
        label: "Accept plan (Full Access)",
        description: "Start implementation in Act with Full Access (bypass approvals)",
        shortcut: 'y',
    },
    PlanOption {
        label: "Revise plan",
        description: "Ask follow-ups or request plan changes",
        shortcut: 'r',
    },
    PlanOption {
        label: "Exit Plan mode",
        description: "Return to Act mode without implementation",
        shortcut: 'q',
    },
];

fn modal_block() -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            " Plan Confirmation ",
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
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
    shortcut: char,
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
        format!("{prefix} [{number}/{shortcut}] {label}"),
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
    /// The effective `max_scroll` computed during the last render, used so
    /// the Esc handler can check the clamped scroll (not the raw `self.scroll`)
    /// and avoid a spurious exit-confirmation on short plans.
    last_max_scroll: Cell<usize>,
    /// When true, an "are you sure?" prompt is shown instead of the option list
    /// because the user pressed Esc after scrolling away from the top.
    confirming_exit: bool,
    /// The plan snapshot to display (if update_plan was called).
    plan: Option<PlanSnapshot>,
    /// The checklist/todo snapshot to display (if `checklist_write` was used).
    /// Kept separate from the plan so the most actionable view of progress is
    /// visible inside the plan confirmation modal.
    todos: Option<TodoListSnapshot>,
}

impl PlanPromptView {
    pub fn new(plan: Option<PlanSnapshot>) -> Self {
        Self {
            selected: 0,
            scroll: 0,
            pending_g: false,
            last_max_scroll: Cell::new(0),
            confirming_exit: false,
            plan,
            todos: None,
        }
    }

    /// Attach the current checklist/todo snapshot so it renders inside the plan
    /// confirmation modal alongside the plan steps. Existing callers default to
    /// `None`, so this is opt-in at the production construction site only.
    #[must_use]
    pub fn with_todos(mut self, todos: Option<TodoListSnapshot>) -> Self {
        self.todos = todos;
        self
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
                option: number as usize,
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
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
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
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Char('e') | KeyCode::Char('E') => {
                self.selected = 3;
                self.submit_selected()
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let number = ch.to_digit(10).unwrap_or(0);
                Self::submit_number(number)
            }
            KeyCode::Enter => self.submit_selected(),
            KeyCode::Esc => {
                // Use the effective (clamped) scroll from the last render so a
                // short plan that fits entirely never triggers a false positive.
                if self.scroll.min(self.last_max_scroll.get()) > 0 {
                    // User scrolled; ask for confirmation before discarding.
                    // Clear a stray pending_g so it doesn't leak into the
                    // confirm dialog and survive a cancel (#).
                    self.pending_g = false;
                    self.confirming_exit = true;
                    ViewAction::None
                } else {
                    ViewAction::EmitAndClose(ViewEvent::PlanPromptDismissed)
                }
            }
            // Scroll the plan content when it overflows the popup.
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(12);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(12);
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
            // Vim-style scroll keys — only pure 'g'/'G' (no Ctrl/Alt).
            KeyCode::Char('g')
                if self.pending_g
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.pending_g = false;
                self.scroll = 0;
                ViewAction::None
            }
            KeyCode::Char('G')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.scroll = usize::MAX;
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
            KeyCode::Char('g') => {
                self.pending_g = true;
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
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // When the user pressed Esc after scrolling, show a confirmation prompt
        // instead of the normal plan + options.  Render it early so we skip the
        // plan-content construction entirely.
        if self.confirming_exit {
            let confirm_lines = vec![
                Line::from(Span::styled(
                    "Exit without implementing?",
                    Style::default().fg(palette::WHALE_INFO).bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "You've scrolled through the plan content. Are you sure you want to exit?",
                    Style::default().fg(palette::TEXT_PRIMARY),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  y — Yes, exit Plan mode",
                    Style::default().fg(palette::WHALE_INFO),
                )),
                Line::from(Span::styled(
                    "  n / Esc — Cancel, go back to plan",
                    Style::default().fg(palette::TEXT_MUTED),
                )),
            ];
            let confirm_footer = Line::from(vec![
                Span::styled(" y ", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled("confirm exit", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("  "),
                Span::styled("n / Esc", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled(" cancel", Style::default().fg(palette::TEXT_MUTED)),
            ]);
            let popup_area = centered_rect(66, 34, area);
            render_modal_chrome(area, popup_area, buf);
            let confirm = Paragraph::new(confirm_lines)
                .alignment(Alignment::Left)
                .wrap(Wrap { trim: true })
                .block(modal_block().title_bottom(confirm_footer));
            confirm.render(popup_area, buf);
            return;
        }

        let popup_area = centered_rect(72, 52, area);
        let content_width = usize::from(popup_area.width.saturating_sub(4).max(1));
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Action required",
            Style::default().fg(palette::WHALE_INFO).bold(),
        )]));
        lines.push(Line::from(vec![Span::styled(
            "Choose what should happen after this plan.",
            Style::default().fg(palette::TEXT_PRIMARY).bold(),
        )]));
        lines.push(Line::from(""));

        // v0.8.44: render plan details when update_plan was called (#834)
        if let Some(ref plan) = self.plan {
            push_plan_snapshot_lines(&mut lines, plan, content_width);
        }

        // v0.8.62: render the active checklist so the most actionable view of
        // progress is visible inside the plan confirmation modal.
        if let Some(ref todos) = self.todos {
            push_todo_snapshot_lines(&mut lines, todos, content_width);
        }

        for (idx, option) in PLAN_OPTIONS.iter().enumerate() {
            let number = idx + 1;
            push_option_lines(
                &mut lines,
                self.selected == idx,
                number,
                option.shortcut,
                option.label,
                option.description,
            );
        }

        // Calculate scroll bounds so long plan content doesn't clip the options.
        // Since plan steps are now pre-wrapped via wrap_text(), each Line is
        // already width-bounded — use the raw line count directly.
        let total_lines = lines.len();
        // Borders and padding consume rows inside the modal. Slice the visible
        // lines ourselves instead of relying on Paragraph's internal clamp so
        // bottom-jump scrolling reliably reaches the action rows.
        let visible_lines = usize::from(popup_area.height).saturating_sub(4).max(1);
        let max_scroll = total_lines.saturating_sub(visible_lines);
        self.last_max_scroll.set(max_scroll);
        let scroll = self.scroll.min(max_scroll);
        let rendered_lines: Vec<Line<'static>> =
            lines.into_iter().skip(scroll).take(visible_lines).collect();

        // Keep the footer intentionally compact. Long action lists live in the
        // selectable rows so narrow terminals never clip a hidden option.
        let mut footer_spans: Vec<Span> = Vec::new();
        let compact_footer = popup_area.width < 64;
        if total_lines > visible_lines {
            let scroll_text = if compact_footer {
                format!(" [{}/{} Pg] ", scroll + 1, max_scroll + 1)
            } else {
                format!(
                    " [{}/{} PgUp/Dn \u{b7} Ctrl+U/D] ",
                    scroll + 1,
                    max_scroll + 1
                )
            };
            footer_spans.push(Span::styled(
                scroll_text,
                Style::default().fg(palette::WHALE_INFO),
            ));
        }
        if compact_footer {
            footer_spans.extend([
                Span::styled("↑↓", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::raw(" "),
                Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::raw(" "),
                Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
            ]);
        } else {
            footer_spans.extend([
                Span::styled("↑/↓", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled(" move  ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled("Enter", Style::default().fg(palette::WHALE_INFO).bold()),
                Span::styled(" choose  ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled("Esc", Style::default().fg(palette::WHALE_INFO).bold()),
            ]);
        }

        render_modal_chrome(area, popup_area, buf);
        // Wrap { trim: false } — disable ratatui's word-boundary-based line
        // wrapping. All content is already pre-wrapped via wrap_text() above,
        // which breaks only on display-width overflow, not on script boundaries
        // (Latin ↔ CJK).  This avoids forced line-breaks between English and
        // Chinese characters when there is still room on the current line.
        let paragraph = Paragraph::new(rendered_lines)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .block(modal_block().title_bottom(Line::from(footer_spans)));

        paragraph.render(popup_area, buf);
    }
}

fn push_plan_snapshot_lines(
    lines: &mut Vec<Line<'static>>,
    plan: &PlanSnapshot,
    content_width: usize,
) {
    let show_empty = plan_uses_rich_artifact_shape(plan);
    push_plan_text(
        lines,
        "Title",
        plan.title.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Objective",
        plan.objective.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Context",
        plan.context_summary.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Explanation",
        plan.explanation.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_list(
        lines,
        "Sources used",
        &plan.sources_used,
        content_width,
        show_empty,
    );
    push_plan_list(
        lines,
        "Critical files",
        &plan.critical_files,
        content_width,
        show_empty,
    );
    push_plan_list(
        lines,
        "Constraints",
        &plan.constraints,
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Recommended approach",
        plan.recommended_approach.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Verification plan",
        plan.verification_plan.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Risks and unknowns",
        plan.risks_and_unknowns.as_deref(),
        content_width,
        show_empty,
    );
    push_plan_text(
        lines,
        "Handoff packet",
        plan.handoff_packet.as_deref(),
        content_width,
        show_empty,
    );

    if !plan.items.is_empty() {
        lines.push(Line::from(Span::styled(
            "Plan steps:",
            Style::default().fg(palette::WHALE_INFO).bold(),
        )));
        for (i, item) in plan.items.iter().enumerate() {
            let status_mark = match item.status {
                StepStatus::Pending => "\u{b7}",
                StepStatus::InProgress => "\u{25b6}",
                StepStatus::Completed => "\u{2713}",
            };
            let step_text = format!("  {status_mark} {}. {}", i + 1, item.step);
            for line in wrap_text(&step_text, content_width) {
                lines.push(Line::from(Span::styled(
                    line,
                    Style::default().fg(palette::TEXT_PRIMARY),
                )));
            }
        }
        lines.push(Line::from(""));
    } else if show_empty {
        lines.push(Line::from(Span::styled(
            "Plan steps:",
            Style::default().fg(palette::WHALE_INFO).bold(),
        )));
        lines.push(Line::from(Span::styled(
            "  Not provided",
            Style::default().fg(palette::TEXT_MUTED).italic(),
        )));
        lines.push(Line::from(""));
    }
}

/// Render the active checklist/todo snapshot beneath the plan details.
///
/// Mirrors the plan-step glyph language (`·` pending, `▶` in progress, `✓`
/// completed) so the two read as one surface. Completed items are dimmed so
/// attention lands on what remains.
fn push_todo_snapshot_lines(
    lines: &mut Vec<Line<'static>>,
    todos: &TodoListSnapshot,
    content_width: usize,
) {
    if todos.items.is_empty() {
        return;
    }
    lines.push(Line::from(Span::styled(
        format!("Checklist ({}% complete):", todos.completion_pct),
        Style::default().fg(palette::WHALE_INFO).bold(),
    )));
    for (i, item) in todos.items.iter().enumerate() {
        let status_mark = match item.status {
            TodoStatus::Pending => "\u{b7}",
            TodoStatus::InProgress => "\u{25b6}",
            TodoStatus::Completed => "\u{2713}",
        };
        let item_text = format!("  {status_mark} {}. {}", i + 1, item.content);
        let style = if matches!(item.status, TodoStatus::Completed) {
            Style::default().fg(palette::TEXT_MUTED)
        } else {
            Style::default().fg(palette::TEXT_PRIMARY)
        };
        for line in wrap_text(&item_text, content_width) {
            lines.push(Line::from(Span::styled(line, style)));
        }
    }
    lines.push(Line::from(""));
}

fn plan_uses_rich_artifact_shape(plan: &PlanSnapshot) -> bool {
    plan.title.is_some()
        || plan.objective.is_some()
        || plan.context_summary.is_some()
        || !plan.sources_used.is_empty()
        || !plan.critical_files.is_empty()
        || !plan.constraints.is_empty()
        || plan.recommended_approach.is_some()
        || plan.verification_plan.is_some()
        || plan.risks_and_unknowns.is_some()
        || plan.handoff_packet.is_some()
}

fn push_plan_text(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    value: Option<&str>,
    content_width: usize,
    show_empty: bool,
) {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_none() && !show_empty {
        return;
    };
    lines.push(Line::from(Span::styled(
        format!("{label}:"),
        Style::default().fg(palette::WHALE_INFO).bold(),
    )));
    let (value, style) = value.map_or_else(
        || {
            (
                "Not provided",
                Style::default().fg(palette::TEXT_MUTED).italic(),
            )
        },
        |value| (value, Style::default().fg(palette::TEXT_MUTED)),
    );
    for line in wrap_text(value, content_width) {
        lines.push(Line::from(Span::styled(format!("  {line}"), style)));
    }
    lines.push(Line::from(""));
}

fn push_plan_list(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    values: &[String],
    content_width: usize,
    show_empty: bool,
) {
    let values: Vec<&str> = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect();
    if values.is_empty() && !show_empty {
        return;
    }
    lines.push(Line::from(Span::styled(
        format!("{label}:"),
        Style::default().fg(palette::WHALE_INFO).bold(),
    )));
    if values.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Not provided",
            Style::default().fg(palette::TEXT_MUTED).italic(),
        )));
        lines.push(Line::from(""));
        return;
    }
    for value in values {
        for line in wrap_text(&format!("- {value}"), content_width) {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
    }
    lines.push(Line::from(""));
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
            let word_width = UnicodeWidthStr::width(word);
            if word_width > width {
                if !current.is_empty() {
                    lines.push(current.trim_end().to_string());
                    current.clear();
                }
                // Split an over-width word by display width, not code points,
                // so CJK characters are measured consistently with
                // wrapped_line_count and ratatui's Paragraph::wrap.
                let mut remaining = word;
                while !remaining.is_empty() {
                    let mut split_at = 0usize;
                    for (i, ch) in remaining.char_indices() {
                        // Use the exclusive byte range [..end) so the prefix is
                        // always valid UTF-8, even for multi-byte characters.
                        let end = i + ch.len_utf8();
                        if UnicodeWidthStr::width(&remaining[..end]) > width {
                            break;
                        }
                        split_at = end;
                    }
                    if split_at == 0 {
                        // Even one character is wider than width; take it anyway.
                        split_at = remaining.chars().next().unwrap().len_utf8();
                    }
                    lines.push(remaining[..split_at].to_string());
                    remaining = &remaining[split_at..];
                }
            } else if UnicodeWidthStr::width(current.as_str()) + 1 + word_width > width {
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
    use ratatui::style::{Color, Style};

    fn render_buffer(view: &PlanPromptView, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
        buf
    }

    fn render_view(view: &PlanPromptView, width: u16, height: u16) -> String {
        let area = Rect::new(0, 0, width, height);
        let buf = render_buffer(view, width, height);

        buffer_text(&buf, area)
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        (area.y..area.y.saturating_add(area.height))
            .map(|y| {
                (area.x..area.x.saturating_add(area.width))
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn row_containing(buf: &Buffer, area: Rect, needle: &str) -> Option<String> {
        (area.y..area.y.saturating_add(area.height)).find_map(|y| {
            let row: String = (area.x..area.x.saturating_add(area.width))
                .map(|x| buf[(x, y)].symbol())
                .collect();
            row.contains(needle).then_some(row)
        })
    }

    #[test]
    fn plan_prompt_calls_out_required_action_and_controls() {
        let rendered = render_view(&PlanPromptView::new(None), 110, 36);

        assert!(rendered.contains("Action required"));
        assert!(rendered.contains("Choose what should happen after this plan."));
        // Data-driven option rows show per-option shortcut labels without
        // depending on a clipped single-line footer.
        assert!(rendered.contains("[1/a]"));
        assert!(rendered.contains("[4/q]"));
    }

    #[test]
    fn plan_prompt_keeps_selected_option_and_description_together() {
        let mut view = PlanPromptView::new(None);
        view.selected = 1;

        let rendered = render_view(&view, 110, 36);

        assert!(rendered.contains("> [2/y] Accept plan (Full Access)"));
        assert!(
            rendered.contains("Start implementation in Act with Full Access (bypass approvals)")
        );
    }

    #[test]
    fn plan_prompt_paints_an_opaque_modal_surface() {
        let area = Rect::new(0, 0, 90, 24);
        let popup_area = centered_rect(72, 52, area);
        let mut buf = Buffer::empty(area);
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                buf[(x, y)]
                    .set_symbol("X")
                    .set_style(Style::default().fg(Color::Red).bg(Color::Blue));
            }
        }

        PlanPromptView::new(None).render(area, &mut buf);

        let blank_interior_x = popup_area.x + popup_area.width.saturating_sub(3);
        let blank_interior_y = popup_area.y + 2;
        let blank = &buf[(blank_interior_x, blank_interior_y)];
        assert_eq!(blank.symbol(), " ");
        assert_eq!(blank.bg, palette::WHALE_BG);

        let mut rendered_popup = String::new();
        for y in popup_area.y..popup_area.y.saturating_add(popup_area.height) {
            for x in popup_area.x..popup_area.x.saturating_add(popup_area.width) {
                rendered_popup.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(
            !rendered_popup.contains('X'),
            "stale background glyphs must not bleed through the plan modal"
        );
    }

    #[test]
    fn plan_prompt_footer_stays_compact_on_narrow_terminals() {
        let area = Rect::new(0, 0, 72, 22);
        let buf = render_buffer(&PlanPromptView::new(None), area.width, area.height);
        let footer = row_containing(&buf, area, "Enter").expect("footer should render");

        assert!(footer.contains("Esc"));
        assert!(!footer.contains("Accept plan"));
        assert!(!footer.contains("YOLO"));
        assert!(
            footer.chars().count() <= area.width as usize,
            "footer must fit the terminal row: {footer:?}"
        );
    }

    #[test]
    fn plan_prompt_renders_rich_plan_artifact_sections() {
        use crate::tools::plan::{PlanItemArg, PlanSnapshot, StepStatus};

        let plan = PlanSnapshot {
            title: Some("PlanArtifact rollout".to_string()),
            objective: Some("Make Plan mode reviewable".to_string()),
            context_summary: Some("Issue #2691 asks for grounded plan artifacts.".to_string()),
            sources_used: vec!["gh issue view 2691".to_string()],
            critical_files: vec!["crates/tui/src/tools/plan.rs".to_string()],
            constraints: vec!["Preserve legacy update_plan payloads".to_string()],
            recommended_approach: Some(
                "Keep To-do primary and enrich update_plan Strategy metadata.".to_string(),
            ),
            verification_plan: Some("Run focused plan prompt tests.".to_string()),
            risks_and_unknowns: Some("Avoid dropping metadata-only plans.".to_string()),
            handoff_packet: Some("Continue with transcript replay checks.".to_string()),
            items: vec![PlanItemArg {
                step: "Render rich sections".to_string(),
                status: StepStatus::InProgress,
            }],
            ..PlanSnapshot::default()
        };
        let view = PlanPromptView::new(Some(plan));
        let rendered = render_view(&view, 160, 120);

        assert!(rendered.contains("Objective:"));
        assert!(rendered.contains("Make Plan mode reviewable"));
        assert!(rendered.contains("Sources used:"));
        assert!(rendered.contains("gh issue view 2691"));
        assert!(rendered.contains("Critical files:"));
        assert!(rendered.contains("Verification plan:"));
        assert!(rendered.contains("Handoff packet:"));
        assert!(rendered.contains("Render rich sections"));
    }

    #[test]
    fn plan_prompt_renders_active_checklist_when_provided() {
        use crate::tools::todo::{TodoItem, TodoListSnapshot, TodoStatus};

        let todos = TodoListSnapshot {
            items: vec![
                TodoItem {
                    id: 1,
                    content: "Read the brief".to_string(),
                    status: TodoStatus::Completed,
                },
                TodoItem {
                    id: 2,
                    content: "Render the checklist".to_string(),
                    status: TodoStatus::InProgress,
                },
                TodoItem {
                    id: 3,
                    content: "Ship the PR".to_string(),
                    status: TodoStatus::Pending,
                },
            ],
            completion_pct: 33,
            in_progress_id: Some(2),
        };
        let view = PlanPromptView::new(None).with_todos(Some(todos));
        let rendered = render_view(&view, 160, 120);

        assert!(rendered.contains("Checklist (33% complete):"));
        assert!(rendered.contains("Read the brief"));
        assert!(rendered.contains("Render the checklist"));
        assert!(rendered.contains("Ship the PR"));
    }

    #[test]
    fn plan_prompt_omits_checklist_section_when_empty() {
        use crate::tools::todo::TodoListSnapshot;

        let todos = TodoListSnapshot {
            items: vec![],
            completion_pct: 0,
            in_progress_id: None,
        };
        let view = PlanPromptView::new(None).with_todos(Some(todos));
        let rendered = render_view(&view, 160, 120);

        assert!(!rendered.contains("Checklist"));
    }

    #[test]
    fn plan_prompt_renders_empty_artifact_sections_for_rich_plans() {
        use crate::tools::plan::PlanSnapshot;

        let plan = PlanSnapshot {
            objective: Some("Review grounded plan".to_string()),
            ..PlanSnapshot::default()
        };
        let view = PlanPromptView::new(Some(plan));
        let rendered = render_view(&view, 160, 120);

        assert!(rendered.contains("Objective:"));
        assert!(rendered.contains("Review grounded plan"));
        assert!(rendered.contains("Sources used:"));
        assert!(rendered.contains("Critical files:"));
        assert!(rendered.contains("Verification plan:"));
        assert!(rendered.contains("Risks and unknowns:"));
        assert!(rendered.contains("Plan steps:"));
        assert!(rendered.contains("Not provided"));
    }

    #[test]
    fn plan_prompt_shows_scroll_indicator_when_content_overflows() {
        use crate::tools::plan::{PlanItemArg, PlanSnapshot, StepStatus};

        let plan = PlanSnapshot {
            explanation: Some("A".repeat(500)),
            items: vec![
                PlanItemArg {
                    step: "Line 1".into(),
                    status: StepStatus::Pending,
                };
                20
            ],
            ..PlanSnapshot::default()
        };
        let view = PlanPromptView::new(Some(plan));
        // Render into a small area so content overflows.
        let rendered = render_view(&view, 80, 24);

        assert!(
            rendered.contains("Pg"),
            "scroll indicator should appear when content overflows"
        );
    }

    #[test]
    fn plan_prompt_page_up_decrements_scroll() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 12;

        let action = view.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn plan_prompt_page_down_increments_scroll() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 0;

        let action = view.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 12);
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
            items: vec![
                PlanItemArg {
                    step: "Step".into(),
                    status: StepStatus::Pending,
                };
                30
            ],
            ..PlanSnapshot::default()
        };
        let mut view = PlanPromptView::new(Some(plan));
        // Set scroll far beyond content.
        view.scroll = usize::MAX;
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

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.scroll, 6);
    }

    #[test]
    fn plan_prompt_ctrl_b_scrolls_up() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 12;

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
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
        view.last_max_scroll.set(5);

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
        view.last_max_scroll.set(3);

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
        view.last_max_scroll.set(2);

        view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::EmitAndClose(_)));
    }

    #[test]
    fn plan_prompt_other_keys_ignored_during_confirmation() {
        let mut view = PlanPromptView::new(None);
        view.scroll = 2;
        view.last_max_scroll.set(2);

        view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(view.confirming_exit);

        // Random key (e.g. 'a') should be ignored — does not submit option.
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(view.confirming_exit);
    }
}
