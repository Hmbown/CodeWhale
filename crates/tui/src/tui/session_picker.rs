//! Session resume picker view for the TUI.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::session_manager::{
    SavedSession, SessionManager, SessionMetadata, extract_title, extract_user_prompt,
    strip_thinking_tags,
};
use crate::tui::hit_region::HitMap;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

fn modal_block(title: &str) -> Block<'static> {
    Block::default()
        .title(Line::from(vec![Span::styled(
            title.to_string(),
            Style::default()
                .fg(palette::WHALE_ACCENT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )]))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .style(Style::default().bg(palette::WHALE_BG))
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
    history_scroll: Cell<usize>,
    history_pinned_to_latest: Cell<bool>,
    history_visible_rows: Cell<usize>,
    search_input: String,
    search_mode: bool,
    sort_mode: SortMode,
    preview_cache: HashMap<String, Vec<String>>,
    current_preview: Vec<String>,
    confirm_delete: bool,
    rename_mode: bool,
    rename_input: String,
    status: Option<String>,
    /// Session-list row hit regions from the last paint.
    list_hit_map: RefCell<HitMap>,
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
            history_scroll: Cell::new(0),
            history_pinned_to_latest: Cell::new(true),
            history_visible_rows: Cell::new(12),
            search_input: String::new(),
            search_mode: false,
            sort_mode: SortMode::Recent,
            preview_cache: HashMap::new(),
            current_preview: Vec::new(),
            confirm_delete: false,
            rename_mode: false,
            rename_input: String::new(),
            status: None,
            list_hit_map: RefCell::new(HitMap::new()),
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

    fn select_visible_shortcut(&mut self, c: char) -> bool {
        let Some(slot) = c.to_digit(10) else {
            return false;
        };
        if !(1..=9).contains(&slot) {
            return false;
        }
        let index = self.list_scroll.get().saturating_add(slot as usize - 1);
        if index >= self.filtered.len() {
            return false;
        }
        self.selected = index;
        self.ensure_selected_visible();
        self.refresh_preview();
        if let Some(session) = self.selected_session() {
            self.status = Some(format!(
                "Opened history for {}",
                crate::session_manager::truncate_id(&session.id)
            ));
        }
        true
    }

    fn update_list_viewport(&self, visible_rows: usize) {
        self.list_visible_rows.set(visible_rows.max(1));
        self.ensure_selected_visible();
    }

    fn update_history_viewport(&self, visible_rows: usize) {
        self.history_visible_rows.set(visible_rows.max(1));
        self.ensure_history_scroll_in_bounds();
    }

    fn scroll_history(&self, delta: isize) {
        let max_scroll =
            max_history_scroll_for(&self.current_preview, self.history_visible_rows.get());
        let current = self.history_scroll.get();
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        let next = next.min(max_scroll);
        self.history_scroll.set(next);
        self.history_pinned_to_latest.set(next == max_scroll);
    }

    fn ensure_history_scroll_in_bounds(&self) {
        let max_scroll =
            max_history_scroll_for(&self.current_preview, self.history_visible_rows.get());
        if self.history_pinned_to_latest.get() {
            self.history_scroll.set(max_scroll);
        } else {
            self.history_scroll
                .set(self.history_scroll.get().min(max_scroll));
        }
    }

    fn scroll_history_to_latest(&self) {
        let max_scroll =
            max_history_scroll_for(&self.current_preview, self.history_visible_rows.get());
        self.history_scroll.set(max_scroll);
        self.history_pinned_to_latest.set(true);
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

    fn focus_session_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.list_hit_map.borrow().row_at(column, row) else {
            return false;
        };
        if index >= self.filtered.len() {
            return false;
        }
        if self.selected != index {
            self.selected = index;
            self.ensure_selected_visible();
            self.refresh_preview();
        }
        true
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

    fn rename_selected(&mut self, new_title: &str) -> ViewAction {
        let Some(session) = self.selected_session().cloned() else {
            self.status = Some("No session selected".to_string());
            return ViewAction::None;
        };
        if new_title.is_empty() || new_title.len() > 100 {
            self.status = Some("Title must be 1–100 characters".to_string());
            return ViewAction::None;
        }
        let manager = match SessionManager::default_location() {
            Ok(m) => m,
            Err(e) => {
                self.status = Some(format!("Could not open sessions: {e}"));
                return ViewAction::None;
            }
        };
        let mut saved = match manager.load_session(&session.id) {
            Ok(s) => s,
            Err(e) => {
                self.status = Some(format!("Could not load session: {e}"));
                return ViewAction::None;
            }
        };
        saved.metadata.title = new_title.to_string();
        if let Err(e) = manager.save_session(&saved) {
            self.status = Some(format!("Rename failed: {e}"));
            return ViewAction::None;
        }
        // Update our local metadata cache.
        if let Some(meta) = self.sessions.iter_mut().find(|s| s.id == session.id) {
            meta.title = new_title.to_string();
        }
        self.apply_sort_and_filter();
        self.refresh_preview();
        self.status = Some(format!("Renamed to \"{new_title}\""));
        ViewAction::None
    }

    fn refresh_preview(&mut self) {
        let Some(session) = self.selected_session() else {
            self.current_preview = vec!["No sessions found.".to_string()];
            self.scroll_history_to_latest();
            return;
        };

        if let Some(lines) = self.preview_cache.get(&session.id) {
            self.current_preview = lines.clone();
            self.scroll_history_to_latest();
            return;
        }

        let manager = match SessionManager::default_location() {
            Ok(manager) => manager,
            Err(_) => {
                self.current_preview = vec!["Failed to open sessions directory.".to_string()];
                self.scroll_history_to_latest();
                return;
            }
        };

        let saved = match manager.load_session(&session.id) {
            Ok(saved) => saved,
            Err(_) => {
                self.current_preview = vec!["Failed to load session preview.".to_string()];
                self.scroll_history_to_latest();
                return;
            }
        };

        let preview = build_preview_lines(&saved);
        self.preview_cache
            .insert(session.id.clone(), preview.clone());
        self.current_preview = preview;
        self.scroll_history_to_latest();
    }
}

impl ModalView for SessionPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::SessionPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left)
                if !self.search_mode && !self.rename_mode && !self.confirm_delete =>
            {
                let _ = self.focus_session_at(mouse.column, mouse.row);
            }
            MouseEventKind::Down(MouseButton::Left)
                if !self.search_mode && !self.rename_mode && !self.confirm_delete =>
            {
                if self.focus_session_at(mouse.column, mouse.row)
                    && let Some(session) = self.selected_session()
                {
                    return ViewAction::EmitAndClose(ViewEvent::SessionSelected {
                        session_id: session.id.clone(),
                    });
                }
            }
            _ => {}
        }
        ViewAction::None
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

        if self.rename_mode {
            match key.code {
                KeyCode::Enter => {
                    self.rename_mode = false;
                    let new_title = self.rename_input.trim().to_string();
                    self.rename_input.clear();
                    return self.rename_selected(&new_title);
                }
                KeyCode::Esc => {
                    self.rename_mode = false;
                    self.rename_input.clear();
                    self.status = Some("Rename cancelled".to_string());
                    return ViewAction::None;
                }
                KeyCode::Backspace => {
                    self.rename_input.pop();
                    return ViewAction::None;
                }
                KeyCode::Char(c) if !c.is_control() => {
                    self.rename_input.push(c);
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
                let rows = self.history_visible_rows.get().max(1);
                self.scroll_history(-(rows as isize));
                ViewAction::None
            }
            KeyCode::PageDown => {
                let rows = self.history_visible_rows.get().max(1);
                self.scroll_history(rows as isize);
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
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.rename_mode = true;
                self.rename_input.clear();
                self.status = Some("New title: ".to_string());
                ViewAction::None
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.confirm_delete = true;
                self.status = Some("Delete session? (y/n)".to_string());
                ViewAction::None
            }
            KeyCode::Char(c) if self.select_visible_shortcut(c) => ViewAction::None,
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

        let narrow = popup_area.width < 95;
        let chunks = Layout::default()
            .direction(if narrow {
                Direction::Vertical
            } else {
                Direction::Horizontal
            })
            .constraints(if narrow {
                [Constraint::Percentage(42), Constraint::Percentage(58)]
            } else {
                [Constraint::Percentage(64), Constraint::Percentage(36)]
            })
            .split(popup_area);
        let (history_area, list_area) = if narrow {
            (chunks[1], chunks[0])
        } else {
            (chunks[0], chunks[1])
        };

        let list_inner = modal_block(" Sessions (1-9) ").inner(list_area);
        let header_rows = 1 + usize::from(self.confirm_delete || self.status.is_some());
        let footer_rows = usize::from(!self.filtered.is_empty());
        let visible_rows = usize::from(list_inner.height)
            .saturating_sub(header_rows + footer_rows)
            .max(1);
        self.update_list_viewport(visible_rows);
        let list_scroll = self.list_scroll.get();
        let mut list_hit_map = HitMap::new();
        for index in list_scroll..(list_scroll + visible_rows).min(self.filtered.len()) {
            let line_y = list_inner
                .y
                .saturating_add(u16::try_from(header_rows + index - list_scroll).unwrap_or(0));
            if line_y < list_inner.y.saturating_add(list_inner.height) {
                list_hit_map.push_row(index, list_inner.x, line_y, list_inner.width);
            }
        }
        *self.list_hit_map.borrow_mut() = list_hit_map;

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
            self.rename_mode,
            &self.rename_input,
            self.status.as_deref(),
        );
        let list = Paragraph::new(list_lines)
            .block(modal_block(" Sessions (1-9) "))
            .wrap(Wrap { trim: false });
        list.render(list_area, buf);

        let history_inner = modal_block(" History (PgUp/PgDn) ").inner(history_area);
        self.update_history_viewport(history_inner.height as usize);
        let visible_preview = visible_preview_lines(
            &self.current_preview,
            self.history_scroll.get(),
            history_inner.height as usize,
        );
        let preview_lines = format_preview(&visible_preview);

        let preview = Paragraph::new(preview_lines)
            .block(modal_block(" History (PgUp/PgDn) "))
            .wrap(Wrap { trim: false });
        preview.render(history_area, buf);
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
    rename_mode: bool,
    rename_input: &str,
    status: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header = if search_mode {
        format!("/{search_input}")
    } else if rename_mode {
        format!("New title: {rename_input}_")
    } else {
        format!(
            "1-9 history | PgUp/PgDn scroll | Enter resume | / search | s sort | r rename | a all | d delete | Sort: {sort_label}"
        )
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
            Style::default().fg(palette::WHALE_INFO),
        )));
    }

    if sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No saved sessions yet.",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(Span::styled(
            "Send a message to start one; it is saved automatically.",
            Style::default().fg(palette::TEXT_HINT),
        )));
        return lines;
    }

    for (idx, session) in sessions.iter().enumerate().skip(scroll).take(visible_rows) {
        let slot = idx.saturating_sub(scroll).saturating_add(1);
        let prefix = if slot <= 9 {
            format!("{slot}. ")
        } else {
            "   ".to_string()
        };
        let mut line = format!("{prefix}{}", format_session_line(session));
        line = truncate(&line, width);
        let style = if idx == selected {
            Style::default()
                .fg(palette::SELECTION_TEXT)
                .bg(palette::SELECTION_BG)
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
    let age = format_relative_time(&session.updated_at);
    let updated = crate::session_manager::format_session_updated_at(&session.updated_at, &age);
    let raw_title = extract_title(&session.title);
    let title = if raw_title == "Session" {
        truncate(crate::session_manager::truncate_id(&session.id), 32)
    } else {
        truncate(raw_title, 32)
    };
    let mode = session
        .mode
        .as_deref()
        .unwrap_or("unknown")
        .to_ascii_lowercase();
    let fork_label = if session.parent_session_id.is_some() {
        " | fork"
    } else {
        ""
    };
    format!(
        "{} | {} | {} msgs{} | {} | {}",
        crate::session_manager::truncate_id(&session.id),
        title,
        session.message_count,
        fork_label,
        mode,
        updated
    )
}

fn build_preview_lines(session: &SavedSession) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("Title: {}", extract_title(&session.metadata.title)));
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
        out.push(format!("Mode: {mode}"));
    }
    out.push("".to_string());

    for message in &session.messages {
        let text = message_text_for_history(message);
        if text.trim().is_empty() {
            continue;
        }
        out.push(format!("{}:", message.role.to_ascii_uppercase()));
        for line in text.lines() {
            out.push(format!("  {line}"));
        }
        out.push(String::new());
    }
    if out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

