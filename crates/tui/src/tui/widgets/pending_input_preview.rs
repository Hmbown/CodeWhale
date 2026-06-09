use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthChar;

use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::widgets::Renderable;

/// Per-item line cap before we collapse the rest into a `…` overflow row.
const PREVIEW_LINE_LIMIT: usize = 3;

/// Description of the keybinding the hint line at the bottom should advertise
/// for the "edit last queued message" action.
#[derive(Debug, Clone)]
pub struct EditBinding {
    pub label: &'static str,
}

impl EditBinding {
    pub const UP: EditBinding = EditBinding { label: "↑" };
}

/// Widget showing pending input while a turn is in progress.
#[derive(Debug, Clone)]
pub struct PendingInputPreview {
    pub locale: Locale,
    pub context_items: Vec<ContextPreviewItem>,
    pub pending_steers: Vec<String>,
    pub rejected_steers: Vec<String>,
    pub queued_messages: Vec<String>,
    pub editing_queued_message: Option<String>,
    pub edit_binding: EditBinding,
}

/// Compact pre-send context row shown above the composer. `included=false`
/// marks missing/skipped context distinctly from files/media that will be
/// sent or inlined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextPreviewItem {
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub included: bool,
    pub removable: bool,
    pub selected: bool,
}

impl PendingInputPreview {
    pub fn new(locale: Locale) -> Self {
        Self {
            locale,
            context_items: Vec::new(),
            pending_steers: Vec::new(),
            rejected_steers: Vec::new(),
            queued_messages: Vec::new(),
            editing_queued_message: None,
            edit_binding: EditBinding::UP,
        }
    }

    fn pending_steer_prefix(&self) -> String {
        format!("  ↳ {}", tr(self.locale, MessageId::PendingSteerLabel))
    }

    fn rejected_steer_prefix(&self) -> String {
        format!("  ↳ {}", tr(self.locale, MessageId::PendingRejectedLabel))
    }

    fn queued_message_prefix(&self) -> String {
        format!("  ↳ {}", tr(self.locale, MessageId::PendingQueuedLabel))
    }

    fn editing_queued_prefix(&self) -> String {
        format!("  ↳ {}", tr(self.locale, MessageId::PendingEditingLabel))
    }

    fn has_pending_inputs(&self) -> bool {
        !self.pending_steers.is_empty()
            || !self.rejected_steers.is_empty()
            || !self.queued_messages.is_empty()
            || self.editing_queued_message.is_some()
    }

    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        if (self.context_items.is_empty() && !self.has_pending_inputs()) || width < 4 {
            return Vec::new();
        }

        let dim = Style::default()
            .fg(palette::TEXT_DIM)
            .add_modifier(Modifier::DIM);
        let dim_italic = dim.add_modifier(Modifier::ITALIC);

        let mut lines: Vec<Line<'static>> = Vec::new();

        if !self.context_items.is_empty() {
            push_section_header(
                &mut lines,
                Line::from(vec![
                    Span::raw("• "),
                    Span::raw(tr(self.locale, MessageId::PendingContextHeader).to_string()),
                ]),
            );
            for item in &self.context_items {
                push_context_item(&mut lines, item, width, self.locale);
            }
        }

