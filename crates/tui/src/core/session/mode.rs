//! Mode policy helpers.
//!
//! Session-mode logic extracted from the engine so it can be shared
//! (sub-agents, TUI commands) without pulling in the full engine crate.

use std::path::Path;

use crate::core::ops::UserInputProvenance;
use crate::prompts;
use crate::sandbox::SandboxPolicy;
use crate::tui::app::AppMode;
use crate::worker_profile::ShellPolicy;

/// User-selected mode that should remain visible in session/UI state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibleSessionMode(AppMode);

impl VisibleSessionMode {
    #[must_use]
    pub fn new(mode: AppMode) -> Self {
        Self(mode)
    }

    #[must_use]
    pub fn as_app_mode(self) -> AppMode {
        self.0
    }

    #[must_use]
    pub fn as_setting(self) -> &'static str {
        self.0.as_setting()
    }
}

impl From<AppMode> for VisibleSessionMode {
    fn from(mode: AppMode) -> Self {
        Self::new(mode)
    }
}

impl PartialEq<AppMode> for VisibleSessionMode {
    fn eq(&self, other: &AppMode) -> bool {
        self.0 == *other
    }
}

/// Effective mode used for one engine turn after policy narrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectiveTurnMode(AppMode);

impl EffectiveTurnMode {
    #[must_use]
    pub(crate) fn new(mode: AppMode) -> Self {
        Self(mode)
    }

    #[must_use]
    pub(crate) fn as_app_mode(self) -> AppMode {
        self.0
    }

    #[must_use]
    pub(crate) fn as_setting(self) -> &'static str {
        self.0.as_setting()
    }

    #[must_use]
    pub(crate) fn label(self) -> &'static str {
        self.0.label()
    }
}

impl PartialEq<AppMode> for EffectiveTurnMode {
    fn eq(&self, other: &AppMode) -> bool {
        self.0 == *other
    }
}

/// Pick the sandbox policy that gates shell commands for a given UI mode.
///
/// - **Plan** (#1077): `ReadOnly` — no writes, no network. The previous
///   `WorkspaceWrite` policy let `python -c "open('f','w').write('x')"` mutate
///   files inside the workspace because it whitelisted the workspace as
///   writable. Plan mode is investigation only; if the user wants to change
///   files they should switch to Agent.
/// - **Agent/Auto**: `WorkspaceWrite` with workspace as writable root and
///   network on. Approval flow gates risky individual commands; the sandbox
///   handles the rest. Network is allowed because cargo / npm / curl-style
///   commands are normal during agent work and DNS-deny breaks them silently.
/// - **YOLO**: `DangerFullAccess` — explicit no-guardrails contract.
pub(crate) fn sandbox_policy_for_mode(mode: AppMode, workspace: &Path) -> SandboxPolicy {
    match mode {
        AppMode::Plan => SandboxPolicy::ReadOnly,
        AppMode::Agent | AppMode::Auto => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![workspace.to_path_buf()],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        },
        AppMode::Yolo => SandboxPolicy::DangerFullAccess,
    }
}

/// Resolve the effective shell policy for a turn from the legacy shell opt-in
/// plus the active mode. This is the typed bridge away from passing a bare
/// `allow_shell` boolean through the runtime.
pub(crate) fn shell_policy_for_mode(mode: AppMode, allow_shell: bool) -> ShellPolicy {
    if !allow_shell {
        return ShellPolicy::None;
    }
    match mode {
        // Plan is read-only planning with no shell execution. The runtime
        // prompt already reports `shell_access="none"` for Plan, so mapping it
        // to `ReadOnly` here created a prompt/registry inconsistency (the
        // registry would expose `exec_shell` while the prompt said there was
        // no shell). Keep Plan shell-free; switch to Agent to run commands.
        AppMode::Plan => ShellPolicy::None,
        AppMode::Agent | AppMode::Auto | AppMode::Yolo => ShellPolicy::Full,
    }
}

/// Return the mode-specific runtime instructions (prompt delta) for `mode`.
pub(crate) fn mode_runtime_instructions(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Agent | AppMode::Auto => prompts::AGENT_MODE,
        AppMode::Plan => prompts::PLAN_MODE,
        AppMode::Yolo => prompts::YOLO_MODE,
    }
    .trim()
}

// ── effective input policy ──────────────────────────────────────────

/// Reason an effective turn policy differs from the visible session mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyNarrowingReason {
    NonAuthoritativeInput(UserInputProvenance),
    ReviewOnlyExternalInput,
}

impl PolicyNarrowingReason {
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NonAuthoritativeInput(_) => "non_authoritative_input",
            Self::ReviewOnlyExternalInput => "review_only_external_input",
        }
    }

    #[must_use]
    pub(crate) fn status_message(self) -> String {
        match self {
            Self::NonAuthoritativeInput(provenance) => format!(
                "Input provenance '{}' cannot inherit standing auto-approval authority; continuing with approvals required.",
                provenance.as_str()
            ),
            Self::ReviewOnlyExternalInput => "Review/inspection request detected; using read-only Plan tools for this turn. Add an explicit fix/edit/commit instruction to allow writes.".to_string(),
        }
    }
}

