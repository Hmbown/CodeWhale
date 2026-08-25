//! Compact rendering of composer attachment lines.
//!
//! The composer buffer stores each attachment as a whole line carrying its
//! path — `[Attached image: 1920x1440 PNG (2.3MB) at /long/path/shot.png]`.
//! That text is deliberately durable: it survives editing, history recall,
//! queueing and session reload with no side table, it is what four persisted
//! stores round-trip, and it is what the Runtime-Chat isolation guard parses
//! in order to redact local paths from a managed host.
//!
//! None of that is worth reading. This module leaves the buffer exactly as
//! it is and collapses each attachment line to `[Image #1]` for display only,
//! so the stored text keeps every guarantee it has while the composer shows
//! the short token instead of a wall of path.
//!
//! Coordinates are CHAR indices throughout, because every render, cursor and
//! mouse consumer in the composer is char-based. The only byte-space input is
//! [`MediaAttachmentReference`], converted once while the display text is
//! built.

use std::borrow::Cow;

use crate::tui::file_mention::media_attachment_references;

/// One collapsed attachment line, in char coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollapsedSpan {
    /// Char index of the line's first char in the buffer.
    pub buffer_start: usize,
    /// Chars of line content in the buffer, excluding any trailing newline.
    pub buffer_len: usize,
    /// Char index where the token begins in the display text.
    pub display_start: usize,
    /// Chars of the rendered token, e.g. `[Image #1]`.
    pub display_len: usize,
    /// 0-based position among the buffer's attachments; the rendered number
    /// is `index + 1`.
    pub index: usize,
}

impl CollapsedSpan {
    fn buffer_end(&self) -> usize {
        self.buffer_start + self.buffer_len
    }

    fn display_end(&self) -> usize {
        self.display_start + self.display_len
    }
}

/// The composer's text as the user sees it, plus the mapping back to the
/// buffer the user is actually editing.
///
/// In the overwhelmingly common case — no attachments, no history search —
/// `text` borrows the buffer and `spans` is empty, so building this costs one
/// line scan and no allocation.
#[derive(Debug, Clone)]
pub struct ComposerDisplay<'a> {
    /// What to paint.
    pub text: Cow<'a, str>,
    /// Caret position in display char coordinates.
    pub cursor: usize,
    /// Ascending, disjoint. Empty when nothing was collapsed.
    spans: Vec<CollapsedSpan>,
}

impl<'a> ComposerDisplay<'a> {
    /// Pass the text through untouched — used for the history-search query,
    /// which is not a buffer and must never be collapsed.
    #[must_use]
    pub fn passthrough(text: &'a str, cursor: usize) -> Self {
        Self {
            text: Cow::Borrowed(text),
            cursor,
            spans: Vec::new(),
        }
    }

    /// Collapse every attachment line in `input` to its compact token.
    ///
    /// `cursor` is a buffer char index and comes back mapped into display
    /// space, so the painted text and the caret can never disagree.
    #[must_use]
    pub fn collapse(input: &'a str, cursor: usize) -> Self {
        let references = media_attachment_references(input);
        if references.is_empty() {
            return Self::passthrough(input, cursor);
        }

        let mut text = String::with_capacity(input.len());
        let mut spans = Vec::with_capacity(references.len());
        // Walk the buffer once, in char space, copying everything that is not
        // an attachment line and emitting a token for everything that is.
        let mut copied_bytes = 0usize;
        let mut copied_chars = 0usize;
        for (index, reference) in references.iter().enumerate() {
            // The reference range covers the whole line including its
            // trailing newline; the newline must survive, or the attachment
            // would visually join the line below it and the row count would
            // disagree with the mouse geometry.
            let line = &input[reference.start_byte..reference.end_byte];
            let content_len = line.trim_end_matches(['\n', '\r']).len();
            let content_end = reference.start_byte + content_len;

            let lead = &input[copied_bytes..reference.start_byte];
            text.push_str(lead);
            let buffer_start = copied_chars + lead.chars().count();
            let display_start = text.chars().count();

            let token = token_for(&reference.kind, index);
            let display_len = token.chars().count();
            text.push_str(&token);

            spans.push(CollapsedSpan {
                buffer_start,
                buffer_len: input[reference.start_byte..content_end].chars().count(),
                display_start,
                display_len,
                index,
            });

            copied_bytes = content_end;
            copied_chars = buffer_start + spans[spans.len() - 1].buffer_len;
        }
        text.push_str(&input[copied_bytes..]);

        let display = Self {
            text: Cow::Owned(text),
            cursor: 0,
            spans,
        };
        let cursor = display.to_display(cursor);
        Self { cursor, ..display }
    }

    /// The span containing a buffer char index, when it falls strictly inside
    /// a collapsed token. This is the atomicity primitive: cursor motion uses
    /// it to step over a token rather than crawling through the ~60 buffer
    /// chars it hides.
    #[must_use]
    pub fn span_containing(&self, buffer_index: usize) -> Option<&CollapsedSpan> {
        self.spans
            .iter()
            .find(|span| buffer_index > span.buffer_start && buffer_index < span.buffer_end())
    }

    /// Map a buffer char index into display space. An index inside a
    /// collapsed line lands on the token's start, which is also the floor a
    /// selection's left edge wants.
    #[must_use]
    pub fn to_display(&self, buffer_index: usize) -> usize {
        self.map_forward(buffer_index, |span| span.display_start)
    }

