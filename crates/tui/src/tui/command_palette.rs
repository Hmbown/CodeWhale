//! Command palette modal for quick command/skill insertion.
//!
//! Product job (#4276): **find and run one action** — not a dense manual.
//! Help owns concepts; Config owns settings; Fleet owns worker readiness.

use std::cell::RefCell;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::commands;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::skills;
use crate::tools::spec::ApprovalRequirement;
use crate::tools::spec::ToolCapability;
use crate::tools::{ToolContext, ToolRegistryBuilder};
use crate::tui::hit_region::HitMap;
use crate::tui::views::{
    ActionHint, CommandPaletteAction, ModalKind, ModalView, ViewAction, ViewEvent,
    render_modal_chrome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PaletteSection {
    Action,
    Command,
    Skill,
    Tool,
    Mcp,
}

#[derive(Debug, Clone)]
pub struct CommandPaletteEntry {
    section: PaletteSection,
    pub label: String,
    pub description: String,
    pub command: String,
    pub action: CommandPaletteAction,
    show_on_empty_query: bool,
}

#[cfg(test)]
impl CommandPaletteEntry {
    #[must_use]
    pub fn section(&self) -> PaletteSection {
        self.section
    }
}

pub struct CommandPaletteView {
    locale: Locale,
    entries: Vec<CommandPaletteEntry>,
    filtered: Vec<usize>,
    query: String,
    selected: usize,
    /// Row hit regions from the last paint (shared by hover + click).
    hit_map: RefCell<HitMap>,
}

pub fn build_entries(
    locale: Locale,
    skills_dir: &Path,
    skills_scan_codewhale_only: bool,
    workspace: &Path,
    mcp_config_path: &Path,
    mcp_snapshot: Option<&crate::mcp::McpManagerSnapshot>,
) -> Vec<CommandPaletteEntry> {
    let mut entries = Vec::new();
    commands::user_registry::with_registry_for_workspace(Some(workspace), |user_registry| {
        for command in commands::command_infos() {
            if user_registry.get(command.name).is_some() {
                continue;
            }
            let mut description =
                palette_description_for_unshadowed_aliases(command, locale, user_registry);
            if command.requires_argument() {
                description.push_str("  ");
                description.push_str(command.usage);
            }
            let action = if command_runs_directly(command.name) {
                CommandPaletteAction::ExecuteCommand {
                    command: format!("/{}", command.name),
                }
            } else {
                CommandPaletteAction::InsertText {
                    text: command.palette_command(),
                }
            };
            entries.push(CommandPaletteEntry {
                section: PaletteSection::Command,
                label: format!("/{}", command.name),
                description,
                command: command.palette_command(),
                action,
                show_on_empty_query: command.show_in_empty_discovery(),
            });
        }

        for command in user_registry.iter().filter(|command| !command.hidden) {
            let mut description = command
                .description
                .clone()
                .unwrap_or_else(|| "User-defined command".to_string());
            if let Some(hint) = &command.argument_hint
                && !hint.trim().is_empty()
            {
                description.push_str("  ");
                description.push_str(hint.trim());
            }
            let slash_command = format!("/{}", command.name);
            let action = if command.argument_hint.is_some() {
                CommandPaletteAction::InsertText {
                    text: format!("{slash_command} "),
                }
            } else {
                CommandPaletteAction::ExecuteCommand {
                    command: slash_command.clone(),
                }
            };
            entries.push(CommandPaletteEntry {
                section: PaletteSection::Command,
                label: slash_command.clone(),
                description,
                command: slash_command,
                action,
                show_on_empty_query: true,
            });
        }
    });

    let skills = skills::discover_for_workspace_and_dir_with_mode(
        workspace,
        skills_dir,
        skills::SkillDiscoveryMode::from_codewhale_only(skills_scan_codewhale_only),
    );
    for skill in skills.list() {
        entries.push(CommandPaletteEntry {
            section: PaletteSection::Skill,
            label: format!("${}", skill.name),
            description: skill.description.clone(),
            command: format!("${}", skill.name),
            action: CommandPaletteAction::ExecuteCommand {
                command: format!("${}", skill.name),
            },
            show_on_empty_query: true,
        });
    }

    let context = ToolContext::new(workspace);
    let registry = ToolRegistryBuilder::new()
        .with_file_tools()
        .with_search_tools()
        .with_shell_tools()
        .with_web_tools()
        .with_git_tools()
        .with_user_input_tool()
        .with_parallel_tool()
        .with_patch_tools()
        .with_note_tool()
        .with_diagnostics_tool()
        .with_project_tools()
        .with_test_runner_tool()
        .build(context);

    let mut tool_entries = registry
        .all()
        .into_iter()
        .filter_map(|tool| {
            let name = tool.name().to_string();
            let capabilities = tool.capabilities();

            let mut tags = Vec::new();
            if tool.is_read_only() {
                tags.push("read-only");
            }
            if capabilities.contains(&ToolCapability::WritesFiles) {
                tags.push("writes");
            }
            if capabilities.contains(&ToolCapability::ExecutesCode) {
                tags.push("shell");
            }
            if capabilities.contains(&ToolCapability::Network) {
                tags.push("network");
            }
            if tool.supports_parallel() {
                tags.push("parallel");
            }
            match tool.approval_requirement() {
                ApprovalRequirement::Required => tags.push("requires approval"),
                ApprovalRequirement::Suggest => tags.push("suggest approval"),
                ApprovalRequirement::Auto => {}
            }

            let mut description = tool.description().to_string();
            if !tags.is_empty() {
                description.push_str(" [");
                description.push_str(&tags.join(", "));
                description.push(']');
            }

            if name.trim().is_empty() {
                return None;
            }
            Some(CommandPaletteEntry {
                section: PaletteSection::Tool,
                label: format!("tool:{name}"),
                description: description.clone(),
                command: name,
                action: CommandPaletteAction::OpenTextPager {
                    title: format!("Tool: {}", tool.name()),
                    content: format_tool_details(tool.name(), tool.description(), &tags),
                },
                show_on_empty_query: true,
            })
        })
        .collect::<Vec<_>>();
    tool_entries.sort_by(|a, b| a.label.cmp(&b.label));
    entries.extend(tool_entries);

    entries.extend(build_mcp_entries(workspace, mcp_config_path, mcp_snapshot));

    entries.sort_by(|a, b| a.label.cmp(&b.label));
    entries.sort_by_key(|entry| entry.section);
    entries
}

fn palette_description_for_unshadowed_aliases(
    command: &commands::CommandInfo,
    locale: Locale,
    user_registry: &commands::user_registry::UserCommandRegistry,
) -> String {
    let desc = command.description_for(locale);
    let aliases = command
        .aliases
        .iter()
        .copied()
        .filter(|alias| user_registry.get(alias).is_none())
        .collect::<Vec<_>>();
    if aliases.len() == command.aliases.len() {
        return command.palette_description_for(locale);
    }
    if aliases.is_empty() {
        desc.to_string()
    } else {
        format!("{}  aliases: {}", desc, aliases.join(", "))
    }
}

fn build_mcp_entries(
    workspace: &Path,
    mcp_config_path: &Path,
    mcp_snapshot: Option<&crate::mcp::McpManagerSnapshot>,
) -> Vec<CommandPaletteEntry> {
    let owned_snapshot = if mcp_snapshot.is_none() {
        crate::mcp::manager_snapshot_from_config_with_workspace(mcp_config_path, workspace, false)
            .ok()
    } else {
        None
    };
    let snapshot = mcp_snapshot.or(owned_snapshot.as_ref());
    let mut entries = vec![CommandPaletteEntry {
        section: PaletteSection::Mcp,
        label: "mcp:manager".to_string(),
        description: format!("Open MCP manager ({})", mcp_config_path.display()),
        command: "/mcp".to_string(),
        action: CommandPaletteAction::ExecuteCommand {
            command: "/mcp".to_string(),
        },
        show_on_empty_query: true,
    }];

    let Some(snapshot) = snapshot else {
        return entries;
    };

    for server in &snapshot.servers {
        let state = if server.enabled {
            if server.connected {
                "connected"
            } else if server.error.is_some() {
                "failed"
            } else {
                "enabled"
            }
        } else {
            "disabled"
        };
        entries.push(CommandPaletteEntry {
            section: PaletteSection::Mcp,
            label: format!("mcp:{}", server.name),
            description: format!(
                "{} {} [{}] tools={} resources={} prompts={}",
                server.transport,
                server.command_or_url,
                state,
                server.tools.len(),
                server.resources.len(),
                server.prompts.len()
            ),
            command: format!("/mcp show {}", server.name),
            action: CommandPaletteAction::OpenTextPager {
                title: format!("MCP Server: {}", server.name),
                content: format_mcp_server_details(snapshot, server),
            },
            show_on_empty_query: true,
        });

        for tool in &server.tools {
            entries.push(CommandPaletteEntry {
                section: PaletteSection::Mcp,
                label: format!("mcp:{}:tool:{}", server.name, tool.name),
                description: format!(
                    "{}{}",
                    tool.model_name,
                    tool.description
                        .as_ref()
                        .map_or(String::new(), |desc| format!(" - {desc}"))
                ),
                command: tool.model_name.clone(),
                action: CommandPaletteAction::OpenTextPager {
                    title: format!("MCP Tool: {}", tool.model_name),
                    content: format!(
                        "Server: {}\nRuntime name: {}\nKind: tool\n\n{}",
                        server.name,
                        tool.model_name,
                        tool.description.as_deref().unwrap_or("(no description)")
                    ),
                },
                show_on_empty_query: true,
            });
            // Add a "use" entry that inserts the tool's model_name into the input
            // so users can quickly reference the tool in their message to the AI.
            if !tool.model_name.trim().is_empty() {
                entries.push(CommandPaletteEntry {
                    section: PaletteSection::Mcp,
                    label: format!("mcp:{}:tool:{} > use", server.name, tool.name),
                    description: format!(
                        "Insert {} into input — type args then send{}",
                        tool.model_name,
                        tool.description
                            .as_ref()
                            .map_or(String::new(), |desc| format!(" ({desc})"))
                    ),
                    command: tool.model_name.clone(),
                    action: CommandPaletteAction::InsertText {
                        text: tool.model_name.clone(),
                    },
                    show_on_empty_query: true,
                });
            }
        }

        for resource in &server.resources {
            entries.push(CommandPaletteEntry {
                section: PaletteSection::Mcp,
                label: format!("mcp:{}:resource:{}", server.name, resource.name),
                description: resource
                    .description
                    .clone()
                    .unwrap_or_else(|| "MCP resource".to_string()),
                command: resource.name.clone(),
                action: CommandPaletteAction::OpenTextPager {
                    title: format!("MCP Resource: {}", resource.name),
                    content: format!(
                        "Server: {}\nResource: {}\nModel helper: list_mcp_resources / read_mcp_resource",
                        server.name, resource.name
                    ),
                },
                show_on_empty_query: true,
            });
        }

        for prompt in &server.prompts {
            entries.push(CommandPaletteEntry {
                section: PaletteSection::Mcp,
                label: format!("mcp:{}:prompt:{}", server.name, prompt.name),
                description: format!(
                    "{}{}",
                    prompt.model_name,
                    prompt
                        .description
                        .as_ref()
                        .map_or(String::new(), |desc| format!(" - {desc}"))
                ),
                command: prompt.model_name.clone(),
                action: CommandPaletteAction::OpenTextPager {
                    title: format!("MCP Prompt: {}", prompt.model_name),
                    content: format!(
                        "Server: {}\nRuntime name: {}\nKind: prompt",
                        server.name, prompt.model_name
                    ),
                },
                show_on_empty_query: true,
            });
        }
    }

    entries
}

fn format_mcp_server_details(
    snapshot: &crate::mcp::McpManagerSnapshot,
    server: &crate::mcp::McpServerSnapshot,
) -> String {
    let mut lines = vec![
        format!("Config: {}", snapshot.config_path.display()),
        format!("Server: {}", server.name),
        format!("Enabled: {}", server.enabled),
        format!("Connected: {}", server.connected),
        format!("Transport: {}", server.transport),
        format!("Target: {}", server.command_or_url),
        format!(
            "Timeouts: connect={}s execute={}s read={}s",
            server.connect_timeout, server.execute_timeout, server.read_timeout
        ),
    ];
    if let Some(error) = server.error.as_ref() {
        lines.push(format!("Error: {error}"));
    }
    lines.push(String::new());
    lines.push(format!("Tools ({})", server.tools.len()));
    for tool in &server.tools {
        lines.push(format!("  - {}", tool.model_name));
    }
    lines.push(format!("Resources ({})", server.resources.len()));
    for resource in &server.resources {
        lines.push(format!("  - {}", resource.name));
    }
    lines.push(format!("Prompts ({})", server.prompts.len()));
    for prompt in &server.prompts {
        lines.push(format!("  - {}", prompt.model_name));
    }
    lines.join("\n")
}

fn parse_section_term(term: &str) -> Option<(PaletteSection, String)> {
    let (section, query) = term.split_once(':')?;

    if section.is_empty() || query.is_empty() {
        return None;
    }

    let query = query.to_ascii_lowercase();
    let section = match section {
        "a" | "action" | "actions" => PaletteSection::Action,
        "c" | "cmd" | "command" | "commands" => PaletteSection::Command,
        "s" | "skill" | "skills" => PaletteSection::Skill,
        "t" | "tool" | "tools" => PaletteSection::Tool,
        "m" | "mcp" => PaletteSection::Mcp,
        _ => return None,
    };

    Some((section, query))
}

fn section_tag(section: PaletteSection) -> &'static str {
    match section {
        PaletteSection::Action => "action",
        PaletteSection::Command => "command",
        PaletteSection::Skill => "skill",
        PaletteSection::Tool => "tool",
        PaletteSection::Mcp => "mcp",
    }
}