/// Per-turn policy projection derived from provenance, content, and session
/// state.  Avoids persisting transient routing decisions into the session
/// history and avoids sending extra system messages.  Each API request
/// projects this as a transient user-role runtime metadata message at the
/// tail, leaving the stable system prompt and stored history byte-stable
/// even for strict chat-template providers.
#[derive(Debug, Clone)]
pub(crate) struct EffectiveInputPolicy {
    pub(crate) visible_mode: VisibleSessionMode,
    effective_mode: EffectiveTurnMode,
    pub(crate) allow_shell: bool,
    pub(crate) trust_mode: bool,
    pub(crate) auto_approve: bool,
    pub(crate) approval_mode: crate::tui::approval::ApprovalMode,
    pub(crate) dynamic_active_tools: Vec<&'static str>,
    pub(crate) narrowing: Option<PolicyNarrowingReason>,
}

impl EffectiveInputPolicy {
    #[must_use]
    pub(crate) fn mode(&self) -> AppMode {
        self.effective_mode.as_app_mode()
    }

    #[must_use]
    pub(crate) fn mode_setting(&self) -> &'static str {
        self.effective_mode.as_setting()
    }

    #[must_use]
    pub(crate) fn mode_label(&self) -> &'static str {
        self.effective_mode.label()
    }

    #[must_use]
    pub(crate) fn status_message(&self) -> Option<String> {
        self.narrowing.map(PolicyNarrowingReason::status_message)
    }
}

pub(crate) fn effective_input_policy(
    provenance: UserInputProvenance,
    requested_mode: AppMode,
    content: &str,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> EffectiveInputPolicy {
    let mut mode = requested_mode;
    let mut trust_mode = trust_mode;
    let mut auto_approve = auto_approve;
    let mut approval_mode = approval_mode;
    let dynamic_active_tools = Vec::new();
    let mut narrowing = None;

    if !provenance_can_inherit_standing_auto_authority(provenance) {
        let had_auto_authority = matches!(mode, AppMode::Yolo)
            || trust_mode
            || auto_approve
            || matches!(approval_mode, crate::tui::approval::ApprovalMode::Bypass);
        if matches!(mode, AppMode::Yolo) {
            mode = AppMode::Agent;
        }
        trust_mode = false;
        auto_approve = false;
        if matches!(
            approval_mode,
            crate::tui::approval::ApprovalMode::Auto | crate::tui::approval::ApprovalMode::Bypass
        ) {
            approval_mode = crate::tui::approval::ApprovalMode::Suggest;
        }
        if had_auto_authority {
            narrowing = Some(PolicyNarrowingReason::NonAuthoritativeInput(provenance));
        }
    } else if matches!(provenance, UserInputProvenance::ExternalUser)
        && is_review_only_user_intent(content)
    {
        mode = AppMode::Plan;
        trust_mode = false;
        auto_approve = false;
        if matches!(
            approval_mode,
            crate::tui::approval::ApprovalMode::Auto | crate::tui::approval::ApprovalMode::Bypass
        ) {
            approval_mode = crate::tui::approval::ApprovalMode::Suggest;
        }
        narrowing = Some(PolicyNarrowingReason::ReviewOnlyExternalInput);
    }

    EffectiveInputPolicy {
        visible_mode: VisibleSessionMode::new(requested_mode),
        effective_mode: EffectiveTurnMode::new(mode),
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        dynamic_active_tools,
        narrowing,
    }
}

fn provenance_can_inherit_standing_auto_authority(provenance: UserInputProvenance) -> bool {
    matches!(
        provenance,
        UserInputProvenance::ExternalUser
            | UserInputProvenance::Runtime
            | UserInputProvenance::SubAgentHandoff
    )
}

fn is_review_only_user_intent(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    let asks_to_inspect = [
        "look",
        "check",
        "review",
        "inspect",
        "scan",
        "audit",
        "看看",
        "看一下",
        "检查",
        "审查",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if !asks_to_inspect {
        return false;
    }

    let explicit_write = [
        "fix",
        "change",
        "update",
        "implement",
        "apply",
        "patch",
        "modify",
        "edit",
        "write",
        "commit",
        "修",
        "改",
        "补",
        "提交",
        "写",
    ]
    .iter()
    .any(|needle| lower.contains(needle));

    !explicit_write
}

pub(crate) fn agent_approval_mode_for_turn(
    auto_approve: bool,
    approval_mode: crate::tui::approval::ApprovalMode,
) -> crate::tui::approval::ApprovalMode {
    if auto_approve {
        crate::tui::approval::ApprovalMode::Bypass
    } else {
        approval_mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_policy_for_mode_returns_correct_policy_per_mode() {
        let workspace = Path::new("/tmp/ws");

        // Plan: ReadOnly. The whole point of #1077.
        assert!(matches!(
            sandbox_policy_for_mode(AppMode::Plan, workspace),
            SandboxPolicy::ReadOnly
        ));

        // Agent: WorkspaceWrite with workspace as writable root, network on.
        match sandbox_policy_for_mode(AppMode::Agent, workspace) {
            SandboxPolicy::WorkspaceWrite {
                writable_roots,
                network_access,
                ..
            } => {
                assert_eq!(writable_roots, vec![workspace]);
                assert!(network_access);
            }
            other => panic!("expected WorkspaceWrite, got {other:?}"),
        }

        // YOLO: DangerFullAccess.
        assert!(matches!(
            sandbox_policy_for_mode(AppMode::Yolo, workspace),
            SandboxPolicy::DangerFullAccess
        ));
    }
}
