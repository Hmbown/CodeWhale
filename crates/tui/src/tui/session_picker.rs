//! Session resume picker view for the TUI.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::session_manager::{SavedSession, SessionManager, SessionMetadata};
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            title.to_string(),
            Style::default()
                .fg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .padding(Padding::uniform(1))
}

#[derive(Debug, Clone, Copy)]
enum SortMode {
    Recent,
    Name,
    Size,
}

pub struct SessionPickerView {
    /// Every session loaded from disk. The picker filters from this set.
    sessions: Vec<SessionMetadata>,
    filtered: Vec<SessionMetadata>,
    selected: usize,
    list_scroll: Cell<usize>,
    list_visible_rows: Cell<usize>,
    search_input: String,
    search_mode: bool,
    sort_mode: SortMode,
    preview_cache: HashMap<String, Vec<String>>,
    current_preview: Vec<String>,
    confirm_delete: bool,
    status: Option<String>,
    /// Canonical workspace path used as the per-project scope filter
    /// (#1395). `None` opts out of scoping (e.g. when the caller can't
    /// resolve a workspace).
    workspace_scope: Option<PathBuf>,
    /// When `true`, the picker shows sessions from every workspace; when
    /// `false`, only sessions whose recorded `workspace` matches the
    /// canonicalised `workspace_scope`.
    show_all_workspaces: bool,
}

impl SessionPickerView {
    /// Construct a picker scoped to `workspace`. Sessions belonging to
    /// other workspaces are hidden by default — press `a` inside the
    /// picker to expand to all workspaces (#1395).
    pub fn new(workspace: &Path) -> Self {
        let sessions = SessionManager::default_location()
            .and_then(|manager| manager.list_sessions())
            .unwrap_or_default();

        let mut view = Self {
            sessions,
            filtered: Vec::new(),
            selected: 0,
            list_scroll: Cell::new(0),
            list_visible_rows: Cell::new(8),
            search_input: String::new(),
            search_mode: false,
            sort_mode: SortMode::Recent,
            preview_cache: HashMap::new(),
            current_preview: Vec::new(),
            confirm_delete: false,
            status: None,
            workspace_scope: Some(canonical_or_self(workspace.to_path_buf())),
            show_all_workspaces: false,
        };
        view.apply_sort_and_filter();
        view.refresh_preview();
        view
    }

    fn matches_workspace_scope(&self, session: &SessionMetadata) -> bool {
        if self.show_all_workspaces {
            return true;
        }
        match self.workspace_scope.as_deref() {
            None => true,
            Some(scope) => canonical_or_self(session.workspace.clone()) == scope,
        }
    }

    /// Flip between current-workspace-only and all-workspaces view
    /// (#1395). Used by the `a` keybinding inside the picker; also
    /// callable from tests.
    pub fn toggle_all_workspaces(&mut self) {
        self.show_all_workspaces = !self.show_all_workspaces;
        let label = if self.show_all_workspaces {
            "showing sessions from every workspace"
        } else {
            "scoped to this workspace"
        };
        self.status = Some(label.to_string());
        self.selected = 0;
        self.apply_sort_and_filter();
    }

