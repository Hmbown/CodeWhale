//! Markdown rendering for TUI transcript lines.
//!
//! ## Width-independent parse vs width-dependent render (CX#6)
//!
//! The previous renderer was a single function `render_markdown(content, width)`
//! that scanned the source, classified each line (heading / list / code-fence /
//! paragraph / link), and word-wrapped to `Line<'static>` in one pass. That meant
//! every terminal resize forced a full re-parse of the source for every visible
//! cell — wasted work on the streaming cell whose content is changing anyway.
//!
//! The codex tui solves this by splitting parse from render. We mirror that:
//!
//! * [`parse`] turns the markdown source into a [`ParsedMarkdown`] AST: a vector
//!   of width-independent [`Block`]s. The block kind already records all the
//!   classification decisions (heading level, list bullet, code block membership)
//!   that don't depend on width.
//! * [`render_parsed`] takes a `ParsedMarkdown` plus a width and a base style and
//!   produces `Vec<Line<'static>>`. It only does word-wrap and span styling.
//!
//! [`render_markdown`] is kept as a thin convenience that does both — useful for
//! callers (Thinking body, message body) that don't want to manage the cache.
//!
//! The transcript cache layer (see `tui/transcript.rs`) caches the parsed AST per
//! cell and re-runs only the render step on width changes. That makes resize a
//! re-flow operation rather than a re-parse + re-flow operation.

#[cfg(test)]
use std::cell::Cell;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::tui::osc8;

// Thread-local counter incremented every time `parse` runs. Used by tests to
// prove that width-only changes hit the cached-AST path and skip parsing.
// Thread-local (not global atomic) so concurrent tests calling `parse()` can't
// pollute each other's counters.
#[cfg(test)]
thread_local! {
    static PARSE_INVOCATIONS: Cell<u64> = const { Cell::new(0) };
}

#[cfg(test)]
#[must_use]
pub fn parse_invocation_count() -> u64 {
    PARSE_INVOCATIONS.with(|c| c.get())
}

#[cfg(test)]
pub fn reset_parse_invocation_count() {
    PARSE_INVOCATIONS.with(|c| c.set(0));
}

/// One classified line of markdown source, width-independent.
///
/// All decisions that depend only on the source text (heading level, bullet
/// kind, whether we're inside a fenced code block, paragraph text) are made at
/// parse time. Width-dependent layout (word-wrap, prefix indent) is deferred to
/// the render step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `# heading text`. Includes the heading level (1..6).
    Heading { level: usize, text: String },
    /// A horizontal rule emitted under a level-1 heading.
    HeadingRule,
    /// A standalone `---` / `***` / `___` horizontal rule.
    HorizontalRule,
    /// A bullet (`-`/`*`) or ordered (`1.`) list item with its prefix and body.
    ListItem { bullet: String, text: String },
    /// A line inside a fenced code block. Fences themselves are dropped.
    Code { line: String },
    /// A table row: cells split on `|`.
    TableRow(Vec<String>),
    /// A table separator row (`|---|---|`). Kept so the renderer can draw
    /// horizontal rules at the correct positions.
    TableSeparator,
    /// A non-empty paragraph line that may contain inline links.
    Paragraph { text: String },
    /// An empty source line, preserved so paragraph spacing survives.
    Blank,
}

/// Width-independent parsed-markdown AST for one cell's source.
///
/// Wrapped in `Arc` at the cache layer so the cache can hand the same AST to
/// many render calls without copying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMarkdown {
    blocks: Vec<Block>,
}

/// Width-dependent rendered line plus the source block kind that produced it.
///
/// Most callers only need styled terminal lines, but transcript rendering also
/// needs to avoid adding its conversational continuation rail in front of code
/// blocks. Keeping this metadata here avoids guessing from styled spans.
#[derive(Debug, Clone)]
pub struct RenderedMarkdownLine {
    pub line: Line<'static>,
    pub is_code: bool,
}

/// Parse markdown source into a width-independent block AST.
///
/// This is a small line-oriented parser tuned for the patterns we render:
/// fenced code blocks, ATX headings, dash/star/numbered list items, and plain
/// paragraphs with optional links. It does not attempt to handle every CommonMark
/// edge case — that's intentional. The renderer will treat anything we don't
/// classify as `Block::Paragraph`.
#[must_use]
pub fn parse(content: &str) -> ParsedMarkdown {
    #[cfg(test)]
    PARSE_INVOCATIONS.with(|c| c.set(c.get() + 1));

    let mut blocks = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            blocks.push(Block::Code {
                line: raw_line.to_string(),
            });
            continue;
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            blocks.push(Block::Heading {
                level,
                text: text.to_string(),
            });
            if level == 1 {
                blocks.push(Block::HeadingRule);
            }
            continue;
        }

        if let Some((bullet, text)) = parse_list_item(trimmed) {
            blocks.push(Block::ListItem {
                bullet,
                text: text.to_string(),
            });
            continue;
        }

        if is_horizontal_rule(trimmed) {
            blocks.push(Block::HorizontalRule);
            continue;
        }

        match parse_table_row(trimmed) {
            Some(cells) => {
                blocks.push(Block::TableRow(cells));
                continue;
            }
            None if trimmed.starts_with('|') => {
                blocks.push(Block::TableSeparator);
                continue;
            }
            None => {}
        }

        if raw_line.is_empty() {
            blocks.push(Block::Blank);
            continue;
        }

        blocks.push(Block::Paragraph {
            text: trimmed.to_string(),
        });
    }

    ParsedMarkdown { blocks }
}

/// Render a parsed-markdown AST at the given terminal width.
///
/// This is the width-dependent half: word-wrapping, link styling, code-block
/// formatting. The AST is owned by the caller (typically the transcript cache),
/// so width-only changes can call `render_parsed` again with the same AST and
/// skip the parse step entirely.
#[must_use]
pub fn render_parsed(parsed: &ParsedMarkdown, width: u16, base_style: Style) -> Vec<Line<'static>> {
    render_parsed_tagged(parsed, width, base_style)
        .into_iter()
        .map(|line| line.line)
        .collect()
}