        if self.has_pending_inputs() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            push_section_header(
                &mut lines,
                Line::from(vec![
                    Span::raw("• "),
                    Span::raw(tr(self.locale, MessageId::PendingInputHeader).to_string()),
                ]),
            );
            let pending_steer_prefix = self.pending_steer_prefix();
            let pending_steer_indent = continuation_indent(&pending_steer_prefix);
            for steer in &self.pending_steers {
                push_truncated_item(
                    &mut lines,
                    steer,
                    width,
                    dim,
                    &pending_steer_prefix,
                    &pending_steer_indent,
                );
            }
            let rejected_steer_prefix = self.rejected_steer_prefix();
            let rejected_steer_indent = continuation_indent(&rejected_steer_prefix);
            for steer in &self.rejected_steers {
                push_truncated_item(
                    &mut lines,
                    steer,
                    width,
                    dim,
                    &rejected_steer_prefix,
                    &rejected_steer_indent,
                );
            }
            if let Some(draft) = self.editing_queued_message.as_deref() {
                let editing_prefix = self.editing_queued_prefix();
                let editing_indent = continuation_indent(&editing_prefix);
                push_truncated_item(
                    &mut lines,
                    draft,
                    width,
                    dim_italic,
                    &editing_prefix,
                    &editing_indent,
                );
                lines.push(Line::from(vec![Span::styled(
                    format!("    {}", tr(self.locale, MessageId::PendingRestoreHint)),
                    dim,
                )]));
            }
            let queued_message_prefix = self.queued_message_prefix();
            let queued_message_indent = continuation_indent(&queued_message_prefix);
            for message in &self.queued_messages {
                push_truncated_item(
                    &mut lines,
                    message,
                    width,
                    dim_italic,
                    &queued_message_prefix,
                    &queued_message_indent,
                );
            }
            if !self.queued_messages.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "    {}",
                        tr(self.locale, MessageId::PendingEditHint)
                            .replace("{key}", self.edit_binding.label)
                    ),
                    dim,
                )]));
            }
        }

        lines
    }
}

impl Default for PendingInputPreview {
    fn default() -> Self {
        Self::new(Locale::En)
    }
}

impl Renderable for PendingInputPreview {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let lines = self.lines(area.width);
        if lines.is_empty() {
            return;
        }
        Paragraph::new(lines).render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let lines = self.lines(width);
        u16::try_from(lines.len()).unwrap_or(u16::MAX)
    }
}

fn continuation_indent(prefix: &str) -> String {
    " ".repeat(display_width(prefix))
}

fn push_section_header(lines: &mut Vec<Line<'static>>, header: Line<'static>) {
    lines.push(header);
}

fn push_context_item(
    lines: &mut Vec<Line<'static>>,
    item: &ContextPreviewItem,
    width: u16,
    locale: Locale,
) {
    let status_style = if item.selected {
        Style::default()
            .fg(palette::SELECTION_TEXT)
            .bg(palette::SELECTION_BG)
            .add_modifier(Modifier::BOLD)
    } else if item.included {
        Style::default().fg(palette::TEXT_MUTED)
    } else {
        Style::default().fg(palette::STATUS_WARNING)
    };
    let label_style = if item.selected {
        Style::default()
            .fg(palette::SELECTION_TEXT)
            .bg(palette::SELECTION_BG)
    } else if item.included {
        Style::default().fg(palette::TEXT_PRIMARY)
    } else {
        Style::default().fg(palette::TEXT_MUTED)
    };
    let detail = item
        .detail
        .as_deref()
        .filter(|detail| !detail.trim().is_empty())
        .map(|detail| format!(" · {detail}"))
        .unwrap_or_default();
    let action = if item.selected {
        format!(" · {}", tr(locale, MessageId::PendingDeleteHint))
    } else if item.removable {
        format!(" · {}", tr(locale, MessageId::PendingRemovable))
    } else {
        String::new()
    };
    let body = format!("[{}] {}{}{}", item.kind, item.label, detail, action);
    let body_width = width.saturating_sub(4).max(1) as usize;
    for (idx, segment) in wrap_to_width(&body, body_width).into_iter().enumerate() {
        let prefix = if idx == 0 {
            if item.selected { "  ▸ " } else { "  ↳ " }
        } else {
            "    "
        };
        lines.push(Line::from(vec![
            Span::styled(prefix.to_string(), status_style),
            Span::styled(segment, label_style),
        ]));
    }
}

