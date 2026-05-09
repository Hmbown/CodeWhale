//! Fuzzy file-picker modal (Ctrl+P).
//!
//! Opens an overlay populated with workspace-relative paths discovered by a
//! single-pass `WalkBuilder` walk (depth 6, hidden=true, follow_links=false,
//! `.gitignore` honored). Subsequent keystrokes filter the cached candidate
//! list in memory using a small subsequence + first-letter-bonus scorer — no
//! per-keystroke disk traversal.
//!
//! Enter emits a [`ViewEvent::FilePickerSelected`] which the UI handler turns
//! into an `@<path>` insertion at the composer cursor.

use std::collections::HashSet;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ignore::WalkBuilder;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

/// Maximum number of candidates collected from the initial walk. Keeps memory
/// bounded for very large monorepos; matches the limits codex-rs uses for the
/// equivalent overlay.
const MAX_CANDIDATES: usize = 20_000;

/// Walk depth for the initial scan. Mirrors the `Workspace` fuzzy index.
const WALK_DEPTH: usize = 6;

/// Visible candidate rows in the overlay.
const VISIBLE_ROWS: usize = 14;

const MODIFIED_BOOST: i32 = 360;
const MENTIONED_BOOST: i32 = 240;
const TOOL_BOOST: i32 = 160;

/// Working-set hints captured when the picker opens.
///
/// The picker keeps this as plain path strings so filtering stays in-memory and
/// per-keystroke work remains the same shape as the original fuzzy search.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilePickerRelevance {
    modified: HashSet<String>,
    mentioned: HashSet<String>,
    tool: HashSet<String>,
}

impl FilePickerRelevance {
    pub fn mark_modified(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !path.is_empty() {
            self.modified.insert(path);
        }
    }

    pub fn mark_mentioned(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !path.is_empty() {
            self.mentioned.insert(path);
        }
    }

    pub fn mark_tool(&mut self, path: impl Into<String>) {
        let path = path.into();
        if !path.is_empty() {
            self.tool.insert(path);
        }
    }

    fn boost_for(&self, path: &str) -> i32 {
        let mut boost = 0;
        if self.modified.contains(path) {
            boost += MODIFIED_BOOST;
        }
        if self.mentioned.contains(path) {
            boost += MENTIONED_BOOST;
        }
        if self.tool.contains(path) {
            boost += TOOL_BOOST;
        }
        boost
    }

    fn markers_for(&self, path: &str) -> String {
        let mut markers = String::with_capacity(3);
        markers.push(if self.modified.contains(path) {
            'M'
        } else {
            ' '
        });
        markers.push(if self.mentioned.contains(path) {
            '@'
        } else {
            ' '
        });
        markers.push(if self.tool.contains(path) { 'T' } else { ' ' });
        markers
    }
}

pub struct FilePickerView {
    /// All workspace-relative candidate paths, captured once at construction.
    candidates: Vec<String>,
    /// Working-set relevance hints, captured once at construction.
    relevance: FilePickerRelevance,
    /// Filtered indices into `candidates`, sorted by descending score.
    filtered: Vec<usize>,
    /// User's typed query (lowercased on each refilter).
    query: String,
    /// Selected row within `filtered`.
    selected: usize,
    /// Top of the visible window within `filtered`.
    scroll: usize,
}

impl FilePickerView {
    /// Build a picker with working-set relevance hints.
    pub fn new_with_relevance(workspace_root: &Path, relevance: FilePickerRelevance) -> Self {
        let candidates = collect_candidates(workspace_root);
        let mut view = Self {
            candidates,
            relevance,
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
            scroll: 0,
        };
        view.refilter();
        view
    }

