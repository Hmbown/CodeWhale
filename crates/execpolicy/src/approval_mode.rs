//! The user-facing approval posture (`Ask` / `Auto-Review` / `Full Access` /
//! `Never`). Lives beside `AskForApproval` so policy code and the TUI share
//! one definition; the TUI adds only presentation on top.

/// Determines when tool executions require user approval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalMode {
    /// Automatically review risky tool calls before deciding whether to ask.
    Auto,
    /// Bypass approvals entirely (YOLO mode / --yolo flag).
    Bypass,
    /// Suggest approval for non-safe tools (non-YOLO modes)
    #[default]
    Suggest,
    /// Never execute tools requiring approval
    Never,
}

impl ApprovalMode {
    /// Shift+Tab permission cycle order (#0.8.68 M2).
    pub const PERMISSION_CYCLE: [Self; 3] = [Self::Suggest, Self::Auto, Self::Bypass];

    pub fn label(self) -> &'static str {
        match self {
            ApprovalMode::Auto => "AUTO",
            ApprovalMode::Bypass => "BYPASS",
            ApprovalMode::Suggest => "SUGGEST",
            ApprovalMode::Never => "NEVER",
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "auto-review" | "auto_review" => Some(ApprovalMode::Auto),
            "bypass" | "yolo" | "dontask" | "dont_ask" | "bypass-permissions"
            | "bypasspermissions" | "full-access" | "full_access" | "full" => {
                Some(ApprovalMode::Bypass)
            }
            "suggest" | "suggested" | "on-request" | "untrusted" | "ask" => {
                Some(ApprovalMode::Suggest)
            }
            "never" | "deny" | "denied" => Some(ApprovalMode::Never),
            _ => None,
        }
    }

    #[must_use]
    pub fn cycle_permission_next(self) -> Self {
        let Some(index) = Self::PERMISSION_CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Suggest;
        };
        Self::PERMISSION_CYCLE[(index + 1) % Self::PERMISSION_CYCLE.len()]
    }

    #[must_use]
    pub fn permission_chip_label(self) -> &'static str {
        match self {
            Self::Suggest => "Ask",
            Self::Auto => "Auto-Review",
            Self::Bypass => "Full Access",
            Self::Never => "Never",
        }
    }
}