fn section_rank(section: PaletteSection) -> usize {
    match section {
        PaletteSection::Action => 0,
        PaletteSection::Command => 1,
        PaletteSection::Skill => 2,
        PaletteSection::Tool => 3,
        PaletteSection::Mcp => 4,
    }
}

fn command_runs_directly(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "clear"
            | "exit"
            | "provider"
            | "model"
            | "models"
            | "modeldb"
            | "queue"
            | "stash"
            | "hooks"
            | "subagents"
            | "links"
            | "home"
            | "save"
            | "sessions"
            | "compact"
            | "export"
            | "config"
            | "fleet"
            | "mode"
            | "statusline"
            | "yolo"
            | "agent"
            | "plan"
            | "trust"
            | "logout"
            | "tokens"
            | "change"
            | "system"
            | "context"
            | "undo"
            | "retry"
            | "init"
            | "settings"
            | "skills"
            | "cost"
            | "jobs"
            | "mcp"
            | "task"
    )
}

fn format_tool_details(name: &str, description: &str, tags: &[&str]) -> String {
    let mut lines = vec![
        format!("Tool: {name}"),
        String::new(),
        description.to_string(),
    ];
    if !tags.is_empty() {
        lines.push(String::new());
        lines.push(format!("Capabilities: {}", tags.join(", ")));
    }
    lines.push(String::new());
    lines.push(
        "Use slash commands and skills here for direct actions; use tool entries to inspect what the agent can call."
            .to_string(),
    );
    lines.join("\n")
}