/// Render a parsed-markdown AST and preserve per-line source metadata.
#[must_use]
pub fn render_parsed_tagged(
    parsed: &ParsedMarkdown,
    width: u16,
    base_style: Style,
) -> Vec<RenderedMarkdownLine> {
    let width = width.max(1) as usize;
    let mut out: Vec<RenderedMarkdownLine> = Vec::with_capacity(parsed.blocks.len());

    let mut i = 0;
    while i < parsed.blocks.len() {
        if matches!(
            &parsed.blocks[i],
            Block::TableRow(_) | Block::TableSeparator
        ) {
            let start = i;
            while i < parsed.blocks.len()
                && matches!(
                    &parsed.blocks[i],
                    Block::TableRow(_) | Block::TableSeparator
                )
            {
                i += 1;
            }
            out.extend(
                render_table_group(&parsed.blocks[start..i], width, base_style)
                    .into_iter()
                    .map(|line| RenderedMarkdownLine {
                        line,
                        is_code: false,
                    }),
            );
            continue;
        }

        match &parsed.blocks[i] {
            Block::Heading { text, .. } => {
                let style = Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD);
                out.extend(render_wrapped_line_tagged(text, width, style, false, false));
            }
            Block::HeadingRule => {
                out.push(RenderedMarkdownLine {
                    line: Line::from(Span::styled(
                        "─".repeat(width.min(40)),
                        Style::default().fg(palette::TEXT_DIM),
                    )),
                    is_code: false,
                });
            }
            Block::HorizontalRule => {
                out.push(RenderedMarkdownLine {
                    line: Line::from(Span::styled(
                        "─".repeat(width.min(60)),
                        Style::default().fg(palette::TEXT_DIM),
                    )),
                    is_code: false,
                });
            }
            Block::ListItem { bullet, text } => {
                let bullet_style = Style::default().fg(palette::DEEPSEEK_SKY);
                out.extend(
                    render_list_line(bullet, text, width, bullet_style, base_style)
                        .into_iter()
                        .map(|line| RenderedMarkdownLine {
                            line,
                            is_code: false,
                        }),
                );
            }
            Block::Code { line } => {
                let code_style = Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::ITALIC);
                out.extend(render_wrapped_line_tagged(
                    line, width, code_style, true, true,
                ));
            }
            Block::Paragraph { text } => {
                let link_style = Style::default()
                    .fg(palette::DEEPSEEK_BLUE)
                    .add_modifier(Modifier::UNDERLINED);
                out.extend(
                    render_line_with_links(text, width, base_style, link_style)
                        .into_iter()
                        .map(|line| RenderedMarkdownLine {
                            line,
                            is_code: false,
                        }),
                );
            }
            Block::Blank => {
                out.push(RenderedMarkdownLine {
                    line: Line::from(""),
                    is_code: false,
                });
            }
            Block::TableRow(_) | Block::TableSeparator => unreachable!(),
        }
        i += 1;
    }

    if out.is_empty() {
        out.push(RenderedMarkdownLine {
            line: Line::from(""),
            is_code: false,
        });
    }

    out
}

/// Convenience wrapper: parse + render in one call.
///
/// Equivalent to `render_parsed(&parse(content), width, base_style)`. Callers
/// that don't manage their own cache (the Thinking body, the immediate message
/// body) use this.
#[must_use]
pub fn render_markdown(content: &str, width: u16, base_style: Style) -> Vec<Line<'static>> {
    let parsed = parse(content);
    render_parsed(&parsed, width, base_style)
}

/// Convenience wrapper: parse + render while keeping per-line source metadata.
#[must_use]
pub fn render_markdown_tagged(
    content: &str,
    width: u16,
    base_style: Style,
) -> Vec<RenderedMarkdownLine> {
    let parsed = parse(content);
    render_parsed_tagged(&parsed, width, base_style)
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 {
        return None;
    }
    let text = trimmed[hashes..].trim();
    if text.is_empty() {
        None
    } else {
        Some((hashes, text))
    }
}

fn parse_list_item(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return Some(("-".to_string(), trimmed[2..].trim()));
    }
    let bytes = trimmed.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == 0 || idx >= bytes.len() || bytes[idx] != b'.' {
        return None;
    }
    let rest = &trimmed[idx + 1..];
    if !rest.starts_with(' ') {
        return None;
    }
    Some((format!("{}.", &trimmed[..idx]), rest.trim_start()))
}

fn render_wrapped_line_tagged(
    line: &str,
    width: usize,
    style: Style,
    indent_code: bool,
    is_code: bool,
) -> Vec<RenderedMarkdownLine> {
    let prefix = if indent_code { "  " } else { "" };
    let prefix_width = prefix.width();
    let available = width.saturating_sub(prefix_width).max(1);
    // Code blocks must preserve leading whitespace (indentation is semantic).
    // Use hard character-width wrapping instead of word-wrap.
    let wrapped = if indent_code {
        wrap_code_line(line, available)
    } else {
        wrap_text(line, available)
    };
    let mut out = Vec::new();

    for (idx, chunk) in wrapped.into_iter().enumerate() {
        let line = if idx == 0 {
            Line::from(vec![Span::raw(prefix), Span::styled(chunk, style)])
        } else {
            Line::from(vec![
                Span::raw(" ".repeat(prefix_width)),
                Span::styled(chunk, style),
            ])
        };
        out.push(RenderedMarkdownLine { line, is_code });
    }

    out
}

fn render_list_line(
    bullet: &str,
    text: &str,
    width: usize,
    bullet_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let bullet_prefix = format!("{bullet} ");
    let bullet_width = bullet_prefix.width();
    let available = width.saturating_sub(bullet_width).max(1);
    let wrapped = render_line_with_links(text, available, text_style, link_style());

    let mut out = Vec::new();
    for (idx, line) in wrapped.into_iter().enumerate() {
        if idx == 0 {
            let mut spans = vec![Span::styled(bullet_prefix.clone(), bullet_style)];
            spans.extend(line.spans);
            out.push(Line::from(spans));
        } else {
            let mut spans = vec![Span::raw(" ".repeat(bullet_width))];
            spans.extend(line.spans);
            out.push(Line::from(spans));
        }
    }
    out
}

