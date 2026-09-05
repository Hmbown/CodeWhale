//! The TUI's user-facing operating mode. Lives in codewhale-config so
//! settings, receipts, and other crates can name it without depending on
//! the TUI; the TUI adds the localized picker strings through an extension
//! trait.

/// Supported application modes for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Agent,
    Plan,
    Operate,
}

impl AppMode {
    /// Productive keyboard cycle: Plan -> Act -> Operate -> Plan.
    ///
    /// Operate joins the visible cycle as the always-on fleet operation:
    /// a lead plans slices, then workers execute against an optional burn rate.
    pub const CYCLE: [Self; 3] = [Self::Plan, Self::Agent, Self::Operate];

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "act" | "work" | "auto" | "1" => Some(Self::Agent),
            "plan" | "2" => Some(Self::Plan),
            "operate" | "operation" | "ops" | "3" => Some(Self::Operate),
            // Invisible one-way permission shorthand only — never a visible
            // mode. These spellings resolve to Act; the bypass posture they
            // imply is carried by the permission surface (settings load,
            // CLI/runtime wire), not by a mode.
            "yolo" | "4" | "bypass" | "bypass-permissions" | "bypasspermissions" => {
                Some(Self::Agent)
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
            Self::Plan => "plan",
            Self::Operate => "operate",
        }
    }

    /// Short label used in the UI footer.
    pub fn label(self) -> &'static str {
        match self {
            AppMode::Agent => "ACT",
            AppMode::Plan => "PLAN",
            AppMode::Operate => "OPERATE",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            AppMode::Agent => "Act",
            AppMode::Plan => "Plan",
            AppMode::Operate => "Operate",
        }
    }

    #[must_use]
    pub fn number(self) -> char {
        match self {
            AppMode::Agent => '1',
            AppMode::Plan => '2',
            AppMode::Operate => '3',
        }
    }

    #[must_use]
    pub fn uses_agent_baseline(self) -> bool {
        matches!(self, Self::Agent | Self::Operate)
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

    /// Description shown in help or onboarding text.
    pub fn description(self) -> &'static str {
        match self {
            AppMode::Agent => "Act mode - direct work in the current session with tools",
            AppMode::Plan => "Plan mode - research and design before implementing",
            AppMode::Operate => {
                "Operate mode - always-on fleet operation: lead plans, optional $/time burn rate, workers follow the plan"
            }
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