fn term_score(term: &str, label: &str, description: &str, command: &str, haystack: &str) -> usize {
    if term.is_empty() {
        return 0;
    }

    if label == term || command == term || description == term {
        return 0;
    }

    if label.starts_with(term) {
        return 8;
    }

    if command.starts_with(term) {
        return 16;
    }

    if description.contains(term) {
        return 64;
    }

    if label.contains(term) {
        return 32;
    }

    if command.contains(term) {
        return 48;
    }

    if haystack.contains(term) {
        return 96;
    }

    128
}

fn entry_match_score(entry: &CommandPaletteEntry, terms: &[&str]) -> Option<usize> {
    if terms.is_empty() {
        return Some(0);
    }

    let section = section_tag(entry.section);
    let label = entry.label.to_ascii_lowercase();
    let description = entry.description.to_ascii_lowercase();
    let command = entry.command.to_ascii_lowercase();
    let entry_text = format!("{section} {label} {description} {command}");

    let mut total_score = 0usize;

    for term in terms {
        if let Some((required_section, scoped_query)) = parse_section_term(term) {
            if entry.section != required_section {
                return None;
            }
            if !entry_text.contains(&scoped_query) {
                return None;
            }
            total_score += term_score(&scoped_query, &label, &description, &command, &entry_text);
            continue;
        }

        if !entry_text.contains(term) {
            return None;
        }
        total_score += term_score(term, &label, &description, &command, &entry_text);
    }

    Some(total_score)
}

/// Number of rendered rows the entry loop consumes for the window
/// `sections[start..end]`: one row per entry, plus one section-label row each
/// time the section changes, plus a separator blank before every section group
/// after the first.
fn rendered_entry_rows(sections: &[PaletteSection], start: usize, end: usize) -> usize {
    let end = end.min(sections.len());
    if start >= end {
        return 0;
    }
    let mut rows = 0usize;
    let mut active: Option<PaletteSection> = None;
    for (slot, sec) in sections[start..end].iter().enumerate() {
        if active != Some(*sec) {
            if slot > 0 {
                rows += 1; // separator blank
            }
            rows += 1; // section label
            active = Some(*sec);
        }
        rows += 1; // the entry itself
    }
    rows
}

