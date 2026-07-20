//! Turn authority and mode/posture policy projections.
//!
//! Keep mode, approval, shell, sandbox, trust, and input provenance decisions
//! in one place so prompt metadata, tool catalogs, and runtime gates cannot
//! drift independently.

use std::path::Path;

use crate::sandbox::SandboxPolicy;
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;
use crate::worker_profile::ShellPolicy;

use super::ops::UserInputProvenance;

/// Durable Agent-era permission baseline that Plan/YOLO restore to (#3386).
///
/// Mode cycling used to be tangled with permission policy: each mode mutated
/// `allow_shell`/`trust_mode`/`approval_mode` directly and ad-hoc snapshots
/// tried to put things back on exit. Instead, keep one canonical baseline: the
/// permission surface the user has chosen for Agent mode.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModeSessionPrefs {
    pub(crate) agent_allow_shell: bool,
    pub(crate) agent_trust_mode: bool,
    pub(crate) agent_approval_mode: ApprovalMode,
}

/// The permission policy a given [`AppMode`] resolves to (#3386).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EffectiveModePolicy {
    #[allow(dead_code)]
    pub(crate) mode: AppMode,
    pub(crate) allow_shell: bool,
    pub(crate) trust_mode: bool,
    pub(crate) approval_mode: ApprovalMode,
}

/// Resolve a mode's effective permission policy from the durable Agent baseline.
///
/// This is the single source of truth for the mode/permission table:
/// - `Plan`   -> read-only: no shell, no trust, `Suggest` approvals.
/// - `Agent`  -> the user's durable baseline (`prefs`).
/// - `Auto`   -> compatibility alias for Agent; not a separate behavior.
/// - `Operate` -> Agent baseline plus orchestration posture in prompts.
/// - `Yolo`   -> legacy compat; full authority: shell + trust + `Bypass` approvals.
#[must_use]
pub(crate) fn base_policy_for_mode(mode: AppMode, prefs: &ModeSessionPrefs) -> EffectiveModePolicy {
    match mode {
        AppMode::Plan => EffectiveModePolicy {
            mode,
            allow_shell: false,
            trust_mode: false,
            approval_mode: ApprovalMode::Suggest,
        },
        AppMode::Agent | AppMode::Auto | AppMode::Operate => EffectiveModePolicy {
            mode,
            allow_shell: prefs.agent_allow_shell,
            trust_mode: prefs.agent_trust_mode,
            approval_mode: prefs.agent_approval_mode,
        },
        AppMode::Yolo => EffectiveModePolicy {
            mode,
            allow_shell: true,
            trust_mode: true,
            approval_mode: ApprovalMode::Bypass,
        },
    }
}

/// Effective authority for one engine turn after provenance narrowing.
#[derive(Debug, Clone)]
pub(crate) struct TurnAuthority {
    pub(crate) mode: AppMode,
    pub(crate) allow_shell: bool,
    pub(crate) trust_mode: bool,
    pub(crate) auto_approve: bool,
    pub(crate) approval_mode: ApprovalMode,
    pub(crate) dynamic_active_tools: Vec<&'static str>,
    pub(crate) status: Option<String>,
}

impl TurnAuthority {
    #[must_use]
    pub(crate) fn from_effective_fields(
        mode: AppMode,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
    ) -> Self {
        Self {
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
            dynamic_active_tools: Vec::new(),
            status: None,
        }
    }

    #[must_use]
    pub(crate) fn approval_mode_for_session(&self) -> ApprovalMode {
        agent_approval_mode_for_turn(self.auto_approve, self.approval_mode)
    }

    #[must_use]
    pub(crate) fn shell_policy(&self) -> ShellPolicy {
        shell_policy_for_mode(self.mode, self.allow_shell)
    }

    #[must_use]
    pub(crate) fn sandbox_policy(&self, workspace: &Path) -> SandboxPolicy {
        sandbox_policy_for_mode(self.mode, workspace)
    }
}

#[must_use]
pub(crate) fn effective_input_policy(
    provenance: UserInputProvenance,
    requested_mode: AppMode,
    _content: &str,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: ApprovalMode,
) -> TurnAuthority {
    let mut mode = requested_mode;
    let mut trust_mode = trust_mode;
    let mut auto_approve = auto_approve;
    let mut approval_mode = approval_mode;
    let mut status = None;

    if !provenance_can_inherit_standing_auto_authority(provenance) {
        let had_auto_authority = matches!(mode, AppMode::Yolo)
            || trust_mode
            || auto_approve
            || matches!(approval_mode, ApprovalMode::Bypass);
        if matches!(mode, AppMode::Yolo) {
            mode = AppMode::Agent;
        }
        trust_mode = false;
        auto_approve = false;
        if matches!(approval_mode, ApprovalMode::Auto | ApprovalMode::Bypass) {
            approval_mode = ApprovalMode::Suggest;
        }
        if had_auto_authority {
            status = Some(format!(
                "Input provenance '{}' cannot inherit standing auto-approval authority; continuing with approvals required.",
                provenance.as_str()
            ));
        }
    }

    // The named permission posture is authoritative. Normalize legacy or
    // host inputs that carry `Bypass` with a stale false auto-approve bit so
    // every engine surface observes the same Full Access contract.
    if approval_mode == ApprovalMode::Bypass {
        auto_approve = true;
    }

    TurnAuthority {
        mode,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        dynamic_active_tools: Vec::new(),
        status,
    }
}

#[must_use]
pub(crate) fn provenance_can_inherit_standing_auto_authority(
    provenance: UserInputProvenance,
) -> bool {
    matches!(
        provenance,
        UserInputProvenance::ExternalUser
            | UserInputProvenance::Runtime
            | UserInputProvenance::SubAgentHandoff
    )
}

/// Whether the active permission posture may pause the turn for a genuine user
/// decision. Permission posture controls tool approval, not whether the model
/// may invent a consequential choice. Auto-Review handles routine tool calls
/// itself, but it must still be able to surface a real missing decision.
#[must_use]
pub(crate) const fn permission_posture_allows_questions(_approval_mode: ApprovalMode) -> bool {
    true
}

#[must_use]
pub(crate) fn agent_approval_mode_for_turn(
    auto_approve: bool,
    approval_mode: ApprovalMode,
) -> ApprovalMode {
    if auto_approve {
        ApprovalMode::Bypass
    } else {
        approval_mode
    }
}

/// Pick the sandbox policy that gates shell commands for a given UI mode.
#[must_use]
pub(crate) fn sandbox_policy_for_mode(mode: AppMode, workspace: &Path) -> SandboxPolicy {
    match mode {
        AppMode::Plan => SandboxPolicy::ReadOnly,
        AppMode::Agent | AppMode::Auto | AppMode::Operate => SandboxPolicy::WorkspaceWrite {
            writable_roots: vec![workspace.to_path_buf()],
            network_access: true,
            exclude_tmpdir: false,
            exclude_slash_tmp: false,
        },
        AppMode::Yolo => SandboxPolicy::DangerFullAccess,
    }
}

/// Resolve the effective shell policy for a turn from legacy shell opt-in plus mode.
#[must_use]
pub(crate) fn shell_policy_for_mode(mode: AppMode, allow_shell: bool) -> ShellPolicy {
    if !allow_shell {
        return ShellPolicy::None;
    }
    match mode {
        AppMode::Plan => ShellPolicy::None,
        AppMode::Agent | AppMode::Auto | AppMode::Operate | AppMode::Yolo => ShellPolicy::Full,
    }
}
