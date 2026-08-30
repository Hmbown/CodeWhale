//! Session-page MCP + plugin boot surface.
//!
//! Plugin discovery and every enabled MCP server boot as a **set**, not a
//! toast per name. The Tideline footer carries the compact pulse
//! (`MCP · 4 connecting`); detailed diagnosis and actions belong in `/mcp`,
//! never as multi-row boot output between the transcript and composer.

use unicode_width::UnicodeWidthStr;

use crate::localization::{Locale, MessageId, tr};
use crate::mcp::{McpManagerSnapshot, McpServerSnapshot};
use crate::plugins::PluginRegistry;
use crate::plugins::types::{PluginDiagnosticLevel, PluginTrustStatus};
use crate::tui::app::App;

const ITEM_SEPARATOR: &str = " · ";
const MAX_NAMED_CHIPS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBootPhase {
    Hidden,
    Booting,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerBootState {
    Connecting,
    Connected,
    Failed,
    NeedsLogin,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerBootRow {
    pub name: String,
    pub state: McpServerBootState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PluginBootSummary {
    pub loaded: usize,
    pub invalid: usize,
    pub duplicate: usize,
    pub needs_setup: usize,
}

impl PluginBootSummary {
    #[must_use]
    pub fn is_quiet(self) -> bool {
        self.loaded == 0 && self.invalid == 0 && self.duplicate == 0 && self.needs_setup == 0
    }

    #[must_use]
    pub fn from_registry(registry: &PluginRegistry) -> Self {
        let loaded = registry.list().len();
        let mut invalid = 0usize;
        let mut duplicate = 0usize;
        let mut needs_setup = 0usize;
        for diagnostic in registry.diagnostics() {
            match diagnostic.code {
                "duplicate-root" | "name-conflict" => duplicate += 1,
                _ if diagnostic.level == PluginDiagnosticLevel::Error => invalid += 1,
                _ => {}
            }
        }
        for plugin in registry.list() {
            if plugin
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == PluginDiagnosticLevel::Error)
            {
                invalid += 1;
            } else if matches!(
                plugin.trust_status,
                PluginTrustStatus::NeverReviewed | PluginTrustStatus::CapabilitiesChanged
            ) {
                needs_setup += 1;
            }
        }
        Self {
            loaded,
            invalid,
            duplicate,
            needs_setup,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBootSurface {
    pub phase: SessionBootPhase,
    pub servers: Vec<McpServerBootRow>,
    pub plugins: PluginBootSummary,
    /// Enabled-server count used when names have not arrived yet, so the
    /// first frame can still say `MCP · N connecting` instead of hiding.
    unnamed_connecting: usize,
}

impl SessionBootSurface {
    #[must_use]
    pub fn from_app(app: &App) -> Self {
        Self::from_parts(
            app.mcp_snapshot.as_ref(),
            app.mcp_initializing,
            &app.mcp_connecting,
            app.mcp_configured_count,
            PluginBootSummary::from_registry(app.plugin_registry.as_ref()),
        )
    }

    #[must_use]
    pub fn from_parts(
        snapshot: Option<&McpManagerSnapshot>,
        initializing: bool,
        connecting: &[String],
        configured_count: usize,
        plugins: PluginBootSummary,
    ) -> Self {
        let servers = if let Some(snapshot) = snapshot {
            snapshot
                .servers
                .iter()
                .map(|server| row_from_snapshot(server, initializing, connecting))
                .collect()
        } else if initializing {
            let mut names = connecting.to_vec();
            names.sort();
            names
                .into_iter()
                .map(|name| McpServerBootRow {
                    name,
                    state: McpServerBootState::Connecting,
                })
                .collect()
        } else {
            Vec::new()
        };

        let connecting_count = servers
            .iter()
            .filter(|row| row.state == McpServerBootState::Connecting)
            .count();
        let unnamed_connecting = if connecting_count == 0 && initializing {
            configured_count
        } else {
            0
        };
        let phase = if servers.is_empty() && plugins.is_quiet() && unnamed_connecting == 0 {
            SessionBootPhase::Hidden
        } else if initializing || connecting_count > 0 || unnamed_connecting > 0 {
            SessionBootPhase::Booting
        } else {
            SessionBootPhase::Settled
        };

        Self {
            phase,
            servers,
            plugins,
            unnamed_connecting,
        }
    }

    #[must_use]
    pub fn activity_chip(&self, locale: Locale, budget: usize) -> Option<String> {
        if self.phase == SessionBootPhase::Hidden || budget == 0 {
            return None;
        }
        let connecting: Vec<&str> = self
            .servers
            .iter()
            .filter(|row| row.state == McpServerBootState::Connecting)
            .map(|row| row.name.as_str())
            .collect();
        let failed = self
            .servers
            .iter()
            .filter(|row| {
                matches!(
                    row.state,
                    McpServerBootState::Failed | McpServerBootState::NeedsLogin
                )
            })
            .count();
        let connected = self
            .servers
            .iter()
            .filter(|row| row.state == McpServerBootState::Connected)
            .count();

        let mut candidates = Vec::new();
        if !connecting.is_empty() {
            let count = connecting.len();
            let named = named_chip_line("MCP", count, "connecting", &connecting);
            candidates.push(named);
            candidates.push(format!("MCP{ITEM_SEPARATOR}{count} connecting"));
        } else if failed > 0 {
            candidates.push(format!(
                "MCP{ITEM_SEPARATOR}{connected} {}{ITEM_SEPARATOR}{failed} {}",
                tr(locale, MessageId::ExtensionsStateConnected),
                tr(locale, MessageId::PhaseFailed)
            ));
            candidates.push(format!("MCP{ITEM_SEPARATOR}{failed} failed"));
        } else if self.phase == SessionBootPhase::Booting {
            let count = self.servers.len().max(self.unnamed_connecting);
            if count > 0 {
                candidates.push(format!("MCP{ITEM_SEPARATOR}{count} connecting"));
            }
        }

        candidates.into_iter().find(|line| line.width() <= budget)
    }
}

fn row_from_snapshot(
    server: &McpServerSnapshot,
    initializing: bool,
    connecting: &[String],
) -> McpServerBootRow {
    if !server.enabled {
        return McpServerBootRow {
            name: server.name.clone(),
            state: McpServerBootState::Disabled,
        };
    }
    if server.connected {
        return McpServerBootRow {
            name: server.name.clone(),
            state: McpServerBootState::Connected,
        };
    }
    if let Some(error) = server.error.as_deref() {
        if mcp_error_requires_login(error) {
            return McpServerBootRow {
                name: server.name.clone(),
                state: McpServerBootState::NeedsLogin,
            };
        }
        return McpServerBootRow {
            name: server.name.clone(),
            state: McpServerBootState::Failed,
        };
    }
    let connecting_now = initializing || connecting.iter().any(|name| name == &server.name);
    McpServerBootRow {
        name: server.name.clone(),
        state: if connecting_now {
            McpServerBootState::Connecting
        } else {
            McpServerBootState::Failed
        },
    }
}

#[must_use]
pub fn mcp_error_requires_login(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("mcp login")
        || error.contains("auth required")
        || (error.contains("oauth") && error.contains("authenticat"))
}

fn named_chip_line(kind: &str, count: usize, verb: &str, names: &[&str]) -> String {
    let chips = names
        .iter()
        .take(MAX_NAMED_CHIPS)
        .copied()
        .collect::<Vec<_>>();
    let extra = names.len().saturating_sub(chips.len());
    let mut line = format!("{kind}{ITEM_SEPARATOR}{count} {verb}");
    if !chips.is_empty() {
        line.push_str(ITEM_SEPARATOR);
        line.push_str(&chips.join(ITEM_SEPARATOR));
        if extra > 0 {
            line.push_str(&format!("{ITEM_SEPARATOR}+{extra}"));
        }
    }
    line
}

/// Activity-strip chip for the current session boot set.
#[must_use]
pub fn activity_chip(app: &App, budget: usize) -> Option<String> {
    SessionBootSurface::from_app(app).activity_chip(app.ui_locale, budget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{McpManagerSnapshot, McpServerCapabilityMetadata, McpServerSnapshot};
    use std::path::PathBuf;

    fn server(
        name: &str,
        enabled: bool,
        connected: bool,
        error: Option<&str>,
    ) -> McpServerSnapshot {
        McpServerSnapshot {
            name: name.to_string(),
            enabled,
            required: false,
            transport: "stdio".to_string(),
            command_or_url: format!("cmd-{name}"),
            connect_timeout: 5,
            execute_timeout: 5,
            read_timeout: 5,
            connected,
            error: error.map(str::to_string),
            capability_metadata: McpServerCapabilityMetadata::NotObserved,
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
        }
    }

    fn snapshot(servers: Vec<McpServerSnapshot>) -> McpManagerSnapshot {
        McpManagerSnapshot {
            config_path: PathBuf::from("mcp.json"),
            config_exists: true,
            reload_required: false,
            servers,
        }
    }

    #[test]
    fn zero_servers_and_quiet_plugins_hide() {
        let surface =
            SessionBootSurface::from_parts(None, false, &[], 0, PluginBootSummary::default());
        assert_eq!(surface.phase, SessionBootPhase::Hidden);
        assert!(surface.activity_chip(Locale::En, 80).is_none());
    }

    #[test]
    fn one_connecting_server_names_itself() {
        let snap = snapshot(vec![server("alpha", true, false, None)]);
        let surface = SessionBootSurface::from_parts(
            Some(&snap),
            true,
            &["alpha".to_string()],
            1,
            PluginBootSummary::default(),
        );
        assert_eq!(surface.phase, SessionBootPhase::Booting);
        assert_eq!(surface.servers.len(), 1);
        assert_eq!(surface.servers[0].state, McpServerBootState::Connecting);
        let chip = surface.activity_chip(Locale::En, 80).expect("chip");
        assert!(chip.contains("alpha"), "{chip}");
        assert!(!chip.to_ascii_lowercase().contains("slack"), "{chip}");
    }

    #[test]
    fn many_connecting_servers_use_count_and_named_chips() {
        let snap = snapshot(vec![
            server("alpha", true, false, None),
            server("beta", true, false, None),
            server("gamma", true, false, None),
            server("docs", true, false, None),
        ]);
        let connecting = ["alpha", "beta", "gamma", "docs"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let surface = SessionBootSurface::from_parts(
            Some(&snap),
            true,
            &connecting,
            4,
            PluginBootSummary::default(),
        );
        assert_eq!(surface.phase, SessionBootPhase::Booting);
        let chip = surface.activity_chip(Locale::En, 80).expect("chip");
        assert!(chip.contains("4 connecting"), "{chip}");
        assert!(chip.contains("alpha"), "{chip}");
        assert!(chip.contains("docs"), "{chip}");
        assert!(!chip.to_ascii_lowercase().contains("slack"), "{chip}");
    }

    #[test]
    fn settled_failures_remain_classified_for_the_footer_chip() {
        let snap = snapshot(vec![
            server("alpha", true, true, None),
            server("beta", true, false, Some("protocol negotiation timed out")),
            server(
                "gamma",
                true,
                false,
                Some("MCP server 'gamma' requires OAuth authentication. Run `/mcp login gamma`"),
            ),
            server("docs", false, false, Some("disabled")),
        ]);
        let surface = SessionBootSurface::from_parts(
            Some(&snap),
            false,
            &[],
            4,
            PluginBootSummary::default(),
        );
        assert_eq!(surface.phase, SessionBootPhase::Settled);
        assert_eq!(
            surface
                .servers
                .iter()
                .find(|row| row.name == "beta")
                .map(|row| row.state),
            Some(McpServerBootState::Failed)
        );
        assert_eq!(
            surface
                .servers
                .iter()
                .find(|row| row.name == "gamma")
                .map(|row| row.state),
            Some(McpServerBootState::NeedsLogin)
        );
    }

    #[test]
    fn narrow_activity_budget_sheds_names_keeps_count() {
        let snap = snapshot(vec![
            server("alpha", true, false, None),
            server("beta", true, false, None),
            server("gamma", true, false, None),
        ]);
        let connecting = ["alpha", "beta", "gamma"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let surface = SessionBootSurface::from_parts(
            Some(&snap),
            true,
            &connecting,
            3,
            PluginBootSummary::default(),
        );
        let chip = surface.activity_chip(Locale::En, 22).expect("chip");
        assert_eq!(chip, "MCP · 3 connecting");
    }

    #[test]
    fn first_frame_names_enabled_servers_before_a_snapshot_arrives() {
        let connecting = ["gamma", "alpha", "docs"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let surface = SessionBootSurface::from_parts(
            None,
            true,
            &connecting,
            3,
            PluginBootSummary::default(),
        );
        assert_eq!(surface.phase, SessionBootPhase::Booting);
        assert_eq!(
            surface
                .servers
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "docs", "gamma"]
        );
        let chip = surface.activity_chip(Locale::En, 80).expect("chip");
        assert!(chip.contains("3 connecting"), "{chip}");
        assert!(chip.contains("alpha"), "{chip}");
        assert!(chip.contains("gamma"), "{chip}");
        assert!(!chip.to_ascii_lowercase().contains("slack"), "{chip}");
    }

    #[test]
    fn initializing_without_names_still_shows_the_count() {
        let surface =
            SessionBootSurface::from_parts(None, true, &[], 4, PluginBootSummary::default());
        assert_eq!(surface.phase, SessionBootPhase::Booting);
        assert!(surface.servers.is_empty());
        assert_eq!(
            surface.activity_chip(Locale::En, 80).as_deref(),
            Some("MCP · 4 connecting")
        );
    }
}