fn render_line_with_links(
    line: &str,
    width: usize,
    base_style: Style,
    link_style: Style,
) -> Vec<Line<'static>> {
    if line.trim().is_empty() {
        return vec![Line::from("")];
    }

    // Flatten inline tokens into (word, style) pairs preserving inter-token spaces.
    let tokens = parse_inline_spans(line, base_style, link_style);
    let mut words: Vec<InlineToken> = Vec::new();
    for token in tokens {
        let mut first = true;
        for part in token.text.split(' ') {
            if !first {
                // The space consumed by split — attach as a plain space word
                // so the wrap loop can decide whether to keep or break it.
                words.push(InlineToken::new(" ".to_string(), token.style, None));
            }
            if !part.is_empty() {
                words.push(InlineToken::new(
                    part.to_string(),
                    token.style,
                    token.link_url.clone(),
                ));
            }
            first = false;
        }
    }

    let mut lines = Vec::new();
    let mut current_spans: Vec<Span> = Vec::new();
    let mut current_width = 0usize;

    for word in words {
        let ww = word.text.width();
        if word.text == " " {
            // Space: emit only if we're mid-line and it fits; otherwise drop
            // (it's a potential wrap point, not content).
            if !current_spans.is_empty() && current_width < width {
                current_spans.push(Span::raw(" "));
                current_width += 1;
            }
            continue;
        }
        // If the word itself is wider than an entire line, hard-break it at
        // character boundaries so wrapping always makes progress (#1344,
        // #1351). Without this, long URLs / paths / hashes were placed on
        // their own line whole and silently overflowed the right edge of
        // the transcript.
        if ww > width && width > 0 {
            // Flush the in-progress line first.
            if !current_spans.is_empty() {
                if let Some(last) = current_spans.last()
                    && last.content.as_ref() == " "
                {
                    current_spans.pop();
                }
                lines.push(Line::from(std::mem::take(&mut current_spans)));
                current_width = 0;
            }
            // Char-break the word into width-sized chunks. Each full chunk
            // becomes its own line; the final partial chunk continues the
            // current line so the next word can pack onto it.
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            for ch in word.text.chars() {
                let cw = ch.width().unwrap_or(1);
                if chunk_w + cw > width && chunk_w > 0 {
                    lines.push(Line::from(vec![word.span_for(std::mem::take(&mut chunk))]));
                    chunk_w = 0;
                }
                chunk.push(ch);
                chunk_w += cw;
            }
            if !chunk.is_empty() {
                current_spans.push(word.span_for(chunk));
                current_width = chunk_w;
            }
            continue;
        }
        // Wrap before this word if it doesn't fit.
        if current_width > 0 && current_width + ww > width {
            // Trim trailing space span before breaking.
            if let Some(last) = current_spans.last()
                && last.content.as_ref() == " "
            {
                current_spans.pop();
            }
            lines.push(Line::from(current_spans));
            current_spans = Vec::new();
            current_width = 0;
        }
        current_spans.push(word.into_span());
        current_width += ww;
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

#[derive(Clone)]
struct InlineToken {
    text: String,
    style: Style,
    link_url: Option<String>,
}

impl InlineToken {
    fn new(text: String, style: Style, link_url: Option<String>) -> Self {
        Self {
            text,
            style,
            link_url,
        }
    }

    fn span_for(&self, text: String) -> Span<'static> {
        let content = match &self.link_url {
            Some(url) if osc8::enabled() => osc8::wrap_link(url, &text),
            _ => text,
        };
        Span::styled(content, self.style)
    }

    fn into_span(self) -> Span<'static> {
        let Self {
            text,
            style,
            link_url,
        } = self;
        let content = match link_url {
            Some(url) if osc8::enabled() => osc8::wrap_link(&url, &text),
            _ => text,
        };
        Span::styled(content, style)
    }
}

/// Parse an entire line into (text, style) segments, handling **bold**,
/// *italic*, `code`, ~~strikethrough~~, `[text](url)` links, and bare URLs.
fn parse_inline_spans(line: &str, base_style: Style, link_style: Style) -> Vec<InlineToken> {
    let bold_style = base_style.add_modifier(Modifier::BOLD);
    let italic_style = base_style.add_modifier(Modifier::ITALIC);
    let code_style = base_style
        .add_modifier(Modifier::ITALIC)
        .bg(palette::SURFACE_ELEVATED);
    let strike_style = base_style.add_modifier(Modifier::CROSSED_OUT);
    let mut out = Vec::new();
    let mut rest = line;

    while !rest.is_empty() {
        // **bold**
        if let Some(end) = rest.strip_prefix("**").and_then(|s| s.find("**")) {
            let inner = &rest[2..2 + end];
            out.push(InlineToken::new(inner.to_string(), bold_style, None));
            rest = &rest[2 + end + 2..];
            continue;
        }
        // __bold__
        if let Some(end) = rest.strip_prefix("__").and_then(|s| s.find("__")) {
            let inner = &rest[2..2 + end];
            out.push(InlineToken::new(inner.to_string(), bold_style, None));
            rest = &rest[2 + end + 2..];
            continue;
        }
        // *italic*
        if rest.starts_with('*')
            && !rest.starts_with("**")
            && let Some(end) = rest[1..].find('*')
        {
            let inner = &rest[1..1 + end];
            let after = &rest[1 + end + 1..];
            // Closing delimiter must not be immediately followed by a
            // letter, digit, or underscore (otherwise it's part of an
            // identifier like `deepseek_tui`, not italic markup).
            if !after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                out.push(InlineToken::new(inner.to_string(), italic_style, None));
                rest = after;
                continue;
            }
        }
        // _italic_
        if rest.starts_with('_')
            && !rest.starts_with("__")
            && let Some(end) = rest[1..].find('_')
        {
            let inner = &rest[1..1 + end];
            let after = &rest[1 + end + 1..];
            // Closing delimiter must not be immediately followed by a
            // letter, digit, or underscore.
            if !after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                out.push(InlineToken::new(inner.to_string(), italic_style, None));
                rest = after;
                continue;
            }
        }
        // `inline code`
        if let Some(end) = rest.strip_prefix('`').and_then(|s| s.find('`')) {
            let inner = &rest[1..1 + end];
            out.push(InlineToken::new(inner.to_string(), code_style, None));
            rest = &rest[1 + end + 1..];
            continue;
        }
        // ~~strikethrough~~
        if let Some(end) = rest.strip_prefix("~~").and_then(|s| s.find("~~")) {
            let inner = &rest[2..2 + end];
            out.push(InlineToken::new(inner.to_string(), strike_style, None));
            rest = &rest[2 + end + 2..];
            continue;
        }
        // [text](url)
        if rest.starts_with('[')
            && let Some(bracket_end) = rest.find(']')
        {
            let text = &rest[1..bracket_end];
            let after_bracket = &rest[bracket_end + 1..];
            if after_bracket.starts_with('(')
                && let Some(paren_end) = after_bracket.find(')')
            {
                let url = &after_bracket[1..paren_end];
                if osc8::enabled() {
                    out.push(InlineToken::new(
                        text.to_string(),
                        link_style,
                        Some(url.to_string()),
                    ));
                } else {
                    out.push(InlineToken::new(
                        format!("{text} ({url})"),
                        link_style,
                        None,
                    ));
                }
                rest = &after_bracket[paren_end + 1..];
                continue;
            }
        }
        // URL: consume until whitespace
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let url = &rest[..end];
            if osc8::enabled() {
                out.push(InlineToken::new(
                    url.to_string(),
                    link_style,
                    Some(url.to_string()),
                ));
            } else {
                out.push(InlineToken::new(url.to_string(), link_style, None));
            }
            rest = &rest[end..];
            continue;
        }
        // Plain text: consume until next marker or URL; always advance at least 1 char.
        let next = find_next_marker(rest).max(rest.chars().next().map_or(1, |c| c.len_utf8()));
        out.push(InlineToken::new(rest[..next].to_string(), base_style, None));
        rest = &rest[next..];
    }
    out
}