    fn refilter(&mut self) {
        let query = self.query.trim().to_lowercase();
        let mut scored: Vec<(usize, i32, i32, i32)> = if query.is_empty() {
            self.candidates
                .iter()
                .enumerate()
                .map(|(idx, path)| {
                    let boost = self.relevance.boost_for(path);
                    (idx, boost, 0, boost)
                })
                .collect()
        } else {
            self.candidates
                .iter()
                .enumerate()
                .filter_map(|(idx, path)| {
                    score(&query, path).map(|fuzzy| {
                        let boost = self.relevance.boost_for(path);
                        (idx, fuzzy + boost, fuzzy, boost)
                    })
                })
                .collect()
        };

        // Higher scores first; tie-break by ascending path length, then lex order
        // so shorter / more central matches surface above deep nested ones.
        scored.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| b.3.cmp(&a.3))
                .then_with(|| self.candidates[a.0].len().cmp(&self.candidates[b.0].len()))
                .then_with(|| self.candidates[a.0].cmp(&self.candidates[b.0]))
        });

        self.filtered = scored.into_iter().map(|(idx, _, _, _)| idx).collect();
        if self.filtered.is_empty() {
            self.selected = 0;
            self.scroll = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
        self.adjust_scroll();
    }

    fn adjust_scroll(&mut self) {
        if self.filtered.is_empty() {
            self.scroll = 0;
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + VISIBLE_ROWS {
            self.scroll = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len() - 1;
        let next = if delta.is_negative() {
            self.selected.saturating_sub(delta.unsigned_abs())
        } else {
            (self.selected + delta as usize).min(max)
        };
        self.selected = next;
        self.adjust_scroll();
    }

    fn selected_path(&self) -> Option<&str> {
        let idx = *self.filtered.get(self.selected)?;
        self.candidates.get(idx).map(String::as_str)
    }

    /// Visible candidate count for tests / diagnostics.
    #[cfg(test)]
    pub fn visible_count(&self) -> usize {
        self.filtered.len()
    }

    #[cfg(test)]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[cfg(test)]
    pub fn selected_for_test(&self) -> Option<&str> {
        self.selected_path()
    }

    #[cfg(test)]
    pub fn markers_for_test(&self, path: &str) -> String {
        self.relevance.markers_for(path)
    }
}

impl ModalView for FilePickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::FilePicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Enter => {
                if let Some(path) = self.selected_path() {
                    let path = path.to_string();
                    return ViewAction::EmitAndClose(ViewEvent::FilePickerSelected { path });
                }
                ViewAction::Close
            }
            KeyCode::Up => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.move_selection(-(VISIBLE_ROWS as isize));
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.move_selection(VISIBLE_ROWS as isize);
                ViewAction::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.selected = 0;
                self.scroll = 0;
                self.refilter();
                ViewAction::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                self.selected = 0;
                self.scroll = 0;
                self.refilter();
                ViewAction::None
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && !ch.is_control() =>
            {
                self.query.push(ch);
                self.selected = 0;
                self.scroll = 0;
                self.refilter();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 80.min(area.width.saturating_sub(4));
        let popup_height = ((VISIBLE_ROWS as u16) + 6).min(area.height.saturating_sub(4));

        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let title = Line::from(vec![Span::styled(
            " File Picker ",
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )]);
        let footer_text = format!(
            " {} match{}  ↑/↓ select  Enter insert @path  Esc close ",
            self.filtered.len(),
            if self.filtered.len() == 1 { "" } else { "es" },
        );
        let block = Block::default()
            .title(title)
            .title_bottom(Line::from(Span::styled(
                footer_text,
                Style::default().fg(palette::TEXT_MUTED),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines: Vec<Line<'static>> = Vec::new();
        // Query line.
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
            Span::raw(self.query.clone()),
            Span::styled(
                " ",
                Style::default()
                    .fg(palette::DEEPSEEK_INK)
                    .bg(palette::DEEPSEEK_SKY),
            ),
        ]));
        lines.push(Line::from(""));

        let visible = VISIBLE_ROWS.min(inner.height.saturating_sub(2) as usize);
        let end = (self.scroll + visible).min(self.filtered.len());
        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No matches",
                Style::default().fg(palette::TEXT_MUTED),
            )));
        } else {
            for idx in self.scroll..end {
                let path = &self.candidates[self.filtered[idx]];
                let selected = idx == self.selected;
                let style = if selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                } else {
                    Style::default().fg(palette::TEXT_PRIMARY)
                };
                let prefix = if selected { "▶ " } else { "  " };
                let marker_field = if inner.width >= 18 {
                    format!("{} ", self.relevance.markers_for(path))
                } else {
                    String::new()
                };
                let reserved = prefix.chars().count() + marker_field.chars().count();
                let display = truncate_path(path, (inner.width as usize).saturating_sub(reserved));
                let mut line = Line::from(format!("{prefix}{marker_field}{display}"));
                line.style = style;
                lines.push(line);
            }
        }

        Paragraph::new(lines)
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .render(inner, buf);
    }
}

fn truncate_path(path: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if path.chars().count() <= max {
        return path.to_string();
    }
    let take = max.saturating_sub(1);
    let truncated: String = path
        .chars()
        .rev()
        .take(take)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{truncated}")
}