fn message_text_for_history(message: &crate::models::Message) -> String {
    let mut text = String::new();
    for block in &message.content {
        let part = match block {
            crate::models::ContentBlock::Text { text: body, .. } => {
                if message.role.eq_ignore_ascii_case("user") {
                    extract_user_prompt(body).to_string()
                } else {
                    strip_thinking_tags(body)
                }
            }
            crate::models::ContentBlock::Thinking { .. } => String::new(),
            crate::models::ContentBlock::ToolUse { name, input, .. } => {
                format!("tool call: {name} {}", truncate(&input.to_string(), 180))
            }
            crate::models::ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                let label = if is_error.unwrap_or(false) {
                    "tool error"
                } else {
                    "tool result"
                };
                format!("{label}: {}", truncate(&content.replace('\n', " "), 220))
            }
            crate::models::ContentBlock::ServerToolUse { name, input, .. } => {
                format!("server tool: {name} {}", truncate(&input.to_string(), 180))
            }
            crate::models::ContentBlock::ToolSearchToolResult { content, .. }
            | crate::models::ContentBlock::CodeExecutionToolResult { content, .. } => {
                format!("tool result: {}", truncate(&content.to_string(), 220))
            }
            crate::models::ContentBlock::ImageUrl { .. } => String::from("[image]"),
        };
        let part = part.trim();
        if !part.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(part);
        }
    }
    text
}