    fn apply_sort_and_filter(&mut self) {
        match self.sort_mode {
            SortMode::Recent => {
                self.sessions
                    .sort_by_key(|s| std::cmp::Reverse(s.updated_at));
            }
            SortMode::Name => {
                self.sessions.sort_by(|a, b| a.title.cmp(&b.title));
            }
            SortMode::Size => {
                self.sessions
                    .sort_by_key(|s| std::cmp::Reverse(s.message_count));
            }
        }

        let query = self.search_input.trim().to_ascii_lowercase();
        self.filtered = self
            .sessions
            .iter()
            .filter(|session| {
                self.matches_workspace_scope(session)
                    && (query.is_empty() || fuzzy_match(&query, session))
            })
            .cloned()
            .collect();

        if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
        self.ensure_selected_visible();

        self.refresh_preview();
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1) as usize;
        self.selected = next;
        self.ensure_selected_visible();
        self.refresh_preview();
    }

    fn update_list_viewport(&self, visible_rows: usize) {
        self.list_visible_rows.set(visible_rows.max(1));
        self.ensure_selected_visible();
    }

    fn ensure_selected_visible(&self) {
        if self.filtered.is_empty() {
            self.list_scroll.set(0);
            return;
        }

        let visible_rows = self.list_visible_rows.get().max(1);
        let max_scroll = self.filtered.len().saturating_sub(visible_rows);
        let mut scroll = self.list_scroll.get().min(max_scroll);

        if self.selected < scroll {
            scroll = self.selected;
        } else if self.selected >= scroll.saturating_add(visible_rows) {
            scroll = self.selected.saturating_add(1).saturating_sub(visible_rows);
        }

        self.list_scroll.set(scroll.min(max_scroll));
    }

    fn selected_session(&self) -> Option<&SessionMetadata> {
        self.filtered.get(self.selected)
    }

    fn cycle_sort(&mut self) {
        self.sort_mode = match self.sort_mode {
            SortMode::Recent => SortMode::Name,
            SortMode::Name => SortMode::Size,
            SortMode::Size => SortMode::Recent,
        };
        self.apply_sort_and_filter();
        self.status = Some(format!("Sort: {}", self.sort_label()));
    }

    fn sort_label(&self) -> &'static str {
        match self.sort_mode {
            SortMode::Recent => "recent",
            SortMode::Name => "name",
            SortMode::Size => "size",
        }
    }

    fn enter_search(&mut self) {
        self.search_mode = true;
        self.search_input.clear();
        self.status = Some("Search: type to filter, Enter to apply".to_string());
    }

    fn exit_search(&mut self) {
        self.search_mode = false;
        self.apply_sort_and_filter();
        self.status = None;
    }

    fn delete_selected(&mut self) -> Option<ViewEvent> {
        let session = self.selected_session().cloned()?;
        let manager = SessionManager::default_location().ok()?;
        if let Err(err) = manager.delete_session(&session.id) {
            self.status = Some(format!("Delete failed: {err}"));
            return None;
        }
        self.sessions.retain(|s| s.id != session.id);
        self.apply_sort_and_filter();
        self.refresh_preview();
        self.status = Some(format!(
            "Deleted session {}",
            crate::session_manager::truncate_id(&session.id)
        ));
        Some(ViewEvent::SessionDeleted {
            session_id: session.id,
            title: session.title,
        })
    }

    fn refresh_preview(&mut self) {
        let Some(session) = self.selected_session() else {
            self.current_preview = vec!["No sessions found.".to_string()];
            return;
        };

        if let Some(lines) = self.preview_cache.get(&session.id) {
            self.current_preview = lines.clone();
            return;
        }

        let manager = match SessionManager::default_location() {
            Ok(manager) => manager,
            Err(_) => {
                self.current_preview = vec!["Failed to open sessions directory.".to_string()];
                return;
            }
        };

        let saved = match manager.load_session(&session.id) {
            Ok(saved) => saved,
            Err(_) => {
                self.current_preview = vec!["Failed to load session preview.".to_string()];
                return;
            }
        };

        let preview = build_preview_lines(&saved);
        self.preview_cache
            .insert(session.id.clone(), preview.clone());
        self.current_preview = preview;
    }
}