/// Single-pass walk that collects workspace-relative paths.
fn collect_candidates(root: &Path) -> Vec<String> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(true)
        .follow_links(false)
        .max_depth(Some(WALK_DEPTH))
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true);

    let mut out: Vec<String> = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(path);
        if rel.as_os_str().is_empty() {
            continue;
        }
        let display = path_to_workspace_string(rel);
        if !display.is_empty() {
            out.push(display);
        }
        if out.len() >= MAX_CANDIDATES {
            break;
        }
    }

    // Whitelist AI-tool dot-directories so they're discoverable even when
    // gitignored. Walk each one separately with gitignore disabled.
    for dir in [".deepseek", ".cursor", ".claude", ".agents"] {
        let dot_dir = root.join(dir);
        if !dot_dir.is_dir() {
            continue;
        }
        let mut dot_builder = WalkBuilder::new(&dot_dir);
        dot_builder
            .hidden(true)
            .follow_links(false)
            .git_ignore(false)
            .ignore(false)
            .max_depth(Some(WALK_DEPTH.saturating_sub(1)));
        for entry in dot_builder.build().flatten() {
            // Exclude machine-generated bulk (e.g. .deepseek/snapshots/).
            if entry.path().starts_with(root.join(".deepseek/snapshots")) {
                continue;
            }
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(path);
            if rel.as_os_str().is_empty() {
                continue;
            }
            let display = path_to_workspace_string(rel);
            if !display.is_empty() {
                out.push(display);
            }
            if out.len() >= MAX_CANDIDATES {
                break;
            }
        }
    }

    out.sort();
    out
}

fn path_to_workspace_string(path: &Path) -> String {
    // Use forward-slash separators for cross-platform display, matching how
    // @-mentions are spelled in the composer.
    let mut out = String::new();
    for (idx, comp) in path.components().enumerate() {
        if idx > 0 {
            out.push('/');
        }
        out.push_str(&comp.as_os_str().to_string_lossy());
    }
    out
}