fn push_truncated_item(
    lines: &mut Vec<Line<'static>>,
    raw: &str,
    width: u16,
    style: Style,
    prefix: &str,
    subsequent_indent: &str,
) {
    let body_width = width.saturating_sub(display_width(prefix) as u16) as usize;
    let body_width = body_width.max(1);

    let mut produced: Vec<String> = Vec::new();
    for (idx, paragraph) in raw.split('\n').enumerate() {
        let wrapped = wrap_to_width(paragraph, body_width);
        for (j, segment) in wrapped.into_iter().enumerate() {
            let row = if idx == 0 && j == 0 {
                format!("{prefix}{segment}")
            } else {
                format!("{subsequent_indent}{segment}")
            };
            produced.push(row);
            if produced.len() > PREVIEW_LINE_LIMIT {
                break;
            }
        }
        if produced.len() > PREVIEW_LINE_LIMIT {
            break;
        }
    }

    let truncated = produced.len() > PREVIEW_LINE_LIMIT;
    for (i, row) in produced.into_iter().enumerate() {
        if i >= PREVIEW_LINE_LIMIT {
            break;
        }
        lines.push(Line::from(Span::styled(row, style)));
    }
    if truncated {
        lines.push(Line::from(Span::styled(
            format!("{subsequent_indent}…"),
            style,
        )));
    }
}