/// Find the index of the next inline marker (`**`, `__`, `*`, `_`, `http`)
/// in `s`, or `s.len()` if none found.
fn find_next_marker(s: &str) -> usize {
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        let ch_len = s[i..].chars().next().map_or(1, |c| c.len_utf8());
        let slice = &s[i..];
        if slice.starts_with("**")
            || slice.starts_with("__")
            || slice.starts_with("~~")
            || slice.starts_with('`')
            || slice.starts_with('[')
            || (slice.starts_with('*') && !slice.starts_with("**"))
            || (slice.starts_with('_') && !slice.starts_with("__"))
            || slice.starts_with("http://")
            || slice.starts_with("https://")
        {
            return i;
        }
        i += ch_len;
    }
    s.len()
}

fn is_horizontal_rule(line: &str) -> bool {
    let stripped: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    (stripped.chars().all(|c| c == '-')
        || stripped.chars().all(|c| c == '*')
        || stripped.chars().all(|c| c == '_'))
        && stripped.len() >= 3
}

/// Parse a markdown table row like `| foo | bar |` into trimmed cell strings.
/// Returns `None` for separator rows (`|---|---|`).
fn parse_table_row(line: &str) -> Option<Vec<String>> {
    if !line.starts_with('|') {
        return None;
    }
    let inner = line.trim_matches('|');
    let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_string()).collect();
    // Separator row: every non-empty cell is only dashes/colons/spaces
    if cells
        .iter()
        .all(|c| c.is_empty() || c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
    {
        return None;
    }
    Some(cells)
}

