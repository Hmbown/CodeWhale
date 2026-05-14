//! Command palette modal for quick command/skill insertion.

use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
};
use unicode_width::UnicodeWidthStr;

use crate::commands;
use crate::localization::Locale;
use crate::palette;
use crate::skills::SkillRegistry;
use crate::tools::spec::ApprovalRequirement;
use crate::tools::spec::ToolCapability;
use crate::tools::{ToolContext, ToolRegistryBuilder};
use crate::tui::views::{CommandPaletteAction, ModalKind, ModalView, ViewAction, ViewEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PaletteSection {
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
}

pub struct CommandPaletteView {
    entries: Vec<CommandPaletteEntry>,
    filtered: Vec<usize>,
    query: String,
    selected: usize,
}

pub fn build_entries(
    locale: Locale,
    skills_dir: &Path,
    workspace: &Path,
    mcp_config_path: &Path,
    mcp_snapshot: Option<&crate::mcp::McpManagerSnapshot>,
) -> Vec<CommandPaletteEntry> {
    let mut entries = Vec::new();

    for command in commands::COMMANDS {
        let mut description = command.palette_description_for(locale);
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
        });
    }

    let skills = SkillRegistry::discover(skills_dir);
    for skill in skills.list() {
        entries.push(CommandPaletteEntry {
            section: PaletteSection::Skill,
            label: format!("skill:{}", skill.name),
            description: skill.description.clone(),
            command: format!("/skill {}", skill.name),
            action: CommandPaletteAction::ExecuteCommand {
                command: format!("/skill {}", skill.name),
            },
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
            })
        })
        .collect::<Vec<_>>();
    tool_entries.sort_by(|a, b| a.label.cmp(&b.label));
    entries.extend(tool_entries);

    entries.extend(build_mcp_entries(mcp_config_path, mcp_snapshot));

    entries.sort_by(|a, b| a.label.cmp(&b.label));
    entries.sort_by_key(|entry| entry.section);
    entries
}

fn build_mcp_entries(
    mcp_config_path: &Path,
    mcp_snapshot: Option<&crate::mcp::McpManagerSnapshot>,
) -> Vec<CommandPaletteEntry> {
    let owned_snapshot = if mcp_snapshot.is_none() {
        crate::mcp::manager_snapshot_from_config(mcp_config_path, false).ok()
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
                            .map_or(String::new(), |desc| format!(" ({})", desc))
                    ),
                    command: tool.model_name.clone(),
                    action: CommandPaletteAction::InsertText {
                        text: tool.model_name.clone(),
                    },
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

fn modal_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .padding(Padding::uniform(1))
}

fn parse_section_term(term: &str) -> Option<(PaletteSection, String)> {
    let (section, query) = term.split_once(':')?;

    if section.is_empty() || query.is_empty() {
        return None;
    }

    let query = query.to_ascii_lowercase();
    let section = match section {
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
        PaletteSection::Command => "command",
        PaletteSection::Skill => "skill",
        PaletteSection::Tool => "tool",
        PaletteSection::Mcp => "mcp",
    }
}

fn section_rank(section: PaletteSection) -> usize {
    match section {
        PaletteSection::Command => 0,
        PaletteSection::Skill => 1,
        PaletteSection::Tool => 2,
        PaletteSection::Mcp => 3,
    }
}

fn command_runs_directly(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "clear"
            | "exit"
            | "models"
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

impl CommandPaletteView {
    pub fn new(entries: Vec<CommandPaletteEntry>) -> Self {
        let mut view = Self {
            entries,
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
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
            .filter_map(|(idx, entry)| entry_match_score(entry, &terms).map(|score| (idx, score)))
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
            PaletteSection::Command => "Commands",
            PaletteSection::Skill => "Skills",
            PaletteSection::Tool => "Tools",
            PaletteSection::Mcp => "MCP",
        };
        Line::from(vec![Span::styled(
            format!("  {title} ({count})  "),
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
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
}

impl ModalView for CommandPaletteView {
    fn kind(&self) -> ModalKind {
        ModalKind::CommandPalette
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
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
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
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
        let popup_width = 90.min(area.width.saturating_sub(4));
        let popup_height = 22.min(area.height.saturating_sub(4));
        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let mut lines = Vec::new();
        let query_label = if self.query.is_empty() {
            "Type to filter".to_string()
        } else {
            format!("Filter: {}", self.query)
        };
        lines.push(Line::from(Span::styled(
            query_label,
            Style::default().fg(palette::TEXT_MUTED),
        )));
        let match_count = if self.query.is_empty() {
            format!("{} entries", self.entries.len())
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

        let visible = popup_height.saturating_sub(7) as usize;
        let mut command_count = 0usize;
        let mut skill_count = 0usize;
        let mut tool_count = 0usize;
        let mut mcp_count = 0usize;
        for idx in &self.filtered {
            match self.entries[*idx].section {
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
            let start = self.selected.saturating_sub(visible.saturating_sub(1));
            let end = (start + visible).min(self.filtered.len());
            let mut active_section = None;
            for (slot, idx) in self.filtered[start..end].iter().enumerate() {
                let absolute = start + slot;
                let is_selected = absolute == self.selected;
                let entry = &self.entries[*idx];

                if active_section != Some(entry.section) {
                    if slot > 0 {
                        lines.push(Line::from(""));
                    }
                    let count = match entry.section {
                        PaletteSection::Command => command_count,
                        PaletteSection::Skill => skill_count,
                        PaletteSection::Tool => tool_count,
                        PaletteSection::Mcp => mcp_count,
                    };
                    lines.push(Self::format_section_label(entry.section, count));
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
                lines.push(Line::from(Span::styled(line, style)));
            }
        }

        let block = modal_block()
            .title(" Command Palette ")
            .title_bottom(Line::from(vec![
                Span::styled(" ↑/↓/j/k move  ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled("Enter run/open  ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled("Esc close", Style::default().fg(palette::TEXT_MUTED)),
            ]));

        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .render(popup_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
    fn command_palette_command_entries_include_links_and_config_but_not_removed_commands() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
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
        assert!(!command_labels.contains(&"/set"));
        assert!(!command_labels.contains(&"/deepseek"));
    }

    #[test]
    fn command_palette_inserts_model_command_for_argument_entry() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
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
            CommandPaletteAction::InsertText { text } if text == "/model "
        ));
    }

    #[test]
    fn command_palette_runs_change_without_requiring_version() {
        let entries = build_entries(
            Locale::En,
            Path::new("."),
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
}