fn wrap_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }

    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_inclusive(' ') {
        let word_width = display_width(word);
        if current_width + word_width > width && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width > width {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_width = 0;
            }
            out.push(word.trim_end().to_string());
            continue;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn display_width(s: &str) -> usize {
    s.chars()
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_to_string(widget: &PendingInputPreview, width: u16) -> Vec<String> {
        let height = widget.desired_height(width);
        if height == 0 {
            return Vec::new();
        }
        let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
        widget.render(Rect::new(0, 0, width, height), &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn empty_widget_has_zero_height() {
        let preview = PendingInputPreview::new(Locale::En);
        assert_eq!(preview.desired_height(40), 0);
    }

    #[test]
    fn single_queued_message_renders_header_item_and_hint() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.queued_messages.push("Hello, world!".to_string());
        let rows = render_to_string(&preview, 40);
        assert_eq!(rows.len(), 3, "got rows: {rows:?}");
        assert!(rows[0].contains("Pending inputs"));
        assert!(rows[1].contains("Hello, world!"));
        assert!(rows[2].contains("edit last queued message"));
    }

    #[test]
    fn editing_queued_message_renders_explicit_state_and_restore_hint() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.editing_queued_message = Some("revise before sending".to_string());

        let rows = render_to_string(&preview, 80);

        assert!(rows[0].contains("Pending inputs"));
        assert!(
            rows.iter()
                .any(|row| row.contains("Editing queued follow-up: revise before sending")),
            "missing editing label: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Esc restores queued follow-up")),
            "missing restore hint: {rows:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("edit last queued message")),
            "editing mode should not also advertise opening a queued edit: {rows:?}"
        );
    }

    #[test]
    fn context_items_render_before_queue_buckets() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.context_items.push(ContextPreviewItem {
            kind: "file".to_string(),
            label: "src/main.rs".to_string(),
            detail: Some("included".to_string()),
            included: true,
            removable: false,
            selected: false,
        });
        preview.context_items.push(ContextPreviewItem {
            kind: "missing".to_string(),
            label: "nope.txt".to_string(),
            detail: Some("not found".to_string()),
            included: false,
            removable: false,
            selected: false,
        });
        let rows = render_to_string(&preview, 64);
        assert!(rows[0].contains("Context for next send"));
        assert!(rows[1].contains("[file] src/main.rs"));
        assert!(rows[2].contains("[missing] nope.txt"));
    }

    #[test]
    fn selected_removable_attachment_renders_delete_hint() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.context_items.push(ContextPreviewItem {
            kind: "image".to_string(),
            label: "/tmp/pasted.png".to_string(),
            detail: Some("attached media".to_string()),
            included: true,
            removable: true,
            selected: true,
        });

        let rows = render_to_string(&preview, 96);

        assert!(
            rows.iter()
                .any(|row| row.contains("Backspace/Delete removes"))
        );
        assert!(rows.iter().any(|row| row.contains("▸")));
    }

    #[test]
    fn pending_steer_renders_without_queue_edit_hint() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.pending_steers.push("Please continue.".to_string());
        let rows = render_to_string(&preview, 80);
        assert!(
            rows.iter().any(|r| r.contains("Pending inputs")),
            "missing pending input header: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("Esc")),
            "unexpected Esc hint: {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("edit last queued message")),
            "unexpected edit hint in pending-steer-only view: {rows:?}"
        );
    }

    #[test]
    fn all_pending_inputs_render_as_one_list() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.pending_steers.push("steer".to_string());
        preview.rejected_steers.push("rejected".to_string());
        preview.queued_messages.push("queued".to_string());
        let rows = render_to_string(&preview, 60);
        assert!(rows[0].contains("Pending inputs"));
        assert_eq!(
            rows.iter().filter(|r| r.contains("Pending inputs")).count(),
            1
        );
        assert!(rows.iter().any(|r| r.contains("steer")));
        assert!(rows.iter().any(|r| r.contains("rejected")));
        assert!(rows.iter().any(|r| r.contains("queued")));
        assert!(rows.iter().any(|r| r.contains("↑")));
    }

    #[test]
    fn pending_input_rows_label_each_delivery_mode() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.pending_steers.push("steer".to_string());
        preview.rejected_steers.push("rejected".to_string());
        preview.queued_messages.push("queued".to_string());
        preview.editing_queued_message = Some("editing".to_string());

        let rows = render_to_string(&preview, 80);

        assert!(
            rows.iter().any(|row| row.contains("Steer pending: steer")),
            "missing pending-steer label: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Rejected steer: rejected")),
            "missing rejected-steer label: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Queued follow-up: queued")),
            "missing queued-follow-up label: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Editing queued follow-up: editing")),
            "missing queued-edit label: {rows:?}"
        );
    }

    #[test]
    fn wrapped_pending_input_aligns_continuation_under_label() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview
            .queued_messages
            .push("alpha beta gamma delta epsilon zeta".to_string());

        let rows = render_to_string(&preview, 34);

        assert!(rows[1].contains("Queued follow-up: alpha"));
        let q_prefix = preview.queued_message_prefix();
        assert!(
            rows[2].starts_with(&continuation_indent(&q_prefix)),
            "continuation should align under label: {rows:?}"
        );
        assert!(
            !rows[2].trim().is_empty(),
            "continuation should keep wrapped text: {rows:?}"
        );
    }

    #[test]
    fn message_truncates_to_three_visible_lines() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview
            .queued_messages
            .push("line1\nline2\nline3\nline4\nline5".to_string());
        let rows = render_to_string(&preview, 40);
        assert_eq!(rows.len(), 6, "got rows: {rows:?}");
        assert!(rows[0].contains("Pending inputs"));
        assert!(rows[1].contains("line1"));
        assert!(rows[2].contains("line2"));
        assert!(rows[3].contains("line3"));
        assert!(rows[4].contains("…"));
        assert!(rows[5].contains("edit last queued message"));
    }

    #[test]
    fn long_url_does_not_explode_into_ellipsis_rows() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.queued_messages.push(
            "example.test/api/v1/projects/alpha/releases/2026-02-17/build/1234567890/artifacts/x"
                .to_string(),
        );
        let rows = render_to_string(&preview, 36);
        assert_eq!(rows.len(), 3, "got rows: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains("…")));
    }

    #[test]
    fn narrow_width_renders_nothing() {
        let mut preview = PendingInputPreview::new(Locale::En);
        preview.queued_messages.push("hi".to_string());
        assert_eq!(preview.desired_height(2), 0);
    }
}