/// Word-wrap a single cell's text into one or more visual lines, each
/// constrained to `col_width` display columns. Whitespace is the preferred
/// break point; words wider than `col_width` are hard-broken at character
/// boundaries so wrapping always makes progress (no infinite loop on URLs
/// or paths). Returns at least one segment.
fn wrap_cell_text(cell: &str, col_width: usize) -> Vec<String> {
    if cell.is_empty() || cell.width() <= col_width {
        return vec![cell.to_string()];
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;

    for word in cell.split_whitespace() {
        let word_w = word.width();
        if current_w == 0 {
            if word_w > col_width {
                push_word_breaking_chars(word, col_width, &mut current, &mut current_w, &mut lines);
            } else {
                current.push_str(word);
                current_w = word_w;
            }
        } else if current_w + 1 + word_w <= col_width {
            current.push(' ');
            current.push_str(word);
            current_w += 1 + word_w;
        } else {
            lines.push(std::mem::take(&mut current));
            current_w = 0;
            if word_w > col_width {
                push_word_breaking_chars(word, col_width, &mut current, &mut current_w, &mut lines);
            } else {
                current.push_str(word);
                current_w = word_w;
            }
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn render_table_row(cells: &[String], width: usize, base_style: Style) -> Vec<Line<'static>> {
    if cells.is_empty() {
        return vec![Line::from("")];
    }
    let col_width = (width.saturating_sub(3 * cells.len() + 1)) / cells.len();
    let col_width = col_width.max(4);
    let sep_style = Style::default().fg(palette::TEXT_DIM);

    // Wrap each cell into one or more visual segments. The row's visual
    // height equals the tallest column. Cells that wrap to fewer segments
    // get blank-padded continuation lines so column separators stay aligned.
    let wrapped: Vec<Vec<String>> = cells.iter().map(|c| wrap_cell_text(c, col_width)).collect();
    let row_height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(row_height);
    for row in 0..row_height {
        let mut spans: Vec<Span> = vec![Span::styled("│ ".to_string(), sep_style)];
        for (i, cell_segments) in wrapped.iter().enumerate() {
            let segment = cell_segments.get(row).map(String::as_str).unwrap_or("");
            let cell_spans = parse_inline_spans(segment, base_style, link_style());
            let cell_width: usize = cell_spans.iter().map(|token| token.text.width()).sum();
            let pad = col_width.saturating_sub(cell_width);
            for token in cell_spans {
                spans.push(token.into_span());
            }
            spans.push(Span::raw(" ".repeat(pad)));
            if i + 1 < cells.len() {
                spans.push(Span::styled(" │ ".to_string(), sep_style));
            } else {
                spans.push(Span::styled(" │".to_string(), sep_style));
            }
        }
        lines.push(Line::from(spans));
    }
    lines
}

fn table_col_width(num_cols: usize, term_width: usize) -> usize {
    let col_width = (term_width.saturating_sub(3 * num_cols + 1)) / num_cols;
    col_width.max(4)
}

fn render_table_border(
    num_cols: usize,
    col_width: usize,
    sep_style: Style,
    left: &str,
    mid: &str,
    right: &str,
) -> Line<'static> {
    let fill = "\u{2500}".repeat(col_width);
    let mut s = String::new();
    s.push_str(left);
    for i in 0..num_cols {
        s.push_str(&fill);
        if i + 1 < num_cols {
            s.push_str(mid);
        } else {
            s.push_str(right);
        }
    }
    Line::from(Span::styled(s, sep_style))
}

fn render_table_group(blocks: &[Block], width: usize, base_style: Style) -> Vec<Line<'static>> {
    let sep_style = Style::default().fg(palette::TEXT_DIM);

    let num_cols = blocks
        .iter()
        .filter_map(|b| match b {
            Block::TableRow(cells) => Some(cells.len()),
            _ => None,
        })
        .max()
        .unwrap_or(1);

    let col_width = table_col_width(num_cols, width);

    let mut lines = Vec::new();

    // Top border
    lines.push(render_table_border(
        num_cols,
        col_width,
        sep_style,
        "\u{250C}\u{2500}",
        "\u{2500}\u{252C}\u{2500}",
        "\u{2500}\u{2510}",
    ));

    let mid_border = || {
        render_table_border(
            num_cols,
            col_width,
            sep_style,
            "\u{251C}\u{2500}",
            "\u{2500}\u{253C}\u{2500}",
            "\u{2500}\u{2524}",
        )
    };

    for i in 0..blocks.len() {
        match &blocks[i] {
            Block::TableRow(cells) => {
                lines.extend(render_table_row(cells, width, base_style));
                if i + 1 < blocks.len() && matches!(&blocks[i + 1], Block::TableRow(_)) {
                    lines.push(mid_border());
                }
            }
            Block::TableSeparator => {
                lines.push(mid_border());
            }
            _ => {}
        }
    }

    // Bottom border
    lines.push(render_table_border(
        num_cols,
        col_width,
        sep_style,
        "\u{2514}\u{2500}",
        "\u{2500}\u{2534}\u{2500}",
        "\u{2500}\u{2518}",
    ));

    lines
}

fn link_style() -> Style {
    Style::default()
        .fg(palette::DEEPSEEK_BLUE)
        .add_modifier(Modifier::UNDERLINED)
}

/// Hard-wrap a code line at `width` display columns, preserving all
/// whitespace (including leading indentation). Unlike [`wrap_text`], this
/// does not split on word boundaries — code indentation is semantic.
/// Display-column width of a single character for the purposes of terminal
/// line-wrap calculations.
///
/// `UnicodeWidthChar::width` returns `None` for control characters, which
/// includes `\t`. A tab advances to the next 8-column tab stop, so we model
/// it as 8 columns here (a safe over-estimate that avoids terminal overflow).
/// Other control characters are counted as 1 column.
fn char_display_width(ch: char, col: usize) -> usize {
    match ch {
        '\t' => 8 - (col % 8), // advance to next 8-column tab stop
        _ => ch.width().unwrap_or(1),
    }
}

/// Hard-wrap a code line at `width` display columns, preserving all
/// whitespace (including leading indentation). Unlike [`wrap_text`], this
/// does not split on word boundaries — code indentation is semantic.
fn wrap_code_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 || line.is_empty() {
        return vec![line.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in line.chars() {
        let ch_width = char_display_width(ch, current_width);
        if current_width + ch_width > width && !current.is_empty() {
            chunks.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }
    chunks.push(current);
    chunks
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = word.width();
        // If this single word is wider than the entire line, hard-break it
        // at character boundaries so wrapping always makes progress
        // (#1344, #1351). Without this, long URLs / paths / hashes overflow
        // the right edge silently.
        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            push_word_breaking_chars(word, width, &mut current, &mut current_width, &mut lines);
            continue;
        }
        let additional = if current.is_empty() {
            word_width
        } else {
            word_width + 1
        };
        if current_width + additional > width && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
        }
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

/// Push characters from `word` into `current`, flushing to `lines` when the
/// running display width would exceed `width`. Width is computed at the
/// `unicode-width` char level, matching the rest of the rendering pipeline.
/// Used by `wrap_text` and `wrap_cell_text` so a word longer than the
/// allotted width never silently overflows the right edge.
fn push_word_breaking_chars(
    word: &str,
    width: usize,
    current: &mut String,
    current_width: &mut usize,
    lines: &mut Vec<String>,
) {
    for ch in word.chars() {
        let cw = ch.width().unwrap_or(1);
        if *current_width + cw > width && *current_width > 0 {
            lines.push(std::mem::take(current));
            *current_width = 0;
        }
        current.push(ch);
        *current_width += cw;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn underscores_inside_identifiers_render_as_literal_text() {
        // Regression for PR #1455 / @tiger-dog: previously the inline
        // markdown parser ate the underscore in `deepseek_tui` because
        // it matched the `_italic_` pattern without a CommonMark-style
        // boundary check. The closing `_` followed by `t` (a letter)
        // must now be treated as part of the identifier, not as
        // markup. The same rule applies to `*` so identifiers like
        // `crate*foo` round-trip cleanly.
        let cases = [
            "crate deepseek_tui handles approvals",
            "see foo_bar_baz for details",
            "look at *not_emphasised*tail",
        ];
        for source in cases {
            let parsed = parse(source);
            let rendered: String = render_parsed(&parsed, 80, Style::default())
                .iter()
                .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
                .collect();
            // The original identifier (with underscores intact) must
            // appear in the rendered output. We don't assert on style
            // here — that's an implementation detail; we assert on
            // the user-visible character sequence.
            for token in source.split_whitespace().filter(|t| t.contains('_')) {
                assert!(
                    rendered.contains(token),
                    "identifier {token:?} must survive markdown rendering of {source:?}; got {rendered:?}"
                );
            }
        }
    }

    #[test]
    fn render_markdown_matches_parse_then_render() {
        // Both calls run in the same thread under the same OSC8 lock so the
        // flag is identical for both paths.
        let source = "# Title\n\nA paragraph with a https://example.com link.\n\n- one\n- two\n```\ncode\n```";
        let direct = render_with_osc8(false, source);
        let two_step = with_osc8(false, || {
            let parsed = parse(source);
            render_parsed(&parsed, 80, Style::default())
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect::<String>()
        });
        assert_eq!(direct, two_step);
    }

    #[test]
    fn parse_is_width_independent() {
        // Same source, two parses, must produce identical AST. (Sanity:
        // parse must not depend on hidden global state like terminal width.)
        let source = "Hello\n\n## Heading\n- list\n";
        let a = parse(source);
        let b = parse(source);
        assert_eq!(a, b);
    }

    #[test]
    fn render_parsed_word_wrap_changes_with_width() {
        // The same AST must produce different layouts at different widths;
        // otherwise the split is decorative, not functional.
        let parsed = parse("alpha beta gamma delta epsilon zeta");
        let wide = render_parsed(&parsed, 80, Style::default());
        let narrow = render_parsed(&parsed, 10, Style::default());
        assert!(
            narrow.len() > wide.len(),
            "narrow should produce more lines"
        );
    }

    #[test]
    fn parse_invocations_increment() {
        // Counter is thread-local, so concurrent tests calling `parse()`
        // can't pollute each other.
        reset_parse_invocation_count();
        let _ = parse("hello\n");
        let _ = parse("world\n");
        assert_eq!(parse_invocation_count(), 2);
    }

    #[test]
    fn render_parsed_does_not_call_parse() {
        // Width-only changes must hit only the render path. This is the
        // perf invariant CX#6 was filed for.
        let parsed = parse("multiline\nsource\nwith several\nlines\n");
        reset_parse_invocation_count();
        let _ = render_parsed(&parsed, 80, Style::default());
        let _ = render_parsed(&parsed, 40, Style::default());
        let _ = render_parsed(&parsed, 20, Style::default());
        assert_eq!(
            parse_invocation_count(),
            0,
            "render_parsed must not call parse"
        );
    }

    #[test]
    fn fenced_code_block_collected_in_parse() {
        let parsed = parse("text\n```\ncode line one\ncode line two\n```\nmore\n");
        let blocks = &parsed.blocks;
        // text paragraph, two code lines, more paragraph (fences are dropped)
        let code_lines: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Code { line } => Some(line.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(code_lines, vec!["code line one", "code line two"]);
    }

    #[test]
    fn code_block_indentation_is_preserved_in_render() {
        // Leading whitespace in code blocks is semantic — indented lines must
        // not be stripped to column zero when rendered.
        let md = "```\nfn main() {\n    println!(\"hi\");\n}\n```\n";
        let lines = render_markdown(md, 80, Style::default());
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        // The indented line must start with spaces (the 2-space code prefix
        // plus the 4-space source indentation).
        let indented = text
            .iter()
            .find(|t| t.contains("println"))
            .expect("should find println line");
        assert!(
            indented.starts_with("      "),
            "expected 6+ leading spaces (2 block prefix + 4 indent), got: {indented:?}"
        );
    }

    #[test]
    fn wrap_code_line_preserves_leading_whitespace() {
        // A short line must not be modified.
        assert_eq!(wrap_code_line("    let x = 1;", 80), vec!["    let x = 1;"]);

        // A line that exceeds the width must be hard-wrapped, keeping the
        // leading whitespace on the first chunk.
        let chunks = wrap_code_line("    abcdefgh", 8);
        assert_eq!(chunks[0], "    abcd", "first chunk keeps leading spaces");
        assert_eq!(chunks[1], "efgh");

        // Empty line produces one empty chunk.
        assert_eq!(wrap_code_line("", 80), vec![""]);
    }

    #[test]
    fn wrap_code_line_tab_counts_toward_width() {
        // tab (8 cols) + "xy" (2 cols) = 10 ≤ 10 — fits on one line.
        let chunks = wrap_code_line("\txy", 10);
        assert_eq!(chunks, vec!["\txy"], "tab + 2 chars fits in width 10");

        // tab (8 cols) + "x" (1 col) = 9 ≤ 9 — "x" fits; "y" overflows.
        let chunks = wrap_code_line("\txy", 9);
        assert_eq!(chunks[0], "\tx", "tab + first char fits exactly");
        assert_eq!(chunks[1], "y", "second char wraps");

        // tab alone (8 cols) fits in width 8; the next "x" overflows.
        let chunks = wrap_code_line("\tx", 8);
        assert_eq!(chunks[0], "\t");
        assert_eq!(chunks[1], "x");
    }

    #[test]
    fn char_display_width_tab_uses_tab_stop() {
        // At column 0 a tab fills to column 8.
        assert_eq!(char_display_width('\t', 0), 8);
        // At column 4 a tab fills to column 8 (4 remaining).
        assert_eq!(char_display_width('\t', 4), 4);
        // At column 8 a tab fills to the next stop at 16 (8 columns).
        assert_eq!(char_display_width('\t', 8), 8);
        // Regular ASCII is 1.
        assert_eq!(char_display_width('a', 0), 1);
    }

    #[test]
    fn ordered_and_unordered_list_items_parse() {
        let parsed = parse("- alpha\n* beta\n1. gamma\n");
        let items: Vec<_> = parsed
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem { bullet, text } => Some((bullet.as_str(), text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(items, vec![("-", "alpha"), ("-", "beta"), ("1.", "gamma")]);
    }

    /// Render with the OSC 8 flag pinned to `enabled`, then restore the prior
    /// value. We serialize through a static mutex because `osc8::ENABLED` is
    /// process-wide state and other tests touching it would race otherwise.
    fn render_with_osc8(enabled: bool, source: &str) -> String {
        render_with_osc8_width(enabled, source, 80)
    }

    fn render_with_osc8_width(enabled: bool, source: &str, width: u16) -> String {
        with_osc8(enabled, || {
            render_markdown(source, width, Style::default())
                .iter()
                .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
                .collect::<String>()
        })
    }

    fn with_osc8<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
        use std::sync::Mutex;
        static OSC8_GUARD: Mutex<()> = Mutex::new(());
        let _guard = OSC8_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let prior = osc8::enabled();
        osc8::set_enabled(enabled);
        let result = f();
        osc8::set_enabled(prior);
        result
    }

    #[test]
    fn http_links_get_osc_8_wrapped_when_enabled() {
        let joined = render_with_osc8(true, "see https://example.com for details");
        assert!(
            joined.contains("\x1b]8;;https://example.com\x1b\\https://example.com\x1b]8;;\x1b\\"),
            "expected OSC 8 wrapper around URL; got {joined:?}"
        );
    }

    #[test]
    fn wrapped_osc_8_url_chunks_keep_full_link_target() {
        let url = "https://raw.githubusercontent.com/Hmbown/deepseek-skills/main/index.json";
        let joined = render_with_osc8_width(true, url, 34);
        let full_target = format!("\x1b]8;;{url}\x1b\\");

        assert!(
            joined.matches(&full_target).count() > 1,
            "expected each wrapped URL chunk to reopen the full OSC 8 target; got {joined:?}"
        );
        assert!(
            !joined.contains(
                "\x1b]8;;https://raw.githubusercontent.com/Hmbown/deepseek-skills/main/inde\x1b\\"
            ),
            "wrapped link must not expose a truncated OSC 8 target: {joined:?}"
        );
    }

    #[test]
    fn osc_8_disabled_emits_plain_url() {
        let joined = render_with_osc8(false, "see https://example.com for details");
        assert!(
            !joined.contains("\x1b]8;;"),
            "expected no OSC 8 wrapper when disabled; got {joined:?}"
        );
        assert!(joined.contains("https://example.com"));
    }

    #[test]
    fn table_separator_row_is_kept() {
        // Separator rows are now kept as TableSeparator blocks so the
        // renderer can draw horizontal rules at the correct positions.
        let src = "| 项目属性 | 详情 |\n|----------|------|\n| **语言** | Rust 1.88+ |\n";
        let parsed = parse(src);
        let blocks: Vec<_> = parsed.blocks.iter().collect();
        // Should have 2 TableRow blocks (header + data) + 1 TableSeparator
        let table_rows: Vec<_> = blocks
            .iter()
            .filter(|b| matches!(b, Block::TableRow(_)))
            .collect();
        assert_eq!(table_rows.len(), 2, "expected 2 table rows: {blocks:?}");
        let separators: Vec<_> = blocks
            .iter()
            .filter(|b| matches!(b, Block::TableSeparator))
            .collect();
        assert_eq!(
            separators.len(),
            1,
            "expected 1 table separator: {blocks:?}"
        );
    }

    #[test]
    fn bold_markers_stripped_in_render() {
        let src = "这是一个 **Rust 工作区项目**，包含多个 crate。\n";
        let lines = render_markdown(src, 80, Style::default());
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert!(
            !text.contains("**"),
            "bold markers leaked into output: {text:?}"
        );
        assert!(text.contains("Rust"), "bold content missing: {text:?}");
    }

    #[test]
    fn table_renders_with_box_drawing_borders() {
        let src = "| 文件 | 改动 |\n|---|---|\n| foo.rs | 重写 |\n";
        let lines = render_markdown(src, 60, Style::default());
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        // Column pipes still present
        assert!(text.contains('│'), "table pipe separator missing: {text:?}");
        // Separator row rendered as middle border, not raw markdown
        assert!(
            !text.contains("|---|"),
            "raw separator row leaked: {text:?}"
        );
        // Top and bottom borders present
        assert!(
            text.contains('\u{250C}'),
            "top-left corner missing: {text:?}"
        );
        assert!(
            text.contains('\u{2510}'),
            "top-right corner missing: {text:?}"
        );
        assert!(
            text.contains('\u{2514}'),
            "bottom-left corner missing: {text:?}"
        );
        assert!(
            text.contains('\u{2518}'),
            "bottom-right corner missing: {text:?}"
        );
        // Middle separator present (at the |---|---| position)
        assert!(
            text.contains('\u{251C}'),
            "middle-left junction missing: {text:?}"
        );
        assert!(
            text.contains('\u{2524}'),
            "middle-right junction missing: {text:?}"
        );
    }

    /// Cells longer than the per-column width must word-wrap to multiple
    /// lines instead of getting truncated with `…`. Truncation silently
    /// drops content the user can never see — particularly bad in narrow
    /// Windows terminals or with verbose English/Chinese instructional
    /// tables (the common LLM-output case).
    #[test]
    fn table_cell_wider_than_column_wraps_instead_of_truncating() {
        let src = "| Feature | How to verify |\n\
                   |---|---|\n\
                   | Workspace-local commands | Drop a .deepseek/commands/foo.md in any project, run deepseek from there, type /foo — should dispatch |\n";
        let lines = render_markdown(src, 80, Style::default());
        let combined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        assert!(
            !combined.contains('…'),
            "table cell was truncated with `…` instead of wrapping; got: {combined:?}"
        );
        assert!(
            combined.contains("type /foo"),
            "tail of long cell was lost; got: {combined:?}"
        );
        assert!(
            combined.contains("Workspace-local commands"),
            "short cell content lost; got: {combined:?}"
        );
    }

    /// Wrapped table rows must keep column separators on every visual
    /// line so the columns remain visually aligned across all wrapped
    /// segments. A wrapped row's continuation lines should still show
    /// the `│` separator pipes at the same column positions.
    #[test]
    fn wrapped_table_row_preserves_column_separators() {
        let src = "| A | B |\n\
                   |---|---|\n\
                   | short | this is a very very long second cell that absolutely must wrap to a new visual line because it cannot fit in the column allocated to it at this terminal width |\n";
        let lines = render_markdown(src, 60, Style::default());
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        // Every line in the rendered table — including wrapped continuation
        // lines — must show the pipe column separator. We identify table
        // body lines as ones that start with the row separator `│`.
        let body_lines: Vec<&String> = rendered.iter().filter(|s| s.starts_with('│')).collect();

        assert!(
            body_lines.len() >= 3,
            "expected at least header + multi-line data row (3+ body lines), got {}: {:?}",
            body_lines.len(),
            body_lines
        );

        for line in &body_lines {
            assert!(
                line.matches('│').count() >= 3,
                "every wrapped table line should have N+1 column separators \
                 for N columns; got fewer in: {line:?}"
            );
        }

        // All of the long cell's content must appear across the wrapped lines.
        let combined: String = rendered.join("\n");
        for fragment in ["this is a very very long", "must wrap", "terminal width"] {
            assert!(
                combined.contains(fragment),
                "fragment {fragment:?} missing from wrapped output:\n{combined}"
            );
        }
    }

    // ─── Paragraph wrap regression suite (#1344, #1351) ────────────────────
    //
    // The bug: paragraph wrap (render_line_with_links) and code-block wrap
    // (wrap_text) are word-based. A single word wider than the available
    // width was placed alone on a line and silently overflowed the right
    // edge of the transcript. Long URLs / paths / hashes / no-whitespace
    // CJK runs all hit this. The fix hard-breaks overlong words at
    // character boundaries; these tests pin that across widths 40/60/80/120.

    fn rendered_widths(rendered: &[Line<'static>]) -> Vec<usize> {
        rendered
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref().width())
                    .sum::<usize>()
            })
            .collect()
    }

    fn render_paragraph_for_test(text: &str, width: usize) -> Vec<Line<'static>> {
        with_osc8(false, || {
            render_line_with_links(text, width, Style::default(), Style::default())
        })
    }

    #[test]
    fn paragraph_wrap_breaks_overlong_word_at_width_40() {
        // 200-char no-whitespace token must not exceed the 40-col window.
        let long = "a".repeat(200);
        let rendered = render_paragraph_for_test(&long, 40);
        for w in rendered_widths(&rendered) {
            assert!(w <= 40, "rendered width {w} exceeds 40-col window");
        }
        // And the full content must still be present across the wrapped lines.
        let combined: String = rendered
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert_eq!(combined.matches('a').count(), 200);
    }

    #[test]
    fn paragraph_wrap_breaks_overlong_word_at_widths_60_80_120() {
        let long = format!("https://example.com/{}", "p".repeat(180));
        for &width in &[60usize, 80, 120] {
            let rendered = render_paragraph_for_test(&long, width);
            for w in rendered_widths(&rendered) {
                assert!(
                    w <= width,
                    "width={width}: rendered line width {w} exceeds budget"
                );
            }
            assert!(rendered.len() >= 2, "width={width}: expected wrap");
        }
    }

    #[test]
    fn paragraph_wrap_keeps_short_words_unbroken() {
        // Regression guard: short words must still be broken at whitespace,
        // not mid-word. Width 40, only short words, expect zero mid-word
        // breaks (each line reads as natural English).
        let text = "the quick brown fox jumps over the lazy dog and then it stops moving";
        let rendered = render_paragraph_for_test(text, 40);
        for line in &rendered {
            let s: String = line.spans.iter().map(|s| s.content.to_string()).collect();
            // Heuristic: trimmed line should not start with a partial word
            // (i.e. should start with a real English start) — every line in
            // this fixture starts with a word in our short list.
            let first = s.split_whitespace().next().unwrap_or("");
            assert!(
                [
                    "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "and", "then",
                    "it", "stops", "moving"
                ]
                .contains(&first),
                "line {s:?} appears to start with a partial word"
            );
        }
    }

    #[test]
    fn paragraph_wrap_mixed_short_and_overlong_word() {
        // The overlong word must wrap; the trailing short words must pack
        // onto subsequent lines. The combined content is preserved.
        let long = "x".repeat(150);
        let text = format!("intro {long} tail words go here");
        let rendered = render_paragraph_for_test(&text, 80);
        for w in rendered_widths(&rendered) {
            assert!(w <= 80, "rendered width {w} exceeds 80-col window");
        }
        let combined: String = rendered
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        for fragment in ["intro", "tail", "words", "go", "here"] {
            assert!(
                combined.contains(fragment),
                "fragment {fragment:?} missing from wrapped output:\n{combined}"
            );
        }
        assert_eq!(combined.matches('x').count(), 150);
    }

    #[test]
    fn wrap_text_breaks_overlong_word_for_code_blocks() {
        // The standalone code-block wrap (wrap_text) had the same overflow
        // bug; pin the fix at widths 40 and 80.
        for &width in &[40usize, 80] {
            let long = "z".repeat(200);
            let lines = wrap_text(&long, width);
            for line in &lines {
                assert!(
                    line.width() <= width,
                    "wrap_text line {line:?} exceeds {width}"
                );
            }
            let combined: String = lines.join("");
            assert_eq!(combined.matches('z').count(), 200);
        }
    }

    #[test]
    fn wrap_cell_text_already_handled_long_words_remains_correct() {
        // Regression guard for the v0.8.25 table-cell fix. After consolidating
        // the char-break helper, wrap_cell_text must continue to handle
        // overlong cells. Pin the property: every wrapped segment fits
        // within the column width, and content is preserved.
        let long = "y".repeat(120);
        let segments = wrap_cell_text(&long, 30);
        for seg in &segments {
            assert!(seg.width() <= 30, "segment {seg:?} exceeds col 30");
        }
        let combined: String = segments.join("");
        assert_eq!(combined.matches('y').count(), 120);
    }

    #[test]
    fn paragraph_wrap_handles_zero_width_gracefully() {
        // Width 0 should not panic or hang; it returns the input as-is or
        // empty, but never produces a line wider than 0 (when 0 means "no
        // budget at all"). This pins the early-return path against future
        // regressions.
        let rendered = render_paragraph_for_test("hello world", 0);
        // Any output is acceptable (the path is degenerate); assert no panic.
        let _ = rendered;
    }
}
