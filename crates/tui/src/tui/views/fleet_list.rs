//! `/fleet` list — the primary Fleet surface.
//!
//! One row per saved named Fleet, across both scopes: user-global
//! (`$CODEWHALE_HOME/fleets/`) and folder (`.codewhale/fleets/`). Every row
//! shows its scope and exact source; same-name Fleets in both scopes are two
//! rows, never a silent shadow. The selected Fleet is marked and its operator
//! route summarized. Legacy per-role profiles get one migration banner, not a
//! pile of shadow badges.
//!
//! The view reads and writes the Fleet store directly (local, atomic file
//! operations); it never touches the live session route.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::fleet::store::{
    FleetEntry, FleetScope, SelectedFleet, delete_fleet, list_fleets, migrate_legacy_roster,
    selected_fleet, set_selected,
};
use crate::palette;
use crate::tui::app::App;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_footer,
};

/// What the host should do after this view acted on the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetListOutcome {
    /// A store mutation happened; show `message`, refresh, and pop the stack.
    Done { message: String },
    /// Open the detail view for the given Fleet.
    OpenDetail { name: String, scope: FleetScope },
    /// Nothing to do.
    None,
}

pub struct FleetListView {
    entries: Vec<FleetEntry>,
    /// The active selection snapshot (workspace then personal).
    selected: Option<SelectedFleet>,
    /// Legacy profile count (how many per-role files exist), for the banner.
    legacy_profile_count: usize,
    /// Whether a v2 Fleet named "Default" already exists (so migration is
    /// offered only when it would not clobber).
    default_fleet_exists: bool,
    row: usize,
    /// Saved scope of the row being acted on (delete/select flow through
    /// confirmation state).
    pending_delete: Option<usize>,
    fleet_config: codewhale_config::FleetConfigToml,
    locale: crate::localization::Locale,
    workspace: PathBuf,
}

impl FleetListView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let workspace = app.workspace.clone();
        let entries = list_fleets(&workspace);
        let selected = selected_fleet(&workspace);
        let legacy_profile_count = legacy_profile_file_count(&workspace);
        let default_fleet_exists = entries
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case("default") && !e.legacy);
        Self {
            entries,
            selected,
            legacy_profile_count,
            default_fleet_exists,
            row: 0,
            pending_delete: None,
            fleet_config: config.fleet_config(),
            locale: app.ui_locale,
            workspace,
        }
    }

    fn selected_entry(&self) -> Option<&FleetEntry> {
        self.entries.get(self.row)
    }

    fn visible_entries(&self) -> Vec<usize> {
        // Rows: one per entry; the migration banner is rendered separately.
        (0..self.entries.len()).collect()
    }

    fn banner_visible(&self) -> bool {
        self.legacy_profile_count > 0 && !self.default_fleet_exists
    }

    fn move_row(&mut self, delta: isize) {
        let rows = self.entries.len();
        if rows == 0 {
            return;
        }
        self.row = crate::tui::list_nav::wrap_index(self.row, rows, delta);
    }

    fn footer_hints(&self) -> Vec<ActionHint> {
        let mut hints = vec![
            ActionHint::new("↑/↓", "move"),
            ActionHint::new("Enter", "open"),
        ];
        if !self.entries.is_empty() {
            hints.push(ActionHint::new("u", "select for user"));
            hints.push(ActionHint::new("w", "select for folder"));
            hints.push(ActionHint::new("d", "delete"));
        }
        if self.banner_visible() {
            hints.push(ActionHint::new("m", "migrate"));
        }
        hints.push(ActionHint::new("Esc", "close"));
        hints
    }

    fn confirm_delete(&mut self) -> Option<FleetListOutcome> {
        let idx = self.pending_delete?;
        let entry = self.entries.get(idx)?;
        let name = entry.name.clone();
        let scope = entry.scope;
        match delete_fleet(&name, scope, &self.workspace) {
            Ok(path) => Some(FleetListOutcome::Done {
                message: format!(
                    "Deleted Fleet `{name}` ({}) — removed {}",
                    scope.label(),
                    path.display()
                ),
            }),
            Err(err) => Some(FleetListOutcome::Done {
                message: format!("Delete failed: {err}"),
            }),
        }
    }
}