impl ModalView for SessionPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::SessionPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if self.search_mode {
            match key.code {
                KeyCode::Enter => {
                    self.exit_search();
                }
                KeyCode::Esc => {
                    self.exit_search();
                    return ViewAction::None;
                }
                KeyCode::Backspace => {
                    self.search_input.pop();
                    self.apply_sort_and_filter();
                    return ViewAction::None;
                }
                KeyCode::Char(c) => {
                    self.search_input.push(c);
                    self.apply_sort_and_filter();
                    return ViewAction::None;
                }
                _ => {}
            }
        }

        if self.confirm_delete {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_delete = false;
                    if let Some(event) = self.delete_selected() {
                        return ViewAction::Emit(event);
                    }
                    return ViewAction::None;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_delete = false;
                    self.status = Some("Delete cancelled".to_string());
                    return ViewAction::None;
                }
                _ => return ViewAction::None,
            }
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.move_selection(-5);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.move_selection(5);
                ViewAction::None
            }
            KeyCode::Char('/') => {
                self.enter_search();
                ViewAction::None
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.cycle_sort();
                ViewAction::None
            }
            // `a`/`A` toggles the per-workspace scope filter (#1395). The
            // picker defaults to showing only sessions for the current
            // workspace so Ctrl+R never restores a different project's
            // history by surprise; press `a` to broaden to every saved
            // session.
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.toggle_all_workspaces();
                ViewAction::None
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.confirm_delete = true;
                self.status = Some("Delete session? (y/n)".to_string());
                ViewAction::None
            }
            KeyCode::Enter => {
                if let Some(session) = self.selected_session() {
                    ViewAction::EmitAndClose(ViewEvent::SessionSelected {
                        session_id: session.id.clone(),
                    })
                } else {
                    ViewAction::None
                }
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };

        Clear.render(popup_area, buf);

        let chunks = Layout::default()
            .direction(if popup_area.width < 95 {
                Direction::Vertical
            } else {
                Direction::Horizontal
            })
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(popup_area);

        let list_inner = modal_block(" Sessions ").inner(chunks[0]);
        let header_rows = 1 + usize::from(self.confirm_delete || self.status.is_some());
        let footer_rows = usize::from(!self.filtered.is_empty());
        let visible_rows = usize::from(list_inner.height)
            .saturating_sub(header_rows + footer_rows)
            .max(1);
        self.update_list_viewport(visible_rows);
        let list_scroll = self.list_scroll.get();

        let list_lines = build_list_lines(
            &self.filtered,
            self.selected,
            list_inner.width,
            list_scroll,
            visible_rows,
            self.search_mode,
            &self.search_input,
            self.sort_label(),
            self.confirm_delete,
            self.status.as_deref(),
        );
        let list = Paragraph::new(list_lines)
            .block(modal_block(" Sessions "))
            .wrap(Wrap { trim: false });
        list.render(chunks[0], buf);

        let preview_inner = modal_block(" Preview ").inner(chunks[1]);
        let preview_lines = format_preview(
            &self.current_preview,
            preview_inner.width,
            preview_inner.height as usize,
        );

        let preview = Paragraph::new(preview_lines)
            .block(modal_block(" Preview "))
            .wrap(Wrap { trim: false });
        preview.render(chunks[1], buf);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_list_lines(
    sessions: &[SessionMetadata],
    selected: usize,
    width: u16,
    scroll: usize,
    visible_rows: usize,
    search_mode: bool,
    search_input: &str,
    sort_label: &str,
    confirm_delete: bool,
    status: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header = if search_mode {
        format!("/{}", search_input)
    } else {
        format!("Sort: {sort_label} | / search | s sort | d delete")
    };
    lines.push(Line::from(Span::styled(
        truncate(&header, width),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    if confirm_delete {
        lines.push(Line::from(Span::styled(
            "Confirm delete (y/n)",
            Style::default()
                .fg(palette::STATUS_WARNING)
                .add_modifier(Modifier::BOLD),
        )));
    } else if let Some(status) = status {
        lines.push(Line::from(Span::styled(
            truncate(status, width),
            Style::default().fg(palette::DEEPSEEK_SKY),
        )));
    }

    if sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No sessions available.",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        return lines;
    }

    for (idx, session) in sessions.iter().enumerate().skip(scroll).take(visible_rows) {
        let mut line = format_session_line(session);
        line = truncate(&line, width);
        let style = if idx == selected {
            Style::default()
                .fg(palette::SELECTION_TEXT)
                .bg(palette::DEEPSEEK_BLUE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette::TEXT_PRIMARY)
        };
        lines.push(Line::from(Span::styled(line, style)));
    }

    if sessions.len() > visible_rows {
        let start = scroll.saturating_add(1);
        let end = (scroll + visible_rows).min(sessions.len());
        lines.push(Line::from(Span::styled(
            truncate(
                &format!("Showing {start}-{end} / {}", sessions.len()),
                width,
            ),
            Style::default().fg(palette::TEXT_DIM),
        )));
    }

    lines
}

fn format_session_line(session: &SessionMetadata) -> String {
    let updated = format_relative_time(&session.updated_at);
    let title = truncate(&session.title, 32);
    let mode = session
        .mode
        .as_deref()
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    format!(
        "{} | {} | {} msgs | {} | {}",
        crate::session_manager::truncate_id(&session.id),
        title,
        session.message_count,
        mode,
        updated
    )
}

fn build_preview_lines(session: &SavedSession) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("Title: {}", session.metadata.title));
    out.push(format!(
        "Updated: {}",
        session
            .metadata
            .updated_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
    ));
    out.push(format!(
        "Messages: {} | Model: {}",
        session.metadata.message_count, session.metadata.model
    ));
    if let Some(mode) = session.metadata.mode.as_deref() {
        out.push(format!("Mode: {}", mode));
    }
    out.push("".to_string());

    for message in session.messages.iter().take(6) {
        let role = message.role.to_ascii_uppercase();
        let mut text = String::new();
        for block in &message.content {
            if let crate::models::ContentBlock::Text { text: body, .. } = block {
                text.push_str(body);
            }
        }
        let preview = truncate(&text.replace('\n', " "), 120);
        out.push(format!("{role}: {preview}"));
    }
    out
}

