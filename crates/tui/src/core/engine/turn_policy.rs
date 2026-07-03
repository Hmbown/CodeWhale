//! Turn-level policy: mode, approval, and trust.
//!
//! This module computes the effective input policy for each turn, resolving
//! the interaction between requested mode, user provenance, trust flags, and
//! approval settings. The resulting [`EffectiveInputPolicy`] drives tool
//! selection, approval gating, and runtime metadata for the turn.

use crate::core::ops::UserInputProvenance;
use crate::tui::app::AppMode;

/// Resolved turn-level policy computed from the requested mode, provenance,
/// and trust/approval flags.
#[derive(Debug, Clone)]
pub(crate) struct EffectiveInputPolicy {
    pub(super) mode: AppMode,
    pub(super) allow_shell: bool,
    pub(super) trust_mode: bool,
    pub(super) auto_approve: bool,
    pub(super) approval_mode: crate::tui::approval::ApprovalMode,
    pub(super) dynamic_active_tools: Vec<&'static str>,
    pub(super) status: Option<String>,
}

/// Compute the effective policy for a turn from the caller-requested mode and
/// per-turn flags, adjusting for input provenance and review-only intent.
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
    let mut status = None;

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
            status = Some(format!(
                "Input provenance '{}' cannot inherit standing auto-approval authority; continuing with approvals required.",
                provenance.as_str()
            ));
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
        status = Some(
            "Review/inspection request detected; using read-only Plan tools for this turn. Add an explicit fix/edit/commit instruction to allow writes.".to_string(),
        );
    }

    EffectiveInputPolicy {
        mode,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
        dynamic_active_tools,
        status,
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

/// Resolve the agent-visible approval mode for a turn.
///
/// When `auto_approve` is set the effective mode is always
/// [`ApprovalMode::Bypass`]; otherwise the caller-chosen mode is used as-is.
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