impl ModalView for FleetListView {
    fn kind(&self) -> ModalKind {
        ModalKind::FleetList
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if self.pending_delete.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    let outcome = self.confirm_delete();
                    self.pending_delete = None;
                    return outcome_to_action(outcome);
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.pending_delete = None;
                    return ViewAction::None;
                }
                _ => return ViewAction::None,
            }
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_row(-1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_row(1);
                ViewAction::None
            }
            KeyCode::Enter => {
                let Some(entry) = self.selected_entry() else {
                    return ViewAction::None;
                };
                if entry.legacy {
                    return ViewAction::Emit(ViewEvent::OpenTextPager {
                        title: format!("Fleet `{}` — legacy format", entry.name),
                        content: format!(
                            "This Fleet file predates the named-Fleet format ({}).\n\n\
                             It is listed so nothing you saved disappears, but it is \
                             read-only here. To edit it, create a new Fleet and copy \
                             the settings you want; legacy files are never migrated \
                             silently.\n\nParse error: {}",
                            entry.path.display(),
                            entry.parse_error.as_deref().unwrap_or("unknown")
                        ),
                    });
                }
                ViewAction::Emit(ViewEvent::FleetListOpenDetailRequested {
                    name: entry.name.clone(),
                    scope: entry.scope,
                })
            }
            KeyCode::Char('u') => {
                let Some(entry) = self.selected_entry() else {
                    return ViewAction::None;
                };
                if entry.legacy {
                    return ViewAction::None;
                }
                match set_selected(&entry.name, FleetScope::Personal, &self.workspace) {
                    Ok(path) => outcome_to_action(Some(FleetListOutcome::Done {
                        message: format!(
                            "Selected Fleet `{}` as your user-global default — wrote {}",
                            entry.name,
                            path.display()
                        ),
                    })),
                    Err(err) => outcome_to_action(Some(FleetListOutcome::Done {
                        message: format!("Selection failed: {err}"),
                    })),
                }
            }
            KeyCode::Char('w') => {
                let Some(entry) = self.selected_entry() else {
                    return ViewAction::None;
                };
                if entry.legacy {
                    return ViewAction::None;
                }
                match set_selected(&entry.name, FleetScope::Workspace, &self.workspace) {
                    Ok(path) => outcome_to_action(Some(FleetListOutcome::Done {
                        message: format!(
                            "Selected Fleet `{}` for this folder only — wrote {}",
                            entry.name,
                            path.display()
                        ),
                    })),
                    Err(err) => outcome_to_action(Some(FleetListOutcome::Done {
                        message: format!("Selection failed: {err}"),
                    })),
                }
            }
            KeyCode::Char('d') => {
                let Some(entry) = self.selected_entry() else {
                    return ViewAction::None;
                };
                if entry.legacy {
                    return ViewAction::None;
                }
                self.pending_delete = Some(self.row);
                ViewAction::None
            }
            KeyCode::Char('m') if self.banner_visible() => {
                match migrate_legacy_roster(
                    &self.fleet_config,
                    &self.workspace,
                    true,
                    FleetScope::Personal,
                ) {
                    Ok(receipt) => {
                        let mut content = format!(
                            "Migrated {} legacy role profiles into Fleet `Default` \
                             (user-global) — wrote {}\n\n",
                            receipt.rows.len(),
                            receipt.saved_to.display()
                        );
                        for row in &receipt.rows {
                            let pin = match &row.pin {
                                Some((model, provider)) => {
                                    format!("{provider}/{model}")
                                }
                                None => "inherit session route".to_string(),
                            };
                            content.push_str(&format!("- {} → {pin}", row.id));
                            if let Some(shadow) = &row.conflicting_shadow {
                                content.push_str(&format!(" [conflict: {shadow}]"));
                            }
                            content.push('\n');
                        }
                        content.push_str(
                            "\nLegacy profile files were left untouched — they are no \
                             longer live configuration once a Fleet is selected.",
                        );
                        if let Ok(path) =
                            set_selected("Default", FleetScope::Personal, &self.workspace)
                        {
                            content.push_str(&format!(
                                "\n\nFleet `Default` is now your user-global default — wrote {}.",
                                path.display()
                            ));
                        }
                        ViewAction::Emit(ViewEvent::OpenTextPager {
                            title: "Legacy migration receipt".to_string(),
                            content,
                        })
                    }
                    Err(err) => ViewAction::Emit(ViewEvent::OpenTextPager {
                        title: "Legacy migration failed".to_string(),
                        content: format!("{err:#}"),
                    }),
                }
            }
            KeyCode::Home => {
                self.row = 0;
                ViewAction::None
            }
            KeyCode::End => {
                self.row = self.entries.len().saturating_sub(1);
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            if self.pending_delete.is_some() {
                self.pending_delete = None;
                return ViewAction::None;
            }
            let (rows_top, _) = (5u16, 0u16);
            if mouse.row >= rows_top {
                let idx = usize::from(mouse.row - rows_top) + self.row.saturating_sub(0);
                if idx < self.entries.len() {
                    self.row = idx;
                    return ViewAction::None;
                }
            }
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        Block::default()
            .style(Style::default().bg(palette::WHALE_BG))
            .render(area, buf);

        let hints = self.footer_hints();
        let content = render_modal_footer(area, buf, &hints);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(4), Constraint::Min(1)])
            .split(content);

        // Header: name + selected summary.
        let selected_line = match &self.selected {
            Some(sel) => format!(
                "Selected: `{}` ({}) — {}",
                sel.name,
                sel.scope.label(),
                sel.path.display()
            ),
            None => "No Fleet selected — the session uses its own route and the legacy roster."
                .to_string(),
        };
        let mut header = vec![
            Line::from(vec![
                Span::styled(
                    "─ Fleet ",
                    Style::default().fg(palette::WHALE_ACTION).bold(),
                ),
                Span::styled(
                    "saved configurations · one operator, one roster, one scope",
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {selected_line}"),
                Style::default().fg(palette::TEXT_SECONDARY),
            )),
        ];
        if self.banner_visible() {
            header.push(Line::from(vec![Span::styled(
                format!(
                    "  ⚠ {} legacy role profile(s) found — press m to migrate them into a \
                         Fleet (nothing is changed until you do)",
                    self.legacy_profile_count
                ),
                Style::default().fg(palette::WHALE_HUMAN),
            )]));
        }
        Paragraph::new(header)
            .wrap(Wrap { trim: false })
            .render(chunks[0], buf);

        self.render_rows(chunks[1], buf);
    }
}