fn format_preview(lines: &[String]) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for line in lines {
        out.push(Line::from(Span::styled(
            line.clone(),
            Style::default().fg(palette::TEXT_PRIMARY),
        )));
    }
    out
}

fn preview_body_start(lines: &[String], visible_rows: usize) -> Option<usize> {
    let visible_rows = visible_rows.max(1);
    let body_start = lines
        .iter()
        .position(|line| line.is_empty())
        .map(|idx| idx + 1)?;
    (body_start < visible_rows).then_some(body_start)
}

fn max_history_scroll_for(lines: &[String], visible_rows: usize) -> usize {
    let visible_rows = visible_rows.max(1);
    let Some(body_start) = preview_body_start(lines, visible_rows) else {
        return lines.len().saturating_sub(visible_rows);
    };
    let body_visible_rows = visible_rows.saturating_sub(body_start).max(1);
    lines
        .len()
        .saturating_sub(body_start)
        .saturating_sub(body_visible_rows)
}

fn visible_preview_lines(lines: &[String], scroll: usize, visible_rows: usize) -> Vec<String> {
    let visible_rows = visible_rows.max(1);
    let max_scroll = max_history_scroll_for(lines, visible_rows);
    let scroll = scroll.min(max_scroll);
    let Some(body_start) = preview_body_start(lines, visible_rows) else {
        return lines
            .iter()
            .skip(scroll)
            .take(visible_rows)
            .cloned()
            .collect();
    };

    let body_visible_rows = visible_rows.saturating_sub(body_start).max(1);
    let mut out = Vec::with_capacity(visible_rows);
    out.extend(lines.iter().take(body_start).cloned());
    out.extend(
        lines
            .iter()
            .skip(body_start + scroll)
            .take(body_visible_rows)
            .cloned(),
    );
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

    use crate::tui::hit_region::HitTarget;

    fn test_session(idx: usize, title: &str) -> SessionMetadata {
        SessionMetadata {
            id: format!("session-{idx:02}"),
            title: title.to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            message_count: idx + 1,
            total_tokens: 100,
            model: "deepseek-v4-pro".to_string(),
            model_provider: "deepseek".to_string(),
            workspace: std::path::PathBuf::from("/tmp"),
            mode: Some("agent".to_string()),
            cost: crate::session_manager::SessionCostSnapshot::default(),
            parent_session_id: None,
            forked_from_message_count: None,
            cumulative_turn_secs: 0,
        }
    }

    fn test_session_in(idx: usize, title: &str, workspace: &str) -> SessionMetadata {
        let mut s = test_session(idx, title);
        s.workspace = std::path::PathBuf::from(workspace);
        s
    }

    fn text_message(role: &str, text: &str) -> crate::models::Message {
        crate::models::Message {
            role: role.to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn saved_session_with_messages(messages: Vec<crate::models::Message>) -> SavedSession {
        let mut session = crate::session_manager::create_saved_session(
            &messages,
            "deepseek-v4-pro",
            std::path::Path::new("/tmp"),
            100,
            None,
        );
        session.metadata.title = "<turn_meta>{}</turn_meta>\nClean session title".to_string();
        session
    }

    fn picker_with(sessions: Vec<SessionMetadata>, scope: Option<&str>) -> SessionPickerView {
        let workspace_scope = scope.map(PathBuf::from);
        let mut view = SessionPickerView {
            sessions: sessions.clone(),
            filtered: sessions,
            selected: 0,
            list_scroll: Cell::new(0),
            list_visible_rows: Cell::new(8),
            history_scroll: Cell::new(0),
            history_pinned_to_latest: Cell::new(true),
            history_visible_rows: Cell::new(12),
            search_input: String::new(),
            search_mode: false,
            sort_mode: SortMode::Recent,
            preview_cache: HashMap::new(),
            current_preview: Vec::new(),
            confirm_delete: false,
            rename_mode: false,
            rename_input: String::new(),
            status: None,
            list_hit_map: RefCell::new(HitMap::new()),
            workspace_scope,
            show_all_workspaces: false,
        };
        view.apply_sort_and_filter();
        view
    }

    fn buffer_row_text(buf: &Buffer, area: Rect, y: u16) -> String {
        (area.x..area.x.saturating_add(area.width))
            .map(|x| buf[(x, y)].symbol())
            .collect()
    }

    fn row_containing(buf: &Buffer, area: Rect, needle: &str) -> Option<u16> {
        (area.y..area.y.saturating_add(area.height))
            .find(|&y| buffer_row_text(buf, area, y).contains(needle))
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
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
        let lines = build_list_lines(
            &sessions, 0, width, 0, 5, false, "", "recent", false, false, "", None,
        );

        for line in lines {
            let rendered_width: usize = line.spans.iter().map(|span| span.content.width()).sum();
            assert!(
                rendered_width <= width as usize,
                "line width {rendered_width} exceeded pane width {width}"
            );
        }
    }

    #[test]
    fn build_list_lines_selected_row_uses_muted_selection_highlight() {
        let sessions = vec![
            test_session(1, "first session"),
            test_session(2, "second session"),
        ];
        let lines = build_list_lines(
            &sessions, 1, 80, 0, 5, false, "", "recent", false, false, "", None,
        );

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
        assert_eq!(span.style.bg, Some(palette::SELECTION_BG));
        assert_ne!(span.style.bg, Some(palette::WHALE_ACCENT_PRIMARY));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn session_picker_selected_row_renders_readable_selection_contrast() {
        let mut first = test_session(1, "first contrast fixture");
        first.id = "alpha-contrast-fixture".to_string();
        let mut second = test_session(2, "second contrast fixture");
        second.id = "bravo-contrast-fixture".to_string();
        let sessions = vec![first, second];
        let mut view = picker_with(sessions, None);
        view.selected = 1;
        view.ensure_selected_visible();
        view.current_preview = vec!["preview".to_string()];
        let selected_id = crate::session_manager::truncate_id(&view.filtered[view.selected].id);
        let area = Rect::new(0, 0, 120, 28);
        let mut buf = Buffer::empty(area);

        view.render(area, &mut buf);

        let y =
            row_containing(&buf, area, selected_id).expect("selected session row should render");
        let rendered_row = buffer_row_text(&buf, area, y);
        let highlighted_cells = (area.x..area.x.saturating_add(area.width))
            .filter(|&x| {
                let cell = &buf[(x, y)];
                !cell.symbol().trim().is_empty()
                    && cell.bg == palette::SELECTION_BG
                    && cell.fg == palette::SELECTION_TEXT
            })
            .count();

        assert!(
            highlighted_cells >= 4,
            "selected /sessions row should use readable selection text; got {highlighted_cells} highlighted cells on {rendered_row:?}"
        );
        assert!(
            !(area.x..area.x.saturating_add(area.width))
                .any(|x| buf[(x, y)].bg == palette::WHALE_ACCENT_PRIMARY),
            "selected /sessions row should not use the bright accent background"
        );
    }

    #[test]
    fn mouse_hover_focuses_and_click_resumes_session_row() {
        let mut first = test_session(1, "first mouse fixture");
        first.id = "alpha-mouse-fixture".to_string();
        let mut second = test_session(2, "second mouse fixture");
        second.id = "bravo-mouse-fixture".to_string();
        let mut view = picker_with(vec![first, second], None);
        view.current_preview = vec!["preview".to_string()];

        let area = Rect::new(0, 0, 120, 28);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let hit = view
            .list_hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|region| matches!(region.target, HitTarget::Row { index } if index > 0))
            .copied()
            .expect("visible non-first session row hit region");
        let HitTarget::Row { index } = hit.target;
        let expected_session_id = view.filtered[index].id.clone();

        let hover = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(matches!(hover, ViewAction::None));
        assert_eq!(view.selected, index);

        view.selected = 0;
        let click = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        match click {
            ViewAction::EmitAndClose(ViewEvent::SessionSelected { session_id }) => {
                assert_eq!(session_id, expected_session_id);
            }
            other => panic!("expected selected session emit, got {other:?}"),
        }
        assert_eq!(view.selected, index);
    }

    #[test]
    fn session_picker_visual_matrix_covers_narrow_and_medium_rendering() {
        let base_time = DateTime::parse_from_rfc3339("2026-06-25T10:30:00Z")
            .expect("visual matrix timestamp")
            .with_timezone(&Utc);
        let sessions = (0..12)
            .map(|idx| {
                let title = if idx == 6 {
                    "selected visual matrix target with 中文内容 and suffix that must truncate"
                } else {
                    "A very long terminal visual regression session title with 中文内容 and suffix that must truncate"
                };
                let mut session = test_session(idx, title);
                session.id = format!("visual-matrix-{idx:02}");
                session.created_at = base_time - chrono::Duration::seconds(idx as i64);
                session.updated_at = session.created_at;
                session
            })
            .collect::<Vec<_>>();
        let mut view = picker_with(sessions, None);
        view.selected = view
            .filtered
            .iter()
            .position(|session| session.id == "visual-matrix-06")
            .expect("visual matrix target session should be filtered");
        view.ensure_selected_visible();
        view.current_preview = vec![
            "Title: terminal visual matrix".to_string(),
            "Updated: 2026-06-25 10:30".to_string(),
            "Messages: 3 | Model: deepseek-v4-pro".to_string(),
            String::new(),
            "USER: narrow panes should keep long CJK text readable 中文中文中文".to_string(),
            "ASSISTANT: overlays should keep borders and truncate rows predictably".to_string(),
        ];

        for (width, height, label) in [(72, 20, "narrow"), (120, 28, "medium")] {
            let area = Rect::new(0, 0, width, height);
            let mut buf = Buffer::empty(area);

            view.render(area, &mut buf);

            let dump = buffer_text(&buf, area);
            assert!(
                dump.contains("Sessions"),
                "{label} sessions pane missing:\n{dump}"
            );
            assert!(
                dump.contains("History"),
                "{label} history pane missing:\n{dump}"
            );
            assert!(dump.contains('┌'), "{label} top border missing:\n{dump}");
            assert!(dump.contains('┘'), "{label} bottom border missing:\n{dump}");
            assert!(
                !dump.contains("suffix that must truncate"),
                "{label} long title tail leaked instead of truncating:\n{dump}"
            );
            assert!(
                dump.contains("..."),
                "{label} should show an explicit ellipsis for truncated rows:\n{dump}"
            );
            assert!(
                !dump.contains('\u{fffd}'),
                "{label} render emitted replacement characters:\n{dump}"
            );

            assert!(
                row_containing(&buf, area, "selected visual").is_some(),
                "{label} selected session row missing:\n{dump}"
            );
        }
    }

    #[test]
    fn build_list_lines_includes_absolute_updated_timestamp() {
        let mut session = test_session(1, "last friday thread");
        session.updated_at = DateTime::parse_from_rfc3339("2026-06-01T12:34:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let lines = build_list_lines(
            &[session],
            0,
            120,
            0,
            5,
            false,
            "",
            "recent",
            false,
            false,
            "",
            None,
        );

        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("2026-06-01 12:34 UTC"),
            "session picker should include an absolute timestamp, got {rendered:?}"
        );
    }

    #[test]
    fn build_list_lines_marks_fork_lineage() {
        let mut forked = test_session(1, "forked path");
        forked.parent_session_id = Some("parent-session-abcdef".to_string());
        forked.forked_from_message_count = Some(3);
        let lines = build_list_lines(
            &[forked],
            0,
            120,
            0,
            5,
            false,
            "",
            "recent",
            false,
            false,
            "",
            None,
        );

        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("fork"));
        assert!(!rendered.contains("parent-session-abcdef"));
    }

    #[test]
    fn build_list_lines_numbers_visible_rows_for_shortcuts() {
        let sessions = vec![
            test_session(1, "first session"),
            test_session(2, "second session"),
        ];
        let lines = build_list_lines(
            &sessions, 0, 80, 0, 5, false, "", "recent", false, false, "", None,
        );

        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("1. session-"));
        assert!(rendered.contains("2. session-"));
    }

    #[test]
    fn digit_shortcut_selects_visible_session_for_history() {
        let sessions = vec![
            test_session(1, "first session"),
            test_session(2, "second session"),
            test_session(3, "third session"),
        ];
        let mut view = picker_with(sessions, None);

        assert!(view.select_visible_shortcut('2'));
        assert_eq!(view.selected, 1);
        assert!(
            view.status
                .as_deref()
                .is_some_and(|status| status.contains("Opened history"))
        );
        assert!(!view.select_visible_shortcut('9'));
    }

    #[test]
    fn history_scroll_pages_and_clamps() {
        let mut view = picker_with(vec![test_session(1, "first")], None);
        view.current_preview = (0..20).map(|idx| format!("line {idx}")).collect();
        view.history_visible_rows.set(5);

        view.scroll_history(6);
        assert_eq!(view.history_scroll.get(), 6);
        view.scroll_history(100);
        assert_eq!(view.history_scroll.get(), 15);
        view.scroll_history(-200);
        assert_eq!(view.history_scroll.get(), 0);
    }

    #[test]
    fn history_preview_keeps_header_while_scrolling_transcript() {
        let lines = vec![
            "Title: version".to_string(),
            "Updated: 2026-05-14 01:02".to_string(),
            "Messages: 100 | Model: auto".to_string(),
            "Mode: agent".to_string(),
            String::new(),
            "USER: oldest prompt".to_string(),
            "ASSISTANT: oldest answer".to_string(),
            "USER: middle prompt".to_string(),
            "ASSISTANT: middle answer".to_string(),
            "USER: newest prompt".to_string(),
            "ASSISTANT: newest answer".to_string(),
        ];

        let max_scroll = max_history_scroll_for(&lines, 8);
        assert_eq!(max_scroll, 3);

        let rendered = visible_preview_lines(&lines, max_scroll, 8).join("\n");
        assert!(rendered.contains("Title: version"));
        assert!(rendered.contains("Updated: 2026-05-14 01:02"));
        assert!(!rendered.contains("oldest prompt"));
        assert!(rendered.contains("newest prompt"));
        assert!(rendered.contains("newest answer"));
    }

    #[test]
    fn history_refresh_starts_at_latest_transcript_messages() {
        let mut view = picker_with(vec![test_session(1, "first")], None);
        view.current_preview = vec![
            "Title: first".to_string(),
            "Updated: 2026-05-14 01:02".to_string(),
            "Messages: 10 | Model: auto".to_string(),
            String::new(),
            "line 0".to_string(),
            "line 1".to_string(),
            "line 2".to_string(),
            "line 3".to_string(),
            "line 4".to_string(),
            "line 5".to_string(),
        ];
        view.history_visible_rows.set(6);

        view.scroll_history_to_latest();

        assert_eq!(view.history_scroll.get(), 4);
        assert!(view.history_pinned_to_latest.get());
    }

    #[test]
    fn build_preview_lines_shows_full_clean_history() {
        let messages = vec![
            text_message(
                "user",
                "<turn_meta>{\"cache\":\"x\"}</turn_meta>\nFirst visible prompt",
            ),
            text_message(
                "assistant",
                "<thinking>hidden reasoning</thinking>\nFirst visible answer",
            ),
            text_message("user", "Second prompt"),
            text_message("assistant", "Second answer"),
            text_message("user", "Third prompt"),
            text_message("assistant", "Third answer"),
            text_message("user", "Fourth prompt beyond old six-message preview"),
        ];
        let session = saved_session_with_messages(messages);
        let lines = build_preview_lines(&session).join("\n");

        assert!(lines.contains("Title: Clean session title"));
        assert!(lines.contains("First visible prompt"));
        assert!(lines.contains("First visible answer"));
        assert!(lines.contains("Fourth prompt beyond old six-message preview"));
        assert!(!lines.contains("turn_meta"));
        assert!(!lines.contains("hidden reasoning"));
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
            history_scroll: Cell::new(0),
            history_pinned_to_latest: Cell::new(true),
            history_visible_rows: Cell::new(12),
            search_input: String::new(),
            search_mode: false,
            sort_mode: SortMode::Recent,
            preview_cache: HashMap::new(),
            current_preview: Vec::new(),
            confirm_delete: false,
            rename_mode: false,
            rename_input: String::new(),
            status: None,
            list_hit_map: RefCell::new(HitMap::new()),
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

    #[test]
    fn session_picker_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;

        const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];
        for (w, h) in BLOCKER_SIZES {
            let sessions = vec![
                test_session(1, "first session"),
                test_session(2, "second session"),
            ];
            let mut view = picker_with(sessions, None);
            view.current_preview = vec![
                "Title: preview".to_string(),
                "Updated: 2026-06-25 10:30".to_string(),
                String::new(),
                "USER: hello".to_string(),
            ];

            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(view);
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            let text = rows.join("\n");

            // Both panes and their key hints survive at every size. The long
            // in-pane action header truncates to the (sometimes narrow) list
            // pane width, so assert the pane titles, which carry the digit-jump
            // and paging shortcuts and always fit.
            assert!(text.contains("Sessions"), "{w}x{h}: missing Sessions pane");
            assert!(text.contains("History"), "{w}x{h}: missing History pane");
            assert!(text.contains("1-9"), "{w}x{h}: missing 1-9 shortcut hint");
            assert!(text.contains("PgUp/PgDn"), "{w}x{h}: missing paging hint");

            // Composited frame is fully opaque.
            assert!(!text.contains('X'), "{w}x{h}: background bleed-through");
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::WHALE_BG,
                "{w}x{h}: modal interior must be opaque"
            );

            // No horizontal overflow.
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }
}