fn format_preview(lines: &[String], width: u16, height: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let available = height.saturating_sub(2).max(1);
    for line in lines.iter().take(available) {
        out.push(Line::from(Span::styled(
            truncate(line, width),
            Style::default().fg(palette::TEXT_PRIMARY),
        )));
    }
    out
}

fn format_relative_time(dt: &DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(*dt);
    if duration.num_minutes() < 1 {
        "just now".to_string()
    } else if duration.num_hours() < 1 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_days() < 1 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}d ago", duration.num_days())
    }
}

fn truncate(text: &str, width: u16) -> String {
    let max = width.max(1) as usize;
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut current = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if current + w >= max.saturating_sub(3) {
            break;
        }
        out.push(ch);
        current += w;
    }
    out.push_str("...");
    out
}

/// Best-effort canonicalisation of a path so two recordings of the same
/// workspace match even when one is symlinked or relative. Falls back to
/// the input path when canonicalisation fails (e.g. for a deleted dir or
/// during tests with tmp paths that have already been cleaned up).
fn canonical_or_self(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn fuzzy_match(query: &str, session: &SessionMetadata) -> bool {
    let haystack = format!(
        "{} {} {}",
        session.title,
        session.id,
        session.workspace.display()
    )
    .to_ascii_lowercase();
    if haystack.contains(query) {
        return true;
    }
    is_subsequence(query, &haystack)
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = needle.chars();
    let mut current = match chars.next() {
        Some(c) => c,
        None => return true,
    };
    for ch in haystack.chars() {
        if ch == current {
            if let Some(next) = chars.next() {
                current = next;
            } else {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use unicode_width::UnicodeWidthStr;

    fn test_session(idx: usize, title: &str) -> SessionMetadata {
        SessionMetadata {
            id: format!("session-{idx:02}"),
            title: title.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            message_count: idx + 1,
            total_tokens: 100,
            model: "deepseek-v4-pro".to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            mode: Some("agent".to_string()),
            cost: crate::session_manager::SessionCostSnapshot::default(),
        }
    }

    fn test_session_in(idx: usize, title: &str, workspace: &str) -> SessionMetadata {
        let mut s = test_session(idx, title);
        s.workspace = std::path::PathBuf::from(workspace);
        s
    }

    fn picker_with(sessions: Vec<SessionMetadata>, scope: Option<&str>) -> SessionPickerView {
        let workspace_scope = scope.map(PathBuf::from);
        let mut view = SessionPickerView {
            sessions: sessions.clone(),
            filtered: sessions,
            selected: 0,
            list_scroll: Cell::new(0),
            list_visible_rows: Cell::new(8),
            search_input: String::new(),
            search_mode: false,
            sort_mode: SortMode::Recent,
            preview_cache: HashMap::new(),
            current_preview: Vec::new(),
            confirm_delete: false,
            status: None,
            workspace_scope,
            show_all_workspaces: false,
        };
        view.apply_sort_and_filter();
        view
    }

    #[test]
    fn workspace_scope_filters_sessions_to_current_project() {
        // #1395 reproduction: Ctrl+R in project B must not surface sessions
        // from project A.
        let sessions = vec![
            test_session_in(1, "project-a chat", "/tmp/project-a"),
            test_session_in(2, "project-b chat", "/tmp/project-b"),
            test_session_in(3, "another project-a chat", "/tmp/project-a"),
        ];
        let view = picker_with(sessions, Some("/tmp/project-b"));
        assert_eq!(view.filtered.len(), 1, "only project-b session should show");
        assert_eq!(view.filtered[0].title, "project-b chat");
    }

    #[test]
    fn workspace_scope_toggle_a_expands_to_all_workspaces() {
        let sessions = vec![
            test_session_in(1, "a", "/tmp/project-a"),
            test_session_in(2, "b", "/tmp/project-b"),
            test_session_in(3, "c", "/tmp/project-c"),
        ];
        let mut view = picker_with(sessions, Some("/tmp/project-b"));
        assert_eq!(view.filtered.len(), 1);

        view.toggle_all_workspaces();
        assert_eq!(view.filtered.len(), 3, "after toggle, every session shows");
        assert!(view.show_all_workspaces);
        assert!(
            view.status
                .as_deref()
                .map(|s| s.contains("every workspace"))
                .unwrap_or(false),
            "status should announce the new mode, got {:?}",
            view.status
        );

        view.toggle_all_workspaces();
        assert_eq!(view.filtered.len(), 1, "toggling back restores the scope");
    }

    #[test]
    fn workspace_scope_none_means_show_all() {
        // An unscoped picker (no workspace) lists everything — matches the
        // pre-#1395 behaviour for any caller that opts out.
        let sessions = vec![
            test_session_in(1, "a", "/tmp/project-a"),
            test_session_in(2, "b", "/tmp/project-b"),
        ];
        let view = picker_with(sessions, None);
        assert_eq!(view.filtered.len(), 2);
    }

    #[test]
    fn build_list_lines_truncates_to_list_pane_width() {
        let sessions = vec![test_session(
            1,
            "A very long title that should be truncated by the list pane width",
        )];
        let width = 24;
        let lines = build_list_lines(&sessions, 0, width, 0, 5, false, "", "recent", false, None);

        for line in lines {
            let rendered_width: usize = line.spans.iter().map(|span| span.content.width()).sum();
            assert!(
                rendered_width <= width as usize,
                "line width {} exceeded pane width {}",
                rendered_width,
                width
            );
        }
    }

    #[test]
    fn build_list_lines_selected_row_uses_strong_highlight() {
        let sessions = vec![
            test_session(1, "first session"),
            test_session(2, "second session"),
        ];
        let lines = build_list_lines(&sessions, 1, 80, 0, 5, false, "", "recent", false, None);

        let selected_line = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains("second session"))
            })
            .expect("selected session should render");
        let span = selected_line
            .spans
            .first()
            .expect("selected row should have a span");

        assert_eq!(span.style.fg, Some(palette::SELECTION_TEXT));
        assert_eq!(span.style.bg, Some(palette::DEEPSEEK_BLUE));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn ensure_selected_visible_updates_scroll_window() {
        let sessions = (0..10)
            .map(|idx| test_session(idx, &format!("Session {idx}")))
            .collect::<Vec<_>>();

        let mut view = SessionPickerView {
            sessions: sessions.clone(),
            filtered: sessions,
            selected: 0,
            list_scroll: Cell::new(0),
            list_visible_rows: Cell::new(3),
            search_input: String::new(),
            search_mode: false,
            sort_mode: SortMode::Recent,
            preview_cache: HashMap::new(),
            current_preview: Vec::new(),
            confirm_delete: false,
            status: None,
            workspace_scope: None,
            show_all_workspaces: true,
        };

        view.selected = 6;
        view.ensure_selected_visible();
        assert_eq!(view.list_scroll.get(), 4);

        view.selected = 1;
        view.ensure_selected_visible();
        assert_eq!(view.list_scroll.get(), 1);

        view.selected = 9;
        view.ensure_selected_visible();
        assert_eq!(view.list_scroll.get(), 7);
    }
}