impl FleetListView {
    fn render_rows(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if self.entries.is_empty() {
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  No saved Fleets yet.",
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled(
                    "  Select a route with /model and /provider, then save it as a Fleet from \
                     the receipt — or press u/w to select one once it exists.",
                    Style::default().fg(palette::TEXT_DIM),
                ),
            ]))
            .render(area, buf);
            return;
        }

        let rows_visible = usize::from(area.height).max(1);
        let scroll = self.row.saturating_sub(rows_visible.saturating_sub(1));

        let mut lines = Vec::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            if idx < scroll || idx >= scroll + rows_visible {
                continue;
            }
            let selected = idx == self.row;
            let is_selected_fleet = self
                .selected
                .as_ref()
                .is_some_and(|sel| sel.name == entry.name && sel.scope == entry.scope);

            let mut spans = Vec::new();
            let marker = if is_selected_fleet { "▸ " } else { "  " };
            let base = if selected {
                Style::default().fg(palette::WHALE_ACTION).bold()
            } else {
                Style::default().fg(palette::TEXT_SECONDARY)
            };
            spans.push(Span::styled(marker, base));
            let name = if selected {
                "» ".to_string()
            } else {
                "  ".to_string()
            };
            spans.push(Span::styled(format!("{name}{}", entry.name), base));

            let scope_badge = match entry.scope {
                FleetScope::Personal => " [user]",
                FleetScope::Workspace => " [folder]",
            };
            spans.push(Span::styled(
                scope_badge,
                Style::default().fg(palette::TEXT_MUTED),
            ));

            if entry.legacy {
                spans.push(Span::styled(
                    "  legacy format — read-only",
                    Style::default().fg(palette::WHALE_HUMAN),
                ));
            } else if let Some(err) = &entry.parse_error {
                spans.push(Span::styled(
                    format!("  unreadable: {err}"),
                    Style::default().fg(palette::WHALE_HUMAN),
                ));
            }

            if is_selected_fleet && !selected {
                spans.push(Span::styled(
                    "  ★",
                    Style::default().fg(palette::WHALE_ACTION),
                ));
            }

            if self.pending_delete == Some(idx) {
                lines.push(Line::from(vec![Span::styled(
                    format!("  Delete `{}` ({})? y/n", entry.name, entry.scope.label()),
                    Style::default().fg(palette::WHALE_HUMAN),
                )]));
            } else {
                lines.push(Line::from(spans));
                // Second line: source + operator summary (compact).
                let summary = self.entry_summary(entry);
                lines.push(Line::from(Span::styled(
                    format!("    {summary}"),
                    Style::default().fg(palette::TEXT_DIM),
                )));
            }
        }

        let text = ratatui::text::Text::from(lines);
        Paragraph::new(text).render(area, buf);
    }

    fn entry_summary(&self, entry: &FleetEntry) -> String {
        if entry.legacy {
            return entry.path.display().to_string();
        }
        // Read the file to summarize operator + members. Parsing errors are
        // already shown on the row.
        let Ok(text) = std::fs::read_to_string(&entry.path) else {
            return entry.path.display().to_string();
        };
        match crate::fleet::store::FleetFile::parse(&text) {
            Ok(fleet) => {
                let operator = match &fleet.operator {
                    Some(op) => format!("{}/{}", op.provider, op.model),
                    None => "inherits session route".to_string(),
                };
                format!(
                    "operator: {operator} · members: {} · {}",
                    fleet.members.len(),
                    entry.path.display()
                )
            }
            Err(err) => format!("{} — {err}", entry.path.display()),
        }
    }
}