/// Compute the `[start, end)` window of filtered entries to render so that the
/// selected entry is always visible and the rendered rows — entries plus the
/// per-section labels and separators inserted between them — fit within
/// `available` rows.
///
/// The previous logic sized the window purely by entry count (`popup_height -
/// 7`) while the same fixed-height area also held the header, section labels,
/// and separators. Those uncounted rows pushed the selection past the bottom
/// clip line, so it vanished and the list appeared frozen until the index
/// finally exceeded the (overlarge) entry budget (#2590).
fn visible_entry_window(
    sections: &[PaletteSection],
    selected: usize,
    available: usize,
) -> (usize, usize) {
    let total = sections.len();
    if total == 0 || available == 0 {
        return (0, 0);
    }
    let selected = selected.min(total - 1);
    // Always include the selected row, then greedily grow downward and upward
    // while the fully-rendered window still fits. Growth only ever adds rows,
    // so the greedy expansion terminates at the largest fitting window.
    let mut start = selected;
    let mut end = selected + 1;
    loop {
        let mut progressed = false;
        if end < total && rendered_entry_rows(sections, start, end + 1) <= available {
            end += 1;
            progressed = true;
        }
        if start > 0 && rendered_entry_rows(sections, start - 1, end) <= available {
            start -= 1;
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    (start, end)
}

impl CommandPaletteView {
    #[cfg(test)]
    pub fn new(entries: Vec<CommandPaletteEntry>) -> Self {
        Self::new_for_locale(Locale::En, entries)
    }

    pub fn new_for_locale(locale: Locale, entries: Vec<CommandPaletteEntry>) -> Self {
        let mut view = Self {
            locale,
            entries,
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
            hit_map: RefCell::new(HitMap::new()),
        };
        view.refilter();
        view
    }

    fn refilter(&mut self) {
        let query = self.query.trim().to_ascii_lowercase();
        let terms: Vec<&str> = query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .collect();

        let mut filtered = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                if terms.is_empty() && !entry.show_on_empty_query {
                    return None;
                }
                entry_match_score(entry, &terms).map(|score| (idx, score))
            })
            .collect::<Vec<_>>();

        filtered.sort_by_key(|(idx, score)| {
            let entry = &self.entries[*idx];
            (section_rank(entry.section), *score, &entry.label)
        });
        self.filtered = filtered.into_iter().map(|(idx, _)| idx).collect();
        if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
    }

    fn scope_hint_lines() -> Line<'static> {
        let hint = "scope: c:/cmd: , s:/skill: , t:/tool: , m:/mcp:";
        Line::from(Span::styled(
            hint,
            Style::default()
                .fg(palette::TEXT_DIM)
                .add_modifier(Modifier::ITALIC),
        ))
    }

    fn format_section_label(section: PaletteSection, count: usize) -> Line<'static> {
        let title = match section {
            PaletteSection::Action => "Actions",
            PaletteSection::Command => "Commands",
            PaletteSection::Skill => "Skills",
            PaletteSection::Tool => "Tools",
            PaletteSection::Mcp => "MCP",
        };
        Line::from(vec![Span::styled(
            format!("  {title} ({count})  "),
            Style::default()
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )])
    }

    fn scope_examples() -> Vec<Line<'static>> {
        vec![
            Line::from(Span::styled("Try:", Style::default().fg(palette::TEXT_DIM))),
            Line::from(Span::styled(
                "  c:<term>  Command-only   e.g. c:agent",
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                "  s:<term>  Skill-only     e.g. s:search",
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                "  t:<term>  Tool-only      e.g. t:git",
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                "  m:<term>  MCP-only       e.g. m:filesystem",
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ]
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1) as usize;
        self.selected = next;
    }

    fn selected_entry(&self) -> Option<&CommandPaletteEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|idx| self.entries.get(*idx))
    }

    /// Align keyboard focus with the row under the pointer (hover = select).
    fn focus_row_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.hit_map.borrow().row_at(column, row) else {
            return false;
        };
        if index < self.filtered.len() && self.selected != index {
            self.selected = index;
            return true;
        }
        index < self.filtered.len()
    }
}

impl ModalView for CommandPaletteView {
    fn kind(&self) -> ModalKind {
        ModalKind::CommandPalette
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            // Hover shares keyboard focus: the highlighted row is the same
            // one Enter would activate.
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                let _ = self.focus_row_at(mouse.column, mouse.row);
            }
            // Click selects the same row Enter would, and runs it
            // (pick-and-close — same as Enter on the command palette).
            MouseEventKind::Down(MouseButton::Left) => {
                if self.focus_row_at(mouse.column, mouse.row) {
                    if let Some(entry) = self.selected_entry() {
                        return ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
                            action: entry.action.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
        ViewAction::None
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Enter => {
                if let Some(entry) = self.selected_entry() {
                    ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
                        action: entry.action.clone(),
                    })
                } else {
                    ViewAction::None
                }
            }
            KeyCode::Up => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::Char('k') if self.query.is_empty() => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Char('j') if self.query.is_empty() => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.move_selection(-8);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.move_selection(8);
                ViewAction::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                ViewAction::None
            }
            // Ctrl+H is the legacy ASCII backspace many terminals emit.
            KeyCode::Char('h')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.query.pop();
                self.refilter();
                ViewAction::None
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.query.push(c);
                self.refilter();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Shared modal chrome (COH-05): centered opaque surface, title, footer.
        // Body paint (filter + list rows) stays local to this view.
        let title = format!(
            " {} — {} ",
            tr(self.locale, MessageId::CommandPaletteTitle),
            tr(self.locale, MessageId::CommandPaletteSubtitle)
        );
        let chrome = render_modal_chrome(
            area,
            buf,
            Line::from(Span::styled(
                title,
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )),
            90,
            22,
            44,
            8,
            &[
                ActionHint::new("↑/↓/j/k", "move"),
                ActionHint::new("Enter", "run"),
                ActionHint::new("Esc", "close"),
            ],
        );
        let popup_width = chrome.popup_area.width;
        let content = chrome.body;

        let mut lines = Vec::new();
        let query_label = if self.query.is_empty() {
            "Type to find and run one action".to_string()
        } else {
            format!("Filter: {}", self.query)
        };
        lines.push(Line::from(Span::styled(
            query_label,
            Style::default().fg(palette::TEXT_MUTED),
        )));
        let match_count = if self.query.is_empty() {
            format!(
                "{} shown / {} entries",
                self.filtered.len(),
                self.entries.len()
            )
        } else {
            format!("{} / {} matches", self.filtered.len(), self.entries.len())
        };
        lines.push(Line::from(Span::styled(
            match_count,
            Style::default().fg(palette::TEXT_DIM).italic(),
        )));
        lines.push(Self::scope_hint_lines());
        lines.extend(Self::scope_examples());
        lines.push(Line::from(""));

