//! The TUI's user-facing operating mode. Lives in codewhale-config so
//! settings, receipts, and other crates can name it without depending on
//! the TUI; the TUI adds the localized picker strings through an extension
//! trait.

/// Supported application modes for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Agent,
    #[allow(dead_code)]
    Auto,
    /// Legacy compatibility alias; resolves to [`Self::Agent`] + bypass approvals.
    Yolo,
    Plan,
    Operate,
}

impl AppMode {
    /// Productive keyboard cycle: Plan -> Act -> Operate -> Plan.
    ///
    /// `Auto` remains an internal variant while the real implementation is
    /// redesigned; do not expose it through user-facing mode selection (#3733).
    /// `Yolo` is kept for parse/back-compat only and is not in the Tab cycle.
    /// Operate joins the visible cycle because ordinary messages can now
    /// coordinate background workers without requiring a Workflow definition.
    pub const CYCLE: [Self; 3] = [Self::Plan, Self::Agent, Self::Operate];

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "act" | "work" | "auto" | "1" => Some(Self::Agent),
            "plan" | "2" => Some(Self::Plan),
            "operate" | "operation" | "ops" | "3" => Some(Self::Operate),
            // Invisible one-way permission shorthand only — never a visible mode.
            "yolo" | "4" | "bypass" | "bypass-permissions" | "bypasspermissions" => {
                Some(Self::Yolo)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        // Unreleased Multitask never shipped; normalize leftover settings to Operate.
        match value.trim().to_ascii_lowercase().as_str() {
            "multitask" | "multi" | "5" => Self::Operate,
            other => Self::parse(other).unwrap_or(Self::Agent),
        }
    }

    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Auto => "agent",
            // Write current permission vocabulary, not the legacy YOLO label.
            Self::Yolo => "agent",
            Self::Plan => "plan",
            Self::Operate => "operate",
        }
    }

    /// Short label used in the UI footer.
    pub fn label(self) -> &'static str {
        match self {
            AppMode::Agent => "ACT",
            AppMode::Auto => "ACT",
            AppMode::Yolo => "ACT",
            AppMode::Plan => "PLAN",
            AppMode::Operate => "OPERATE",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            AppMode::Agent => "Act",
            AppMode::Auto => "Act",
            AppMode::Yolo => "Act",
            AppMode::Plan => "Plan",
            AppMode::Operate => "Operate",
        }
    }

    #[must_use]
    pub fn number(self) -> char {
        match self {
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => '1',
            AppMode::Plan => '2',
            AppMode::Operate => '3',
        }
    }

    #[must_use]
    pub fn uses_agent_baseline(self) -> bool {
        matches!(self, Self::Agent | Self::Auto | Self::Operate)
    }

    /// Operate gets a higher parallel launch floor so background fan-out is
    /// not throttled to a single slot when config is low.
    #[must_use]
    pub fn mode_delegation_launch_floor(self) -> usize {
        match self {
            Self::Operate => 4,
            _ => 1,
        }
    }

    #[allow(dead_code)]
    /// Description shown in help or onboarding text.
    pub fn description(self) -> &'static str {
        match self {
            AppMode::Agent | AppMode::Auto => {
                "Act mode - direct work in the current session with tools"
            }
            AppMode::Yolo => "Act mode with Full Access (legacy compatibility setting)",
            AppMode::Plan => "Plan mode - research and design before implementing",
            AppMode::Operate => "Operate mode - send tasks while Fleet workers run in parallel",
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        let Some(index) = Self::CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Agent;
        };
        Self::CYCLE[(index + 1) % Self::CYCLE.len()]
    }

    #[must_use]
    pub fn previous(self) -> Self {
        let Some(index) = Self::CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Agent;
        };
        Self::CYCLE[(index + Self::CYCLE.len() - 1) % Self::CYCLE.len()]
    }
}