    /// As [`Self::to_display`], but an interior index lands past the token —
    /// the right edge of a selection, so any overlap highlights the whole
    /// token instead of none of it.
    #[must_use]
    pub fn to_display_ceil(&self, buffer_index: usize) -> usize {
        self.map_forward(buffer_index, CollapsedSpan::display_end)
    }

    /// Map a display char index back to the buffer. An index inside a token
    /// resolves to the start of the line it stands for.
    #[must_use]
    pub fn to_buffer(&self, display_index: usize) -> usize {
        let mut shift = 0isize;
        for span in &self.spans {
            if display_index <= span.display_start {
                break;
            }
            if display_index < span.display_end() {
                return span.buffer_start;
            }
            shift += span.buffer_len as isize - span.display_len as isize;
        }
        display_index.saturating_add_signed(shift)
    }

    fn map_forward(&self, buffer_index: usize, interior: fn(&CollapsedSpan) -> usize) -> usize {
        let mut shift = 0isize;
        for span in &self.spans {
            if buffer_index <= span.buffer_start {
                break;
            }
            if buffer_index < span.buffer_end() {
                return interior(span);
            }
            shift += span.display_len as isize - span.buffer_len as isize;
        }
        buffer_index.saturating_add_signed(shift)
    }
}

/// The compact token for an attachment. Only `image` and `video` are
/// producible today (`media_kind` in `commands/contract.rs`); anything else
/// keeps a readable generic label rather than being silently mislabelled.
fn token_for(kind: &str, index: usize) -> String {
    let label = match kind {
        "image" => "Image",
        "video" => "Video",
        _ => "Attachment",
    };
    format!("[{label} #{}]", index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(path: &str) -> String {
        format!("[Attached image: 8x4 PNG (2KB) at {path}]")
    }

    #[test]
    fn input_without_attachments_borrows_and_maps_identity() {
        let input = "just some prose\nover two lines";
        let display = ComposerDisplay::collapse(input, 7);
        assert!(
            matches!(display.text, Cow::Borrowed(_)),
            "must not allocate"
        );
        assert_eq!(display.text.as_ref(), input);
        assert_eq!(display.cursor, 7);
        assert!(display.spans.is_empty());
        for index in 0..=input.chars().count() {
            assert_eq!(display.to_display(index), index);
            assert_eq!(display.to_buffer(index), index);
        }
    }

    #[test]
    fn a_single_attachment_collapses_to_a_numbered_token() {
        let input = format!("before\n{}\nafter", attachment("/tmp/shot.png"));
        let display = ComposerDisplay::collapse(&input, 0);
        assert_eq!(display.text.as_ref(), "before\n[Image #1]\nafter");
    }

    #[test]
    fn two_attachments_number_in_order_and_shift_cumulatively() {
        let input = format!(
            "{}\n{}\ntail",
            attachment("/tmp/one.png"),
            attachment("/tmp/two.png")
        );
        let display = ComposerDisplay::collapse(&input, 0);
        assert_eq!(display.text.as_ref(), "[Image #1]\n[Image #2]\ntail");
        // The tail must map to the same characters in both directions.
        let tail_buffer = input.chars().count() - "tail".chars().count();
        let tail_display = display.to_display(tail_buffer);
        assert_eq!(
            display.text.as_ref()[..tail_display].chars().count(),
            tail_display
        );
        assert_eq!(&display.text.as_ref()[tail_display..], "tail");
        assert_eq!(display.to_buffer(tail_display), tail_buffer);
    }

    #[test]
    fn a_trailing_attachment_without_a_final_newline_collapses() {
        let input = format!("look\n{}", attachment("/tmp/last.png"));
        let display = ComposerDisplay::collapse(&input, 0);
        assert_eq!(display.text.as_ref(), "look\n[Image #1]");
    }

    #[test]
    fn a_cursor_inside_a_collapsed_line_lands_on_the_token() {
        let line = attachment("/tmp/shot.png");
        let input = format!("a\n{line}\nb");
        // Somewhere in the middle of the hidden path.
        let interior = 2 + line.chars().count() / 2;
        let display = ComposerDisplay::collapse(&input, interior);
        let span = &display.spans[0];
        assert_eq!(display.cursor, span.display_start);
        assert_eq!(display.to_display(interior), span.display_start);
        assert_eq!(display.to_display_ceil(interior), span.display_end());
        assert!(display.span_containing(interior).is_some());
        // The boundaries are not interior.
        assert!(display.span_containing(span.buffer_start).is_none());
    }

    #[test]
    fn round_trips_every_display_index_back_into_the_buffer() {
        let input = format!(
            "head\n{}\nmid\n{}\ntail",
            attachment("/tmp/one.png"),
            attachment("/tmp/two.png")
        );
        let display = ComposerDisplay::collapse(&input, 0);
        for index in 0..=display.text.chars().count() {
            let buffer = display.to_buffer(index);
            assert!(
                buffer <= input.chars().count(),
                "display {index} mapped outside the buffer"
            );
        }
    }

    #[test]
    fn a_video_attachment_keeps_its_own_label() {
        let input = "[Attached video: 3s MP4 at /tmp/clip.mp4]".to_string();
        let display = ComposerDisplay::collapse(&input, 0);
        assert_eq!(display.text.as_ref(), "[Video #1]");
    }

    #[test]
    fn passthrough_never_collapses_a_history_search_query() {
        let query = format!("find {}", attachment("/tmp/shot.png"));
        let display = ComposerDisplay::passthrough(&query, 3);
        assert_eq!(display.text.as_ref(), query);
        assert_eq!(display.cursor, 3);
        assert!(display.spans.is_empty());
    }
}
