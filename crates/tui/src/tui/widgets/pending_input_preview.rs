//! Pending-input preview widget for the composer area.
//!
//! Port of `codex-rs/tui/src/bottom_pane/pending_input_preview.rs` for
//! issue #85. Renders queued/steered messages above the composer when a
//! turn is in flight, so user input typed during a running turn doesn't
//! disappear silently. The backing state still distinguishes queue/steer
//! origins, but the UI renders one coherent pending-input list.
//!
//! Empty state renders zero rows so the composer doesn't gain wasted height
//! when there's nothing to show.
//!
//! Wired into `ui.rs::render` between the chat area and the composer; the user
//! can see when typed input has been captured for later delivery.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use unicode_width::UnicodeWidthChar;

use crate::localization::{self, Locale};
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
    pub context_items: Vec<ContextPreviewItem>,
    pub pending_steers: Vec<String>,
    pub rejected_steers: Vec<String>,
    pub queued_messages: Vec<String>,
    pub edit_binding: EditBinding,
    pub locale: Locale,
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
    pub fn new() -> Self {
        Self {
            context_items: Vec::new(),
            pending_steers: Vec::new(),
            rejected_steers: Vec::new(),
            queued_messages: Vec::new(),
            edit_binding: EditBinding::UP,
            locale: Locale::En,
        }
    }

    pub fn with_locale(locale: Locale) -> Self {
        Self {
            context_items: Vec::new(),
            pending_steers: Vec::new(),
            rejected_steers: Vec::new(),
            queued_messages: Vec::new(),
            edit_binding: EditBinding::UP,
            locale,
        }
    }

    fn has_pending_inputs(&self) -> bool {
        !self.pending_steers.is_empty()
            || !self.rejected_steers.is_empty()
            || !self.queued_messages.is_empty()
    }

    /// Build the (possibly empty) ordered line list this widget would render
    /// at `width`. Pulled out so `desired_height` can ask the same renderer
    /// without duplicating wrapping logic.
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
                    Span::raw(localization::tr(
                        self.locale,
                        localization::MessageId::PendingInputsContextHeader,
                    )),
                ]),
            );
            for item in &self.context_items {
                push_context_item(&mut lines, item, width);
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
                    Span::raw(localization::tr(
                        self.locale,
                        localization::MessageId::PendingInputsHeader,
                    )),
                ]),
            );
            for steer in &self.pending_steers {
                push_truncated_item(&mut lines, steer, width, dim, "  ↳ ", "    ");
            }
            for steer in &self.rejected_steers {
                push_truncated_item(&mut lines, steer, width, dim, "  ↳ ", "    ");
            }
            for message in &self.queued_messages {
                push_truncated_item(&mut lines, message, width, dim_italic, "  ↳ ", "    ");
            }
            if !self.queued_messages.is_empty() {
                let hint =
                    localization::tr(self.locale, localization::MessageId::PendingInputsEditHint);
                lines.push(Line::from(vec![Span::styled(
                    hint.replace("{}", self.edit_binding.label),
                    dim,
                )]));
            }
        }

        lines
    }
}

impl Default for PendingInputPreview {
    fn default() -> Self {
        Self::new()
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

fn push_section_header(lines: &mut Vec<Line<'static>>, header: Line<'static>) {
    lines.push(header);
}

fn push_context_item(lines: &mut Vec<Line<'static>>, item: &ContextPreviewItem, width: u16) {
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
        " · Backspace/Delete removes"
    } else if item.removable {
        " · removable"
    } else {
        ""
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

/// Render a single bucket item with `↳` prefix, truncating to
/// [`PREVIEW_LINE_LIMIT`] visible rows. Multi-line input wraps at the given
/// column budget and the continuation rows get the `subsequent_indent` so
/// the prefix and the body stay column-aligned.
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

/// Naive word-aware wrap that respects unicode display widths. Matches the
/// behavior expected by snapshot tests in the codex source — long URL-like
/// tokens that exceed `width` are emitted on their own row instead of being
/// hard-broken mid-character.
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
            // Token longer than the budget: flush current, emit the word as
            // its own row even though it overflows. Avoids the codex-issue
            // of a long URL fanning out into N junk-ellipsis rows.
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
        let preview = PendingInputPreview::new();
        assert_eq!(preview.desired_height(40), 0);
    }

    #[test]
    fn single_queued_message_renders_header_item_and_hint() {
        let mut preview = PendingInputPreview::new();
        preview.queued_messages.push("Hello, world!".to_string());
        let rows = render_to_string(&preview, 40);
        // Expect: header line, message line, hint line.
        assert_eq!(rows.len(), 3, "got rows: {rows:?}");
        assert!(rows[0].contains("Pending inputs"));
        assert!(rows[1].contains("Hello, world!"));
        assert!(rows[2].contains("edit last queued message"));
    }

    #[test]
    fn context_items_render_before_queue_buckets() {
        let mut preview = PendingInputPreview::new();
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
        let mut preview = PendingInputPreview::new();
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
        let mut preview = PendingInputPreview::new();
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
        let mut preview = PendingInputPreview::new();
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
    fn message_truncates_to_three_visible_lines() {
        let mut preview = PendingInputPreview::new();
        preview
            .queued_messages
            .push("line1\nline2\nline3\nline4\nline5".to_string());
        let rows = render_to_string(&preview, 40);
        // Header + 3 visible lines + ellipsis row + hint = 6 rows.
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
        let mut preview = PendingInputPreview::new();
        preview.queued_messages.push(
            "example.test/api/v1/projects/alpha/releases/2026-02-17/build/1234567890/artifacts/x"
                .to_string(),
        );
        let rows = render_to_string(&preview, 36);
        // Header + URL row + hint = 3 rows; the URL must NOT cause a chain of
        // wrapped-ellipsis rows.
        assert_eq!(rows.len(), 3, "got rows: {rows:?}");
        assert!(!rows.iter().any(|r| r.contains("…")));
    }

    #[test]
    fn narrow_width_renders_nothing() {
        let mut preview = PendingInputPreview::new();
        preview.queued_messages.push("hi".to_string());
        assert_eq!(preview.desired_height(2), 0);
    }
}