/// Subsequence scorer with first-letter and boundary bonuses.
///
/// Returns `None` if `query` is not a subsequence of `path` (case-insensitive),
/// otherwise a positive score where higher is better.
///
/// Heuristics (kept deliberately small and predictable):
/// * +25 for each match that lands at the start of the path or right after a
///   boundary character (`/`, `_`, `-`, `.`, ` `).
/// * +10 if the very first character of the query matches the first character
///   of the path.
/// * +5 per consecutive match (rewards contiguous runs like typing "main" and
///   matching `main.rs`).
/// * Penalty proportional to the gap between consecutive matches keeps tightly
///   matched candidates above scattered ones.
pub fn score(query: &str, path: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let p: Vec<char> = path.chars().flat_map(char::to_lowercase).collect();
    if q.len() > p.len() {
        return None;
    }

    let mut qi = 0usize;
    let mut score: i32 = 0;
    let mut last_match: Option<usize> = None;
    let mut consecutive = 0i32;

    for (i, ch) in p.iter().enumerate() {
        if qi >= q.len() {
            break;
        }
        if *ch == q[qi] {
            // Boundary / start bonus.
            if i == 0 {
                score += 25;
                if qi == 0 {
                    score += 10;
                }
            } else if matches!(p[i - 1], '/' | '_' | '-' | '.' | ' ') {
                score += 25;
            } else {
                score += 1;
            }

            // Consecutive bonus.
            if last_match == Some(i.saturating_sub(1)) {
                consecutive += 1;
                score += 5 * consecutive;
            } else {
                consecutive = 0;
            }

            // Gap penalty.
            if let Some(prev) = last_match {
                let gap = i - prev - 1;
                score -= gap as i32;
            }

            last_match = Some(i);
            qi += 1;
        }
    }

    if qi == q.len() { Some(score) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn score_subsequence_match() {
        // Identical query matches start with high bonus.
        let a = score("main", "main.rs").unwrap();
        let b = score("main", "src/very/deep/main.rs").unwrap();
        assert!(a > b, "a={} b={}", a, b);
    }

    #[test]
    fn score_rejects_non_subsequence() {
        assert!(score("zzz", "main.rs").is_none());
        assert!(score("xyz", "src/lib.rs").is_none());
    }

    #[test]
    fn score_boundary_bonus_beats_substring() {
        // "fp" matches the boundary letters in "file_picker.rs" but only the
        // first letter in "filepicker.rs" — so the boundary candidate should
        // win.
        let boundary = score("fp", "src/file_picker.rs").unwrap();
        let inline = score("fp", "src/filepicker.rs");
        // inline doesn't even contain 'p' immediately following 'f'? It does:
        // f-i-l-e-p-i-c-k-e-r — 'p' is preceded by 'e' (no boundary), so it
        // gets only the +1 path score, while boundary gets +25 for the 'p'
        // following the underscore.
        if let Some(inline_score) = inline {
            assert!(
                boundary > inline_score,
                "boundary={} inline={}",
                boundary,
                inline_score
            );
        }
    }

    #[test]
    fn score_case_insensitive() {
        assert!(score("MAIN", "main.rs").is_some());
        assert!(score("main", "MAIN.RS").is_some());
    }

    #[test]
    fn score_empty_query_returns_zero() {
        assert_eq!(score("", "anything").unwrap(), 0);
    }

    #[test]
    fn picker_typing_narrows_candidates() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();
        fs::write(root.join("Cargo.toml"), "").unwrap();

        let mut view = FilePickerView::new_with_relevance(root, FilePickerRelevance::default());
        // Empty query -> all 4 files visible.
        assert_eq!(view.visible_count(), 4, "expected all 4 candidates");

        // Typing "main" should narrow to just src/main.rs.
        for ch in "main".chars() {
            view.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        assert_eq!(view.query(), "main");
        let visible = view.visible_count();
        assert_eq!(visible, 1, "expected exactly 1 match for 'main'");
        let selected = view.selected_for_test().expect("selected path");
        assert!(selected.ends_with("main.rs"), "selected = {selected}");
    }

    #[test]
    fn picker_empty_query_prioritizes_working_set_files() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
        fs::write(root.join("README.md"), "").unwrap();

        let mut relevance = FilePickerRelevance::default();
        relevance.mark_modified("src/lib.rs");
        let view = FilePickerView::new_with_relevance(root, relevance);

        assert_eq!(view.selected_for_test(), Some("src/lib.rs"));
        assert_eq!(view.markers_for_test("src/lib.rs"), "M  ");
    }

    #[test]
    fn picker_fuzzy_query_keeps_working_set_boosts() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/alpha.rs"), "").unwrap();
        fs::write(root.join("src/zeta.rs"), "").unwrap();

        let mut relevance = FilePickerRelevance::default();
        relevance.mark_mentioned("src/zeta.rs");
        relevance.mark_tool("src/zeta.rs");
        let mut view = FilePickerView::new_with_relevance(root, relevance);
        for ch in "rs".chars() {
            view.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }

        assert_eq!(view.selected_for_test(), Some("src/zeta.rs"));
        assert_eq!(view.markers_for_test("src/zeta.rs"), " @T");
    }

    #[test]
    fn picker_backspace_widens_candidates() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("a.txt"), "").unwrap();
        fs::write(root.join("b.txt"), "").unwrap();

        let mut view = FilePickerView::new_with_relevance(root, FilePickerRelevance::default());
        view.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(view.visible_count(), 1);
        view.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(view.visible_count(), 2);
    }

    #[test]
    fn picker_enter_emits_event() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("only.txt"), "").unwrap();

        let mut view = FilePickerView::new_with_relevance(root, FilePickerRelevance::default());
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::FilePickerSelected { path }) => {
                assert!(path.ends_with("only.txt"));
            }
            other => panic!("expected EmitAndClose(FilePickerSelected), got {other:?}"),
        }
    }

    #[test]
    fn picker_esc_closes_without_emit() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("only.txt"), "").unwrap();

        let mut view = FilePickerView::new_with_relevance(root, FilePickerRelevance::default());
        let action = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn picker_honors_gitignore() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        // .gitignore filtering only kicks in inside a git repo or with an
        // explicit `.ignore` file. Use `.ignore` which `WalkBuilder` honors
        // even outside of git.
        fs::write(root.join(".ignore"), "skipme.txt\n").unwrap();
        fs::write(root.join("keepme.txt"), "").unwrap();
        fs::write(root.join("skipme.txt"), "").unwrap();

        let view = FilePickerView::new_with_relevance(root, FilePickerRelevance::default());
        let visible: Vec<_> = view
            .filtered
            .iter()
            .map(|i| view.candidates[*i].as_str())
            .collect();
        assert!(visible.iter().any(|p| p.ends_with("keepme.txt")));
        assert!(
            !visible.iter().any(|p| p.ends_with("skipme.txt")),
            "skipme.txt should be filtered by .ignore: {visible:?}"
        );
    }
}
