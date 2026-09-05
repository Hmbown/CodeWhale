//! Prototype boundary values used by the command capability shapes.
//!
//! These types deliberately do not replace the current TUI-owned production
//! types in FEAT-014. During the in-place adoption stage, thin TUI adapters
//! convert between existing application values and these boundary values.

/// Stable provider identity at the command boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandProviderId(pub String);

/// Provider-neutral reasoning preference exposed to commands.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CommandReasoningEffort {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Ultra,
    Auto,
    #[default]
    Max,
}

/// Application mode visible to commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandMode {
    Agent,
    Plan,
    Operate,
}

/// Tool-approval posture visible to commands.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CommandApprovalMode {
    Auto,
    Bypass,
    #[default]
    Suggest,
    Never,
}

/// Cost currency used by command-facing accounting operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandCurrency {
    Usd,
    Cny,
}