fn outcome_to_action(outcome: Option<FleetListOutcome>) -> ViewAction {
    match outcome {
        Some(FleetListOutcome::Done { message }) => {
            ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged { message })
        }
        Some(FleetListOutcome::OpenDetail { name, scope }) => {
            ViewAction::Emit(ViewEvent::FleetListOpenDetailRequested { name, scope })
        }
        Some(FleetListOutcome::None) | None => ViewAction::None,
    }
}

/// How many legacy per-role profile files exist across both scopes.
fn legacy_profile_file_count(workspace: &std::path::Path) -> usize {
    let mut count = 0;
    if let Ok(dir) = crate::fleet::profile::personal_agent_profile_dir() {
        if let Ok(read) = std::fs::read_dir(dir) {
            count += read
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .count();
        }
    }
    let ws_dir = workspace.join(".codewhale").join("agents");
    if let Ok(read) = std::fs::read_dir(ws_dir) {
        count += read
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .count();
    }
    count
}

/// Read the `[fleet]` table for migration input. The view holds `config` only
/// at construction; the migration path re-reads via the standard loader so it
/// never works from a stale snapshot.
fn config_fleet_table() -> codewhale_config::FleetConfigToml {
    crate::config::Config::default().fleet_config()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::fleet::store::{FleetFile, save_fleet};
    use crate::tui::app::{App, TuiOptions};
    use std::sync::OnceLock;

    fn sealed_home() -> &'static std::path::Path {
        static HOME: OnceLock<PathBuf> = OnceLock::new();
        HOME.get_or_init(|| {
            let dir = tempfile::TempDir::new().expect("temp dir").into_path();
            std::fs::create_dir_all(dir.join("fleets")).expect("fleets dir");
            dir
        })
    }

    fn app_in(workspace: PathBuf) -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(workspace.clone())
        };
        let mut app = App::new(options, &Config::default());
        app.workspace = workspace;
        app
    }

    fn sample_fleet(name: &str) -> FleetFile {
        let mut fleet = FleetFile::new(name.to_string(), None).unwrap();
        fleet.operator = Some(crate::fleet::store::FleetOperator {
            provider: "deepseek".to_string(),
            model: "deepseek-v4-flash".to_string(),
            reasoning: None,
        });
        fleet
    }

    fn save_in(workspace: &std::path::Path, scope: FleetScope, name: &str) {
        let fleet = sample_fleet(name);
        let _ = save_fleet(&fleet, scope, workspace).expect("save");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[test]
    fn list_renders_both_scopes_and_marks_selection() {
        let _lock = crate::test_support::lock_test_env();
        let prev = std::env::var_os("CODEWHALE_HOME");
        // SAFETY: serialised by lock_test_env.
        unsafe { std::env::set_var("CODEWHALE_HOME", sealed_home()) };
        let ws = tempfile::TempDir::new().unwrap();

        save_in(ws.path(), FleetScope::Personal, "DeepSeek Flash");
        save_in(ws.path(), FleetScope::Workspace, "OpenAI Codex");
        set_selected("OpenAI Codex", FleetScope::Workspace, ws.path()).unwrap();

        let mut app = app_in(ws.path().to_path_buf());
        let view = FleetListView::new(&app, &Config::default());

        // Both scopes listed under their display names; the workspace one
        // selected. The shared sealed home may hold other fleets, so assert
        // by name+scope, never by total count.
        assert!(
            view.entries
                .iter()
                .any(|e| e.name == "DeepSeek Flash" && e.scope == FleetScope::Personal),
            "{:?}",
            view.entries
        );
        assert!(
            view.entries
                .iter()
                .any(|e| e.name == "OpenAI Codex" && e.scope == FleetScope::Workspace),
            "{:?}",
            view.entries
        );
        let selected = view.selected.as_ref().unwrap();
        assert_eq!(selected.name, "OpenAI Codex");
        assert_eq!(selected.scope, FleetScope::Workspace);

        // Restore env.
        // SAFETY: serialised by lock_test_env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CODEWHALE_HOME", v),
                None => std::env::remove_var("CODEWHALE_HOME"),
            }
        }
        let _ = &mut app;
    }

    #[test]
    fn select_user_global_writes_receipt_naming_the_file() {
        let _lock = crate::test_support::lock_test_env();
        let prev = std::env::var_os("CODEWHALE_HOME");
        // SAFETY: serialised by lock_test_env.
        unsafe { std::env::set_var("CODEWHALE_HOME", sealed_home()) };
        let ws = tempfile::TempDir::new().unwrap();

        save_in(ws.path(), FleetScope::Personal, "DeepSeek Flash");
        let mut view = FleetListView::new(&app_in(ws.path().to_path_buf()), &Config::default());

        let action = view.handle_key(key(KeyCode::Char('u')));
        let ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged { message }) = action else {
            panic!("expected FleetStoreChanged, got {action:?}");
        };
        assert!(message.contains("DeepSeek Flash"), "{message}");
        assert!(message.contains("user-global"), "{message}");
        assert!(message.contains("selected"), "{message}");

        // The selection really persisted, at the personal scope. The
        // SelectedFleet path is the fleet file the marker names.
        let sel = selected_fleet(ws.path()).expect("selection");
        assert_eq!(sel.name, "DeepSeek Flash");
        assert_eq!(sel.scope, FleetScope::Personal);
        assert!(
            sel.path.ends_with("fleets/deepseek-flash.toml"),
            "{:?}",
            sel.path
        );

        // SAFETY: serialised by lock_test_env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CODEWHALE_HOME", v),
                None => std::env::remove_var("CODEWHALE_HOME"),
            }
        }
    }

    #[test]
    fn delete_requires_confirmation_and_removes_the_file() {
        let _lock = crate::test_support::lock_test_env();
        let ws = tempfile::TempDir::new().unwrap();

        save_in(ws.path(), FleetScope::Workspace, "Temp Fleet");
        let mut view = FleetListView::new(&app_in(ws.path().to_path_buf()), &Config::default());
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].name, "Temp Fleet");

        // 'd' arms the confirmation; the file is still there.
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('d'))),
            ViewAction::None
        ));
        assert!(
            list_fleets(ws.path())
                .iter()
                .any(|e| e.name == "Temp Fleet")
        );

        // 'n' cancels.
        view.handle_key(key(KeyCode::Char('n')));
        assert!(
            list_fleets(ws.path())
                .iter()
                .any(|e| e.name == "Temp Fleet")
        );

        // 'd' then 'y' deletes and emits a receipt naming the removed path.
        view.handle_key(key(KeyCode::Char('d')));
        let action = view.handle_key(key(KeyCode::Char('y')));
        let ViewAction::EmitAndClose(ViewEvent::FleetStoreChanged { message }) = action else {
            panic!("expected FleetStoreChanged, got {action:?}");
        };
        assert!(message.contains("Deleted Fleet `Temp Fleet`"), "{message}");
        assert!(list_fleets(ws.path()).is_empty());
    }

    #[test]
    fn legacy_entry_opens_a_pager_instead_of_editing() {
        let _lock = crate::test_support::lock_test_env();
        let prev = std::env::var_os("CODEWHALE_HOME");
        // SAFETY: serialised by lock_test_env.
        unsafe { std::env::set_var("CODEWHALE_HOME", sealed_home()) };
        let ws = tempfile::TempDir::new().unwrap();

        // A legacy exact fleet file (workflow schema) in the personal dir.
        std::fs::create_dir_all(sealed_home().join("fleets")).unwrap();
        std::fs::write(
            sealed_home().join("fleets/stopship.toml"),
            r#"schema = "exact"
schema_revision = 1
name = "stopship"
members = []"#,
        )
        .unwrap();

        let mut view = FleetListView::new(&app_in(ws.path().to_path_buf()), &Config::default());
        let idx = view
            .entries
            .iter()
            .position(|e| e.name == "stopship")
            .expect("legacy entry listed");
        view.row = idx;
        let entry = view.selected_entry().expect("legacy entry listed");
        assert!(entry.legacy);

        let action = view.handle_key(key(KeyCode::Enter));
        assert!(
            matches!(action, ViewAction::Emit(ViewEvent::OpenTextPager { .. })),
            "legacy entry must open a read-only pager: {action:?}"
        );

        // SAFETY: serialised by lock_test_env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CODEWHALE_HOME", v),
                None => std::env::remove_var("CODEWHALE_HOME"),
            }
        }
    }

    #[test]
    fn migration_banner_appears_only_when_needed_and_migrates() {
        let _lock = crate::test_support::lock_test_env();
        let prev = std::env::var_os("CODEWHALE_HOME");
        // SAFETY: serialised by lock_test_env.
        unsafe { std::env::set_var("CODEWHALE_HOME", sealed_home()) };
        let ws = tempfile::TempDir::new().unwrap();

        // No legacy profiles: no banner.
        let view = FleetListView::new(&app_in(ws.path().to_path_buf()), &Config::default());
        assert!(!view.banner_visible());

        // A workspace legacy role profile.
        let agents = ws.path().join(".codewhale/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("scout.toml"),
            r#"id = "scout"
role_hint = "scout"
model = "deepseek-v4-flash"
provider = "deepseek"
"#,
        )
        .unwrap();

        let mut view = FleetListView::new(&app_in(ws.path().to_path_buf()), &Config::default());
        assert!(
            view.banner_visible(),
            "banner must show with legacy profiles"
        );

        let action = view.handle_key(key(KeyCode::Char('m')));
        assert!(
            matches!(action, ViewAction::Emit(ViewEvent::OpenTextPager { .. })),
            "migration must open the receipt pager: {action:?}"
        );

        // The Default fleet now exists and is the user-global selection.
        let entries = list_fleets(ws.path());
        assert!(
            entries.iter().any(|e| e.name == "Default" && !e.legacy),
            "{entries:?}"
        );
        let sel = selected_fleet(ws.path()).expect("selection after migration");
        assert_eq!(sel.name, "Default");

        // SAFETY: serialised by lock_test_env.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("CODEWHALE_HOME", v),
                None => std::env::remove_var("CODEWHALE_HOME"),
            }
        }
    }
}
