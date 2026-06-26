//! Setup summary step for the configuration wizard.
//!
//! Displays configured MCP servers, skills, and plugin state
//! as a read-only overview before offering safe bootstrap paths.
//!
//! Related: #3407

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction};

#[derive(Debug, Clone)]
pub struct SetupSummaryData {
    pub mcp_servers: Vec<McpServerInfo>,
    pub mcp_config_path: Option<String>,
    pub skills_dirs: Vec<String>,
    pub skills_installed: usize,
    pub plugin_dir: Option<String>,
    pub plugin_available: bool,
}

#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub enabled: bool,
    pub state: String,
}

pub struct SetupSummaryView {
    data: SetupSummaryData,
    scroll: u16,
}

impl SetupSummaryView {
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let data = Self::collect(config);
        Self { data, scroll: 0 }
    }

    fn collect(config: &Config) -> SetupSummaryData {
        let mcp_path = config.mcp_config_path();
        let mcp_config_path = Some(mcp_path.to_string_lossy().to_string());
        let mcp_servers = if mcp_path.exists() {
            crate::mcp::load_config(&mcp_path)
                .ok()
                .map(|cfg| {
                    cfg.servers.iter().map(|(name, svr)| {
                        let state = if svr.is_enabled() { "enabled" } else { "disabled" };
                        McpServerInfo {
                            name: name.clone(),
                            enabled: svr.is_enabled(),
                            state: state.to_string(),
                        }
                    }).collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let skills_dir = crate::skills::default_skills_dir();
        let skills_dirs = vec![skills_dir.to_string_lossy().to_string()];
        let skills_installed = if skills_dir.exists() {
            crate::skills::SkillRegistry::discover(&skills_dir).len()
        } else {
            0
        };

        SetupSummaryData {
            mcp_servers,
            mcp_config_path,
            skills_dirs,
            skills_installed,
            plugin_dir: None,
            plugin_available: false,
        }
    }
                        }).collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            }))
            .unwrap_or_default();

        let skills_dir = crate::skills::default_skills_dir();
        let skills_dirs = vec![skills_dir.to_string_lossy().to_string()];
        let skills_installed = if skills_dir.exists() {
            crate::skills::SkillRegistry::discover(&skills_dir).len()
        } else {
            0
        };

        SetupSummaryData {
            mcp_servers,
            mcp_config_path: config.mcp_config_path(),
            skills_dirs,
            skills_installed,
            plugin_dir: app.plugin_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
            plugin_available: app.plugin_dir.as_ref().is_some_and(|d| d.exists()),
        }
    }
}

impl ModalView for SetupSummaryView {
    fn kind(&self) -> ModalKind {
        ModalKind::SetupSummary
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 80.min(area.width.saturating_sub(4)).max(50);
        let popup_height = area.height.saturating_sub(2);
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + 1,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Setup Summary ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Up/Down ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("scroll "),
                Span::styled(" Esc/q ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("close"),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines: Vec<Line> = Vec::new();

        // Section: MCP
        lines.push(Line::from(Span::styled(
            " MCP Servers ",
            Style::default()
                .fg(palette::ACCENT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            format!(
                " Config: {}",
                self.data.mcp_config_path.as_deref().unwrap_or("not found")
            ),
            Style::default().fg(palette::TEXT_MUTED),
        )));

        if self.data.mcp_servers.is_empty() {
            lines.push(Line::from(Span::styled(
                "   No MCP servers configured.",
                Style::default().fg(palette::TEXT_MUTED),
            )));
        } else {
            for server in &self.data.mcp_servers {
                let status = if server.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(
                            " {} ",
                            if server.enabled {
                                "\u{25cf}"
                            } else {
                                "\u{25cb}"
                            }
                        ),
                        if server.enabled {
                            Style::default().fg(palette::SURFACE_SUCCESS)
                        } else {
                            Style::default().fg(palette::TEXT_MUTED)
                        },
                    ),
                    Span::styled(&server.name, Style::default().fg(palette::TEXT_PRIMARY)),
                    Span::raw(" "),
                    Span::styled(
                        format!("[{status}]"),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("({})", server.state),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                ]));
            }
        }

        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            " Skills ",
            Style::default()
                .fg(palette::ACCENT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )));
        if self.data.skills_dirs.is_empty() {
            lines.push(Line::from(Span::styled(
                "   No skills directories configured.",
                Style::default().fg(palette::TEXT_MUTED),
            )));
        } else {
            for dir in &self.data.skills_dirs {
                lines.push(Line::from(vec![
                    Span::styled(" Dir: ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::styled(dir, Style::default().fg(palette::TEXT_PRIMARY)),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled(" Installed: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(
                    self.data.skills_installed.to_string(),
                    Style::default().fg(palette::TEXT_PRIMARY),
                ),
            ]));
        }

        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            " Plugins ",
            Style::default()
                .fg(palette::ACCENT_PRIMARY)
                .add_modifier(Modifier::BOLD),
        )));
        match &self.data.plugin_dir {
            Some(dir) => {
                lines.push(Line::from(vec![
                    Span::styled(" Dir: ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::styled(dir, Style::default().fg(palette::TEXT_PRIMARY)),
                ]));
                if self.data.plugin_available {
                    lines.push(Line::from(Span::styled(
                        "  Plugins directory exists",
                        Style::default().fg(palette::SURFACE_SUCCESS),
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        "  Plugins directory not found",
                        Style::default().fg(palette::SURFACE_ERROR),
                    )));
                }
            }
            None => {
                lines.push(Line::from(Span::styled(
                    "   Plugin support not configured.",
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }
        }

        let paragraph = Paragraph::new(lines).scroll((self.scroll, 0));
        paragraph.render(inner, buf);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_setup_summary() {
        let data = SetupSummaryData {
            mcp_servers: vec![],
            mcp_config_path: None,
            skills_dirs: vec![],
            skills_installed: 0,
            plugin_dir: None,
            plugin_available: false,
        };
        assert!(data.mcp_servers.is_empty());
        assert_eq!(data.skills_installed, 0);
        assert!(!data.plugin_available);
    }

    #[test]
    fn mcp_server_info_construction() {
        let info = McpServerInfo {
            name: "test-server".into(),
            enabled: true,
            state: "connected".into(),
        };
        assert_eq!(info.name, "test-server");
        assert!(info.enabled);
        assert_eq!(info.state, "connected");
    }

    #[test]
    fn mcp_server_disabled() {
        let info = McpServerInfo {
            name: "disabled-server".into(),
            enabled: false,
            state: "disconnected".into(),
        };
        assert!(!info.enabled);
    }

    #[test]
    fn setup_summary_data_with_mcp() {
        let data = SetupSummaryData {
            mcp_servers: vec![
                McpServerInfo { name: "srv-a".into(), enabled: true, state: "connected".into() },
                McpServerInfo { name: "srv-b".into(), enabled: false, state: "disconnected".into() },
            ],
            mcp_config_path: Some("/tmp/mcp.json".into()),
            skills_dirs: vec!["/tmp/skills".into()],
            skills_installed: 5,
            plugin_dir: Some("/tmp/plugins".into()),
            plugin_available: true,
        };
        assert_eq!(data.mcp_servers.len(), 2);
        assert_eq!(data.skills_installed, 5);
        assert!(data.plugin_available);
    }
}