        // Rows the bordered popup can show for the list, minus the header that
        // was already pushed above. The entry loop additionally emits section
        // labels and separators, so the scroll window is sized against the real
        // rendered cost rather than a flat entry count (#2590).
        let header_lines = lines.len();
        let available = (content.height as usize).saturating_sub(header_lines);
        // Track the terminal y-coordinate of each entry row so mouse
        // hover/click can resolve to the same row keyboard focus uses.
        let mut hit_map = HitMap::new();
        let mut row_y = content
            .y
            .saturating_add(u16::try_from(header_lines).unwrap_or(u16::MAX));
        let mut action_count = 0usize;
        let mut command_count = 0usize;
        let mut skill_count = 0usize;
        let mut tool_count = 0usize;
        let mut mcp_count = 0usize;
        for idx in &self.filtered {
            match self.entries[*idx].section {
                PaletteSection::Action => action_count += 1,
                PaletteSection::Command => command_count += 1,
                PaletteSection::Skill => skill_count += 1,
                PaletteSection::Tool => tool_count += 1,
                PaletteSection::Mcp => mcp_count += 1,
            }
        }
        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "No matches.",
                Style::default().fg(palette::TEXT_MUTED).italic(),
            )));
        } else {
            let label_width = 24.min(popup_width.saturating_sub(26) as usize);
            let sections: Vec<PaletteSection> = self
                .filtered
                .iter()
                .map(|idx| self.entries[*idx].section)
                .collect();
            let (start, end) = visible_entry_window(&sections, self.selected, available);
            let mut active_section = None;
            for (slot, idx) in self.filtered[start..end].iter().enumerate() {
                let absolute = start + slot;
                let is_selected = absolute == self.selected;
                let entry = &self.entries[*idx];

                if active_section != Some(entry.section) {
                    if slot > 0 {
                        lines.push(Line::from(""));
                        row_y = row_y.saturating_add(1);
                    }
                    let count = match entry.section {
                        PaletteSection::Action => action_count,
                        PaletteSection::Command => command_count,
                        PaletteSection::Skill => skill_count,
                        PaletteSection::Tool => tool_count,
                        PaletteSection::Mcp => mcp_count,
                    };
                    lines.push(Self::format_section_label(entry.section, count));
                    row_y = row_y.saturating_add(1);
                    active_section = Some(entry.section);
                }

                let style = if is_selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                } else {
                    Style::default().fg(palette::TEXT_PRIMARY)
                };

                let mut line = format!("  {:<label_width$}", entry.label);
                let desc_capacity = popup_width as usize - (label_width + 4);
                let desc = if entry.description.width() > desc_capacity {
                    let mut shortened = String::new();
                    for ch in entry.description.chars() {
                        if shortened.width() >= desc_capacity.saturating_sub(3) {
                            break;
                        }
                        shortened.push(ch);
                    }
                    format!("{shortened}...")
                } else {
                    entry.description.clone()
                };
                if is_selected {
                    line = format!("> {:<label_width$}", entry.label);
                }
                line.push_str("  ");
                line.push_str(&desc);
                // Record hit region before painting so hover/click can find it.
                if row_y < content.y.saturating_add(content.height) {
                    hit_map.push_row(absolute, content.x, row_y, content.width);
                }
                lines.push(Line::from(Span::styled(line, style)));
                row_y = row_y.saturating_add(1);
            }
        }

        *self.hit_map.borrow_mut() = hit_map;
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(content, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn visible_window_keeps_selection_in_view_and_fits() {
        // Single large section, small budget: every selection must stay visible
        // and the rendered window must fit the available rows (#2590).
        let sections = vec![PaletteSection::Command; 30];
        let available = 10;
        for selected in 0..sections.len() {
            let (start, end) = visible_entry_window(&sections, selected, available);
            assert!(
                start <= selected && selected < end,
                "selected {selected} must lie within [{start}, {end})"
            );
            assert!(
                rendered_entry_rows(&sections, start, end) <= available,
                "window [{start}, {end}) must fit within {available} rows"
            );
        }
    }

    #[test]
    fn visible_window_scrolls_as_selection_advances() {
        let sections = vec![PaletteSection::Command; 30];
        let available = 8;
        let (start_near, _) = visible_entry_window(&sections, 0, available);
        assert_eq!(start_near, 0);
        // A far-down selection must advance the window start — the old code
        // left it pinned at 0 so the selection scrolled off-screen.
        let (start_far, end_far) = visible_entry_window(&sections, 25, available);
        assert!(start_far > 0, "window should scroll for a far selection");
        assert!(start_far <= 25 && 25 < end_far);
    }

    #[test]
    fn visible_window_accounts_for_section_overhead() {
        // Each entry is its own section, so each costs a label (plus a
        // separator after the first) on top of the entry row. Far fewer than
        // `available` entries fit, and the window must still respect the budget.
        let sections = vec![
            PaletteSection::Action,
            PaletteSection::Command,
            PaletteSection::Skill,
            PaletteSection::Tool,
            PaletteSection::Mcp,
        ];
        let available = 6;
        let (start, end) = visible_entry_window(&sections, 0, available);
        assert_eq!(start, 0);
        assert!(end >= 1, "at least the selected entry must render");
        assert!(rendered_entry_rows(&sections, start, end) <= available);
    }

    #[test]
    fn visible_window_handles_empty_and_zero_budget() {
        assert_eq!(visible_entry_window(&[], 0, 10), (0, 0));
        let sections = vec![PaletteSection::Command; 5];
        assert_eq!(visible_entry_window(&sections, 2, 0), (0, 0));
    }

    fn palette_entry(
        section: PaletteSection,
        label: &str,
        description: &str,
        command: &str,
    ) -> CommandPaletteEntry {
        CommandPaletteEntry {
            section,
            label: label.to_string(),
            description: description.to_string(),
            command: command.to_string(),
            action: CommandPaletteAction::InsertText {
                text: command.to_string(),
            },
            show_on_empty_query: true,
        }
    }

    #[test]
    fn command_palette_filters_with_section_shortcuts() {
        let entries = vec![
            palette_entry(PaletteSection::Command, "/mode", "mode command", "/mode"),
            palette_entry(
                PaletteSection::Skill,
                "skill:search",
                "search skill",
                "/skill search",
            ),
            palette_entry(PaletteSection::Tool, "tool:git", "git tool", "git"),
            palette_entry(
                PaletteSection::Tool,
                "tool:search",
                "search utility",
                "search",
            ),
            palette_entry(PaletteSection::Mcp, "mcp:fs", "filesystem", "mcp_fs_read"),
        ];
        let mut view = CommandPaletteView::new(entries);

        view.query = "c:mode".to_string();
        view.refilter();
        assert_eq!(view.filtered, vec![0]);

        view.query = "s:search".to_string();
        view.refilter();
        assert_eq!(view.filtered, vec![1]);

        view.query = "t:search".to_string();
        view.refilter();
        assert_eq!(view.filtered, vec![3]);

        view.query = "m:fs".to_string();
        view.refilter();
        assert_eq!(view.filtered, vec![4]);
    }

    #[test]
    fn command_palette_ranks_label_matches_before_description_matches() {
        let entries = vec![
            palette_entry(
                PaletteSection::Command,
                "/git",
                "status summary for repository",
                "git",
            ),
            palette_entry(
                PaletteSection::Command,
                "/config",
                "configure git settings",
                "config",
            ),
            palette_entry(
                PaletteSection::Command,
                "/sync",
                "sync repository state",
                "sync",
            ),
        ];
        let mut view = CommandPaletteView::new(entries);

        view.query = "git".to_string();
        view.refilter();

        assert_eq!(view.entries[view.filtered[0]].label, "/git");
        assert_eq!(view.entries[view.filtered[1]].label, "/config");
    }

    #[test]
    fn command_palette_supports_multiple_terms() {
        let entries = vec![
            palette_entry(
                PaletteSection::Command,
                "/search-code",
                "search with ripgrep",
                "search code",
            ),
            palette_entry(
                PaletteSection::Tool,
                "tool:search",
                "search web and files",
                "search",
            ),
            palette_entry(
                PaletteSection::Skill,
                "skill:search",
                "search files and docs",
                "/skill search",
            ),
        ];
        let mut view = CommandPaletteView::new(entries);

        view.query = "search code".to_string();
        view.refilter();
        assert_eq!(view.filtered.len(), 1);
        assert_eq!(view.entries[view.filtered[0]].label, "/search-code");

        view.query = "s:search".to_string();
        view.refilter();
        assert_eq!(view.filtered.len(), 1);
        assert_eq!(view.entries[view.filtered[0]].label, "skill:search");
    }

    #[test]
    fn command_palette_skills_use_workspace_and_configured_directories() {
        let tmp = TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let workspace_skill_dir = workspace
            .join(".agents")
            .join("skills")
            .join("workspace-skill");
        std::fs::create_dir_all(&workspace_skill_dir).expect("create workspace skill dir");
        std::fs::write(
            workspace_skill_dir.join("SKILL.md"),
            "---\nname: workspace-skill\ndescription: Workspace skill\ngithub: https://example.com\n---\nbody",
        )
        .expect("write workspace skill");

        let configured_dir = tmp.path().join("configured-skills");
        let configured_skill_dir = configured_dir.join("configured-skill");
        std::fs::create_dir_all(&configured_skill_dir).expect("create configured skill dir");
        std::fs::write(
            configured_skill_dir.join("SKILL.md"),
            "---\nname: configured-skill\ndescription: Configured skill\n---\nbody",
        )
        .expect("write configured skill");

        let entries = build_entries(
            Locale::En,
            configured_dir.as_path(),
            false,
            workspace.as_path(),
            Path::new("mcp.json"),
            None,
        );
        let skill_labels = entries
            .iter()
            .filter(|entry| entry.section == PaletteSection::Skill)
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>();

        assert!(skill_labels.contains(&"$workspace-skill"));
        assert!(skill_labels.contains(&"$configured-skill"));
    }

    #[test]
    fn command_palette_skills_respect_codewhale_only_scan() {
        let tmp = TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let claude_skill_dir = workspace
            .join(".claude")
            .join("skills")
            .join("claude-skill");
        std::fs::create_dir_all(&claude_skill_dir).expect("create claude skill dir");
        std::fs::write(
            claude_skill_dir.join("SKILL.md"),
            "---\nname: claude-skill\ndescription: Claude skill\n---\nbody",
        )
        .expect("write claude skill");
        let codewhale_skill_dir = workspace
            .join(".codewhale")
            .join("skills")
            .join("codewhale-skill");
        std::fs::create_dir_all(&codewhale_skill_dir).expect("create codewhale skill dir");
        std::fs::write(
            codewhale_skill_dir.join("SKILL.md"),
            "---\nname: codewhale-skill\ndescription: CodeWhale skill\n---\nbody",
        )
        .expect("write codewhale skill");

        let entries = build_entries(
            Locale::En,
            workspace.join(".codewhale").join("skills").as_path(),
            true,
            workspace.as_path(),
            Path::new("mcp.json"),
            None,
        );
        let skill_labels: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.section == PaletteSection::Skill)
            .map(|entry| entry.label.as_str())
            .collect();

        assert!(skill_labels.contains(&"$codewhale-skill"));
        assert!(!skill_labels.contains(&"$claude-skill"));
    }

    #[test]
    fn command_palette_command_entries_include_links_and_config_but_not_removed_commands() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
            false,
            Path::new("."),
            Path::new("mcp.json"),
            None,
        );
        let command_labels = entries
            .iter()
            .filter(|entry| entry.section == PaletteSection::Command)
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>();

        assert!(command_labels.contains(&"/config"));
        assert!(command_labels.contains(&"/links"));
        assert!(command_labels.contains(&"/voice"));
        assert!(!command_labels.contains(&"/set"));
        assert!(!command_labels.contains(&"/deepseek"));
    }

    #[test]
    fn command_palette_includes_workspace_user_commands() {
        let tmp = TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let commands_dir = workspace.join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).expect("create commands dir");
        std::fs::write(
            commands_dir.join("review.md"),
            "---\ndescription: Review with context\nargument-hint: <path>\n---\nReview $ARGUMENTS",
        )
        .expect("write user command");

        let entries = build_entries(
            Locale::En,
            tmp.path().join("skills").as_path(),
            false,
            workspace.as_path(),
            tmp.path().join("mcp.json").as_path(),
            None,
        );
        let user_entry = entries
            .iter()
            .find(|entry| entry.section == PaletteSection::Command && entry.label == "/review")
            .expect("user command should appear in command palette");

        assert!(user_entry.description.contains("Review with context"));
        assert!(user_entry.description.contains("<path>"));
        assert!(matches!(
            &user_entry.action,
            CommandPaletteAction::InsertText { text } if text == "/review "
        ));
    }

    #[test]
    fn command_palette_excludes_hidden_user_commands() {
        let tmp = TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let commands_dir = workspace.join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).expect("create commands dir");
        std::fs::write(
            commands_dir.join("secret.md"),
            "---\ndescription: Internal workflow\nhidden: true\n---\nsecret",
        )
        .expect("write hidden user command");

        let entries = build_entries(
            Locale::En,
            tmp.path().join("skills").as_path(),
            false,
            workspace.as_path(),
            tmp.path().join("mcp.json").as_path(),
            None,
        );

        assert!(
            !entries
                .iter()
                .any(|entry| entry.section == PaletteSection::Command && entry.label == "/secret")
        );
    }

    #[test]
    fn command_palette_filters_shadowed_builtin_aliases_from_description() {
        let tmp = TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        let commands_dir = workspace.join(".codewhale").join("commands");
        std::fs::create_dir_all(&commands_dir).expect("create commands dir");
        std::fs::write(
            commands_dir.join("image-review.md"),
            "---\ndescription: Review an image\nalias: image\n---\nreview image",
        )
        .expect("write user command");

        let entries = build_entries(
            Locale::En,
            tmp.path().join("skills").as_path(),
            false,
            workspace.as_path(),
            tmp.path().join("mcp.json").as_path(),
            None,
        );
        let attach = entries
            .iter()
            .find(|entry| entry.section == PaletteSection::Command && entry.label == "/attach")
            .expect("built-in canonical command should remain visible");

        assert!(
            !attach.description.contains("aliases: image")
                && !attach.description.contains(", image")
                && !attach.description.contains("image,"),
            "shadowed /image alias must not be advertised by /attach: {}",
            attach.description
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.section == PaletteSection::Command
                    && entry.label == "/image-review"),
            "user command that owns the /image alias should be visible"
        );
    }

    #[test]
    fn command_palette_has_one_entry_for_every_registered_command() {
        let tmp = TempDir::new().expect("tempdir");
        let skills_dir = tmp.path().join("skills");
        let mcp_config_path = tmp.path().join("mcp.json");
        let entries = build_entries(
            Locale::En,
            skills_dir.as_path(),
            false,
            tmp.path(),
            mcp_config_path.as_path(),
            None,
        );

        let command_entries = entries
            .iter()
            .filter(|entry| entry.section == PaletteSection::Command)
            .collect::<Vec<_>>();
        let user_registry = commands::user_registry::registry_for_workspace(Some(tmp.path()));
        let visible_user_commands = user_registry
            .iter()
            .filter(|command| !command.hidden)
            .count();
        let shadowed_builtins = commands::command_infos()
            .iter()
            .filter(|command| user_registry.get(command.name).is_some())
            .count();
        assert_eq!(
            command_entries.len(),
            commands::command_infos().len() - shadowed_builtins + visible_user_commands
        );

        for command in commands::command_infos() {
            if user_registry.get(command.name).is_some() {
                continue;
            }
            let label = format!("/{}", command.name);
            let matching = command_entries
                .iter()
                .filter(|entry| entry.label == label)
                .collect::<Vec<_>>();
            assert_eq!(
                matching.len(),
                1,
                "expected one palette entry for /{}",
                command.name
            );

            let entry = matching[0];
            assert_eq!(entry.command, command.palette_command());
            assert!(
                entry
                    .description
                    .contains(&*command.description_for(Locale::En)),
                "/{} palette description should include command help text",
                command.name
            );
            if command.requires_argument() {
                assert!(
                    entry.description.contains(command.usage),
                    "/{} palette description should include usage {:?}",
                    command.name,
                    command.usage
                );
            }
        }
    }

    #[test]
    fn command_palette_hides_toolbox_commands_until_searched() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
            false,
            Path::new("."),
            Path::new("mcp.json"),
            None,
        );
        let mut view = CommandPaletteView::new(entries);
        let root_labels = view
            .filtered
            .iter()
            .map(|idx| view.entries[*idx].label.as_str())
            .collect::<Vec<_>>();

        assert!(root_labels.contains(&"/provider"));
        assert!(root_labels.contains(&"/model"));
        assert!(root_labels.contains(&"/fleet"));
        assert!(root_labels.contains(&"/config"));
        assert!(root_labels.contains(&"/statusline"));
        assert!(!root_labels.contains(&"/rlm"));
        assert!(!root_labels.contains(&"/modeldb"));
        assert!(!root_labels.contains(&"/models"));
        assert!(!root_labels.contains(&"/subagents"));

        view.query = "rlm".to_string();
        view.refilter();
        assert!(
            view.filtered
                .iter()
                .any(|idx| view.entries[*idx].label == "/rlm"),
            "advanced /rlm should still be searchable"
        );
    }

    #[test]
    fn command_palette_runs_model_command_to_open_picker() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
            false,
            Path::new("."),
            Path::new("mcp.json"),
            None,
        );
        let model = entries
            .iter()
            .find(|entry| entry.section == PaletteSection::Command && entry.label == "/model")
            .expect("model command entry");

        assert_eq!(model.command, "/model ");
        assert!(matches!(
            &model.action,
            CommandPaletteAction::ExecuteCommand { command } if command == "/model"
        ));
    }

    #[test]
    fn command_palette_runs_change_without_requiring_version() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
            false,
            Path::new("."),
            Path::new("mcp.json"),
            None,
        );
        let change = entries
            .iter()
            .find(|entry| entry.section == PaletteSection::Command && entry.label == "/change")
            .expect("change command entry");

        assert!(matches!(
            &change.action,
            CommandPaletteAction::ExecuteCommand { command } if command == "/change"
        ));
    }

    #[test]
    fn command_palette_includes_mcp_discovery_and_failed_servers() {
        let snapshot = crate::mcp::McpManagerSnapshot {
            config_path: Path::new("mcp.json").to_path_buf(),
            config_exists: true,
            restart_required: false,
            servers: vec![
                crate::mcp::McpServerSnapshot {
                    name: "fs".to_string(),
                    enabled: true,
                    required: false,
                    transport: "stdio".to_string(),
                    command_or_url: "node server.js".to_string(),
                    connect_timeout: 10,
                    execute_timeout: 60,
                    read_timeout: 120,
                    connected: true,
                    error: None,
                    tools: vec![crate::mcp::McpDiscoveredItem {
                        name: "read".to_string(),
                        model_name: "mcp_fs_read".to_string(),
                        description: Some("Read files".to_string()),
                    }],
                    resources: Vec::new(),
                    prompts: Vec::new(),
                },
                crate::mcp::McpServerSnapshot {
                    name: "broken".to_string(),
                    enabled: true,
                    required: false,
                    transport: "http/sse".to_string(),
                    command_or_url: "https://example.invalid/mcp".to_string(),
                    connect_timeout: 10,
                    execute_timeout: 60,
                    read_timeout: 120,
                    connected: false,
                    error: Some("connect failed".to_string()),
                    tools: Vec::new(),
                    resources: Vec::new(),
                    prompts: Vec::new(),
                },
            ],
        };
        let entries = build_entries(
            Locale::En,
            Path::new("."),
            false,
            Path::new("."),
            Path::new("mcp.json"),
            Some(&snapshot),
        );

        assert!(entries.iter().any(|entry| entry.label == "mcp:manager"));
        assert!(entries.iter().any(|entry| entry.command == "mcp_fs_read"));
        let failed = entries
            .iter()
            .find(|entry| entry.label == "mcp:broken")
            .expect("failed server visible");
        assert!(failed.description.contains("failed"));

        // Verify the "use" insert entry for MCP tools
        let use_entry = entries
            .iter()
            .find(|entry| entry.label == "mcp:fs:tool:read > use")
            .expect("MCP tool use entry should exist");
        assert!(matches!(
            &use_entry.action,
            CommandPaletteAction::InsertText { text } if text == "mcp_fs_read"
        ));
        assert_eq!(use_entry.command, "mcp_fs_read");
    }

    #[test]
    fn command_palette_marks_disabled_servers_visibly() {
        // The healthy/failed cases are covered above; disabled was the
        // remaining gap from #197's acceptance list. Disabled servers must
        // appear in the palette with a `[disabled]` state tag so users can
        // see them without opening the MCP manager.
        let snapshot = crate::mcp::McpManagerSnapshot {
            config_path: Path::new("mcp.json").to_path_buf(),
            config_exists: true,
            restart_required: false,
            servers: vec![crate::mcp::McpServerSnapshot {
                name: "muted".to_string(),
                enabled: false,
                required: false,
                transport: "stdio".to_string(),
                command_or_url: "node disabled.js".to_string(),
                connect_timeout: 10,
                execute_timeout: 60,
                read_timeout: 120,
                connected: false,
                error: None,
                tools: Vec::new(),
                resources: Vec::new(),
                prompts: Vec::new(),
            }],
        };
        let entries = build_entries(
            Locale::En,
            Path::new("."),
            false,
            Path::new("."),
            Path::new("mcp.json"),
            Some(&snapshot),
        );

        let muted = entries
            .iter()
            .find(|entry| entry.label == "mcp:muted")
            .expect("disabled server should still appear in the palette");
        assert!(
            muted.description.contains("[disabled]"),
            "expected `[disabled]` state tag in description, got: {}",
            muted.description
        );
    }

    #[test]
    fn command_palette_emits_actions_not_raw_insertions() {
        let entries = vec![CommandPaletteEntry {
            section: PaletteSection::Command,
            label: "/config".to_string(),
            description: "open config".to_string(),
            command: "/config".to_string(),
            action: CommandPaletteAction::ExecuteCommand {
                command: "/config".to_string(),
            },
            show_on_empty_query: true,
        }];
        let mut view = CommandPaletteView::new(entries);

        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
                action: CommandPaletteAction::ExecuteCommand { .. }
            })
        ));
    }

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires every
    /// overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    fn sample_palette_view() -> CommandPaletteView {
        let entries = vec![
            palette_entry(PaletteSection::Command, "/config", "open config", "/config"),
            palette_entry(PaletteSection::Command, "/model", "choose model", "/model"),
            palette_entry(PaletteSection::Skill, "$search", "search skill", "$search"),
            palette_entry(PaletteSection::Tool, "tool:git", "git tool", "git"),
            palette_entry(PaletteSection::Mcp, "mcp:fs", "filesystem", "mcp_fs_read"),
        ];
        CommandPaletteView::new(entries)
    }

    #[test]
    fn command_palette_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(sample_palette_view());
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            let text = rows.join("\n");

            // Footer keeps every action.
            assert!(text.contains("move"), "{w}x{h}: missing 'move' hint");
            assert!(text.contains("run"), "{w}x{h}: missing 'run' hint");
            assert!(text.contains("close"), "{w}x{h}: missing 'close' hint");

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

    #[test]
    fn mouse_hover_and_click_share_keyboard_selected_row() {
        use crossterm::event::MouseButton;

        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        let mut view = sample_palette_view();
        // Default selected = 0 ("/config"). Paint so hit map is populated.
        view.render(area, &mut buf);
        assert!(
            !view.hit_map.borrow().is_empty(),
            "render must fill hit map"
        );

        // Find the "/model" row (index 1 in filtered) coordinates.
        let model_hit = view
            .hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|r| {
                matches!(
                    r.target,
                    crate::tui::hit_region::HitTarget::Row { index: 1 }
                )
            })
            .copied()
            .expect("model row hit region");
        let col = model_hit.rect.x + 2;
        let row = model_hit.rect.y;

        // Hover moves keyboard focus without activating.
        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.selected, 1, "hover should focus /model row");

        // Enter on the hovered row matches keyboard Enter path.
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected { .. })
        ));

        // Fresh view: click activates the row under the pointer (same as Enter).
        let mut view = sample_palette_view();
        view.render(area, &mut buf);
        // Find the "$search" skill row (index 2 in filtered).
        let skill_hit = view
            .hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|r| {
                matches!(
                    r.target,
                    crate::tui::hit_region::HitTarget::Row { index: 2 }
                )
            })
            .copied()
            .expect("skill row hit region");
        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: skill_hit.rect.x + 2,
            row: skill_hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected { .. })
        ));
        assert_eq!(view.selected, 2, "click should select $search skill row");

        // Clicking outside any row does nothing.
        let mut view = sample_palette_view();
        view.render(area, &mut buf);
        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(action, ViewAction::None));
    }
}
