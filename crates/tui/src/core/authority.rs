//! Turn authority and mode/posture policy projections.
//!
//! Keep mode, approval, shell, sandbox, trust, and input provenance decisions
//! in one place so prompt metadata, tool catalogs, and runtime gates cannot
//! drift independently.

use std::path::Path;

use crate::sandbox::SandboxPolicy;
use crate::tools::spec::ApprovalRequirement;
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

/// Why runtime policy narrowed the authority a turn was asked to run with.
///
/// One variant per narrowing site. Adding a site means adding a variant, which
/// is the mechanism that makes "no silent effective mode change" enforceable
/// rather than aspirational (#3947).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PolicyNarrowingReason {
    /// Input arrived from a provenance that cannot inherit standing
    /// auto-approval authority (sub-agent handoffs, restored checkpoints).
    NonAuthoritativeProvenance,
}

impl PolicyNarrowingReason {
    /// Stable machine-readable identifier. Shared by the model-visible
    /// metadata line and doctor output so the two cannot drift.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NonAuthoritativeProvenance => "non_authoritative_provenance",
        }
    }
}

/// A structured record of one authority narrowing.
///
/// Before this existed, narrowing produced only a free-text UI status line:
/// the model saw the narrowed posture but never learned it had been narrowed
/// or why, and doctor could not report it at all. Every consumer now renders
/// from this one value, so the UI status, the `<turn_meta>` line, and doctor
/// necessarily agree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyNarrowingEvent {
    reason: PolicyNarrowingReason,
    /// Mode before narrowing, and after, as setting strings.
    from_mode: &'static str,
    to_mode: &'static str,
    /// Permission posture before narrowing, and after.
    from_approval: ApprovalMode,
    to_approval: ApprovalMode,
    /// Human-readable cause, e.g. the provenance that could not inherit.
    detail: String,
}

impl PolicyNarrowingEvent {
    pub(crate) fn reason(&self) -> PolicyNarrowingReason {
        self.reason
    }

    /// The single user-facing sentence. The TUI status line renders exactly
    /// this, and the model-visible metadata carries the same string.
    pub(crate) fn message(&self) -> String {
        match self.reason {
            PolicyNarrowingReason::NonAuthoritativeProvenance => format!(
                "Input provenance '{}' cannot inherit standing auto-approval authority; continuing with approvals required.",
                self.detail
            ),
        }
    }

    /// Compact `from -> to` summary for doctor and debug surfaces.
    pub(crate) fn transition(&self) -> String {
        format!(
            "{} ({}) -> {} ({})",
            self.from_mode,
            self.from_approval.permission_chip_label(),
            self.to_mode,
            self.to_approval.permission_chip_label(),
        )
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
    /// Structured record of any narrowing applied to this turn (#3947). The
    /// UI status line, `<turn_meta>`, and doctor all render from here, so a
    /// narrowing that reaches one surface reaches all of them.
    pub(crate) narrowing: Option<PolicyNarrowingEvent>,
}

impl TurnAuthority {
    /// The user-facing status sentence for this turn's narrowing, if any.
    pub(crate) fn status(&self) -> Option<String> {
        self.narrowing.as_ref().map(PolicyNarrowingEvent::message)
    }

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
            narrowing: None,
        }
    }

    #[must_use]
    pub(crate) fn approval_mode_for_session(&self) -> ApprovalMode {
        agent_approval_mode_for_turn(self.auto_approve, self.approval_mode)
    }

    /// Authority for the per-tool approval gate, folded from the legacy
    /// session `auto_approve` bit so [`resolve_tool_permission`] observes the
    /// same effective posture the old boolean helpers encoded: a set bit is
    /// Full Access (Yolo/Bypass-shaped), a cleared bit is an ordinary Ask
    /// turn. The engine's `Never` denial deliberately stays at the UI layer,
    /// so this constructor never produces a `Never` posture.
    #[must_use]
    pub(crate) fn for_tool_approval_decision(auto_approve: bool) -> Self {
        Self::from_effective_fields(
            if auto_approve {
                AppMode::Yolo
            } else {
                AppMode::Agent
            },
            true,
            false,
            auto_approve,
            if auto_approve {
                ApprovalMode::Bypass
            } else {
                ApprovalMode::Suggest
            },
        )
    }

    #[must_use]
    pub(crate) fn shell_policy(&self) -> ShellPolicy {
        shell_policy_for_mode(self.mode, self.allow_shell)
    }

    #[must_use]
    pub(crate) fn sandbox_policy(
        &self,
        workspace: &Path,
        configured_mode: Option<&str>,
    ) -> SandboxPolicy {
        sandbox_policy_for_turn(
            self.mode,
            self.approval_mode_for_session(),
            configured_mode,
            workspace,
        )
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
    let mut narrowing = None;

    if !provenance_can_inherit_standing_auto_authority(provenance) {
        let from_mode = mode;
        let from_approval = approval_mode;
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
            // Record the transition, not just a sentence about it: the same
            // value drives the UI status, `<turn_meta>`, and doctor (#3947).
            narrowing = Some(PolicyNarrowingEvent {
                reason: PolicyNarrowingReason::NonAuthoritativeProvenance,
                from_mode: from_mode.as_setting(),
                to_mode: mode.as_setting(),
                from_approval,
                to_approval: approval_mode,
                detail: provenance.as_str().to_string(),
            });
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
        narrowing,
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

/// Whether the active permission posture may pause the turn for a user
/// decision. Auto-Review is the fully autonomous posture: it must decide from
/// available context and keep moving. Tool approval and user-question policy
/// stay deliberately separate in every other posture.
#[must_use]
pub(crate) fn permission_posture_allows_questions(approval_mode: ApprovalMode) -> bool {
    approval_mode != ApprovalMode::Auto
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

/// Resolve the filesystem boundary for one turn.
///
/// Permission posture and filesystem scope are separate controls, but the
/// named Full Access posture must have a truthful default: outside Plan it
/// disables Codewhale's own sandbox, matching the product meaning of the
/// name. An explicit effective sandbox setting may still *tighten* that
/// default. It can never loosen Plan, Ask, or Auto-Review.
#[must_use]
pub(crate) fn sandbox_policy_for_turn(
    mode: AppMode,
    approval_mode: ApprovalMode,
    configured_mode: Option<&str>,
    workspace: &Path,
) -> SandboxPolicy {
    let default = if mode == AppMode::Plan {
        SandboxPolicy::ReadOnly
    } else if mode == AppMode::Yolo || approval_mode == ApprovalMode::Bypass {
        SandboxPolicy::DangerFullAccess
    } else {
        workspace_write_policy(workspace)
    };

    // The effective Config has already applied managed/project precedence.
    // Only stricter scopes clamp the posture-derived default: a configured
    // danger-full-access value must not silently loosen Ask or Auto-Review.
    match (default, configured_mode) {
        (SandboxPolicy::ReadOnly, _) | (_, Some("read-only")) => SandboxPolicy::ReadOnly,
        (SandboxPolicy::DangerFullAccess, Some("workspace-write")) => {
            workspace_write_policy(workspace)
        }
        (SandboxPolicy::DangerFullAccess, Some("external-sandbox")) => {
            SandboxPolicy::ExternalSandbox {
                network_access: true,
            }
        }
        (policy, _) => policy,
    }
}

fn workspace_write_policy(workspace: &Path) -> SandboxPolicy {
    SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![workspace.to_path_buf()],
        network_access: true,
        exclude_tmpdir: false,
        exclude_slash_tmp: false,
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

/// Per-tool permission decision from the unified resolver (#4412).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolPermission {
    /// Tool executes without any approval prompt.
    Allow,
    /// Tool requires user approval before execution.
    Prompt,
    /// Tool is denied without a prompt (approval_mode=Never).
    Deny,
}

/// Unified per-tool permission resolver (#4412).
///
/// Consolidates the approval decision that was previously scattered across
/// `registered_tool_approval_required` (turn_loop), `app_auto_approve_enabled`
/// (ui.rs), and the `Never` short-circuit. One call site, one answer.
///
/// The truth table mirrors the legacy helpers exactly:
/// - `Auto` tools always run — even under `Never`, which stays read-only
///   rather than dead.
/// - `Never` denies any tool that would otherwise prompt, but only when the
///   authority is not full-access shaped: a Yolo/Bypass authority carrying a
///   stale `Never` enum still auto-approves, matching the legacy UI order in
///   which the full-access shortcut ran before the `Never` check.
/// - `Suggest` and `Required` are both bypassable by auto-approve authority
///   unless the tool is on the typed non-bypassable hold list
///   (`is_non_bypassable`), which always prompts. A generic `Required` tool
///   remains auto-approved in Full Access (#3866).
#[must_use]
pub(crate) fn resolve_tool_permission(
    authority: &TurnAuthority,
    requirement: ApprovalRequirement,
    is_non_bypassable: bool,
) -> ToolPermission {
    if authority.approval_mode == ApprovalMode::Never
        && requirement != ApprovalRequirement::Auto
        && !authority.auto_approve
        && authority.mode != AppMode::Yolo
    {
        return ToolPermission::Deny;
    }
    match requirement {
        ApprovalRequirement::Auto => ToolPermission::Allow,
        ApprovalRequirement::Suggest | ApprovalRequirement::Required => {
            if is_non_bypassable {
                return ToolPermission::Prompt;
            }
            if authority.auto_approve
                || authority.approval_mode == ApprovalMode::Bypass
                || authority.mode == AppMode::Yolo
            {
                ToolPermission::Allow
            } else {
                ToolPermission::Prompt
            }
        }
    }
}

/// Disposition for an approval request that reached the UI (#4412).
///
/// The engine emits `ApprovalRequired` whenever its resolver answer was
/// `Prompt`; the UI then disposes of that request — honoring session caches
/// and posture races — through this single decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApprovalRequestDisposition {
    /// Session grant or full-access posture: approve without a modal.
    AutoApprove,
    /// The user already denied this approval key this session (#360).
    AutoDenySessionDenied,
    /// A forced (non-bypassable) policy hold arrived under a full-access
    /// posture that opens no modal: fail closed.
    AutoDenyFullAccessPolicyHold,
    /// Auto-Review is autonomous: unresolved holds fail closed instead of
    /// opening a user-approval modal.
    AutoDenyAutoReview,
    /// approval_mode=Never: deny without a modal.
    AutoDenyNeverPosture,
    /// Open the approval modal.
    Prompt,
}

/// Resolve how the UI disposes of one incoming approval request.
///
/// `session_approved` / `session_denied` are the caller's lookups into the
/// session approval caches (grouping key or tool name / exact approval key).
/// The branch order is the legacy handler's order: session denial, then the
/// full-access forced-hold denial, then auto-approval (full access or a
/// session grant), then the `Never` denial, and only finally a modal.
#[must_use]
pub(crate) fn resolve_approval_request_disposition(
    authority: &TurnAuthority,
    session_approved: bool,
    session_denied: bool,
    approval_force_prompt: bool,
) -> ApprovalRequestDisposition {
    if session_denied {
        return ApprovalRequestDisposition::AutoDenySessionDenied;
    }
    if authority.approval_mode_for_session() == ApprovalMode::Auto {
        return ApprovalRequestDisposition::AutoDenyAutoReview;
    }
    // The request exists, so the engine already resolved Prompt for the tool
    // itself. What remains is the posture question: how does this authority
    // treat an ordinary promptable tool?
    let posture = resolve_tool_permission(authority, ApprovalRequirement::Suggest, false);
    if approval_force_prompt && posture == ToolPermission::Allow {
        return ApprovalRequestDisposition::AutoDenyFullAccessPolicyHold;
    }
    if !approval_force_prompt && (posture == ToolPermission::Allow || session_approved) {
        return ApprovalRequestDisposition::AutoApprove;
    }
    if posture == ToolPermission::Deny {
        return ApprovalRequestDisposition::AutoDenyNeverPosture;
    }
    ApprovalRequestDisposition::Prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(mode: AppMode, auto_approve: bool, approval_mode: ApprovalMode) -> TurnAuthority {
        TurnAuthority::from_effective_fields(mode, true, false, auto_approve, approval_mode)
    }

    #[test]
    fn full_access_is_unsandboxed_unless_effective_config_is_stricter() {
        let workspace = Path::new("/work");
        let full_access = authority(AppMode::Agent, true, ApprovalMode::Bypass);

        assert_eq!(
            full_access.sandbox_policy(workspace, None),
            SandboxPolicy::DangerFullAccess
        );
        assert!(matches!(
            full_access.sandbox_policy(workspace, Some("workspace-write")),
            SandboxPolicy::WorkspaceWrite { writable_roots, .. }
                if writable_roots == vec![workspace.to_path_buf()]
        ));
        assert_eq!(
            full_access.sandbox_policy(workspace, Some("read-only")),
            SandboxPolicy::ReadOnly
        );
        assert!(matches!(
            full_access.sandbox_policy(workspace, Some("external-sandbox")),
            SandboxPolicy::ExternalSandbox {
                network_access: true
            }
        ));
    }

    #[test]
    fn plan_ask_and_auto_review_cannot_be_loosened_by_sandbox_config() {
        let workspace = Path::new("/work");
        for approval_mode in [ApprovalMode::Suggest, ApprovalMode::Auto] {
            let authority = authority(AppMode::Agent, false, approval_mode);
            assert!(matches!(
                authority.sandbox_policy(workspace, Some("danger-full-access")),
                SandboxPolicy::WorkspaceWrite { .. }
            ));
        }

        let plan = authority(AppMode::Plan, true, ApprovalMode::Bypass);
        assert_eq!(
            plan.sandbox_policy(workspace, Some("danger-full-access")),
            SandboxPolicy::ReadOnly
        );
    }

    #[test]
    fn auto_requirement_always_allows() {
        for (mode, auto_approve, approval_mode) in [
            (AppMode::Agent, false, ApprovalMode::Suggest),
            (AppMode::Agent, false, ApprovalMode::Auto),
            (AppMode::Agent, false, ApprovalMode::Never),
            (AppMode::Agent, true, ApprovalMode::Bypass),
            (AppMode::Yolo, true, ApprovalMode::Bypass),
            (AppMode::Plan, false, ApprovalMode::Suggest),
        ] {
            let auth = authority(mode, auto_approve, approval_mode);
            for non_bypassable in [false, true] {
                assert_eq!(
                    resolve_tool_permission(&auth, ApprovalRequirement::Auto, non_bypassable),
                    ToolPermission::Allow,
                    "{mode:?}/{auto_approve}/{approval_mode:?}/nb={non_bypassable}"
                );
            }
        }
    }

    #[test]
    fn ask_posture_prompts_for_non_auto_tools() {
        let auth = authority(AppMode::Agent, false, ApprovalMode::Suggest);
        for requirement in [ApprovalRequirement::Suggest, ApprovalRequirement::Required] {
            assert_eq!(
                resolve_tool_permission(&auth, requirement, false),
                ToolPermission::Prompt
            );
            assert_eq!(
                resolve_tool_permission(&auth, requirement, true),
                ToolPermission::Prompt
            );
        }
    }

    #[test]
    fn full_access_allows_bypassable_but_prompts_for_non_bypassable() {
        for auth in [
            authority(AppMode::Agent, true, ApprovalMode::Bypass),
            authority(AppMode::Yolo, true, ApprovalMode::Bypass),
            TurnAuthority::for_tool_approval_decision(true),
        ] {
            for requirement in [ApprovalRequirement::Suggest, ApprovalRequirement::Required] {
                assert_eq!(
                    resolve_tool_permission(&auth, requirement, false),
                    ToolPermission::Allow,
                    "generic {requirement:?} tool stays auto-approved in Full Access"
                );
                assert_eq!(
                    resolve_tool_permission(&auth, requirement, true),
                    ToolPermission::Prompt,
                    "non-bypassable {requirement:?} tool forces a prompt in Full Access"
                );
            }
        }
    }

    #[test]
    fn never_denies_promptable_tools_but_not_reads_or_full_access_shapes() {
        let never = authority(AppMode::Agent, false, ApprovalMode::Never);
        assert_eq!(
            resolve_tool_permission(&never, ApprovalRequirement::Suggest, false),
            ToolPermission::Deny
        );
        assert_eq!(
            resolve_tool_permission(&never, ApprovalRequirement::Required, true),
            ToolPermission::Deny
        );
        assert_eq!(
            resolve_tool_permission(&never, ApprovalRequirement::Auto, false),
            ToolPermission::Allow,
            "Never remains read-only rather than dead"
        );

        // Legacy host shape: full-access bit/Yolo mode with a stale Never enum
        // still auto-approves — the UI's full-access shortcut ran before its
        // Never check.
        let stale = authority(AppMode::Agent, true, ApprovalMode::Never);
        assert_eq!(
            resolve_tool_permission(&stale, ApprovalRequirement::Suggest, false),
            ToolPermission::Allow
        );
        let yolo_never = authority(AppMode::Yolo, false, ApprovalMode::Never);
        assert_eq!(
            resolve_tool_permission(&yolo_never, ApprovalRequirement::Suggest, false),
            ToolPermission::Allow
        );
    }

    #[test]
    fn approval_request_disposition_preserves_legacy_branch_order() {
        let ask = authority(AppMode::Agent, false, ApprovalMode::Suggest);
        let auto = authority(AppMode::Agent, false, ApprovalMode::Auto);
        let full_access = authority(AppMode::Agent, true, ApprovalMode::Bypass);
        let never = authority(AppMode::Agent, false, ApprovalMode::Never);

        // Session denial wins over everything, including full access.
        assert_eq!(
            resolve_approval_request_disposition(&full_access, true, true, false),
            ApprovalRequestDisposition::AutoDenySessionDenied
        );
        // Forced hold under full access fails closed instead of auto-approving.
        assert_eq!(
            resolve_approval_request_disposition(&full_access, true, false, true),
            ApprovalRequestDisposition::AutoDenyFullAccessPolicyHold
        );
        // Full access and session grants auto-approve ordinary requests.
        assert_eq!(
            resolve_approval_request_disposition(&full_access, false, false, false),
            ApprovalRequestDisposition::AutoApprove
        );
        assert_eq!(
            resolve_approval_request_disposition(&ask, true, false, false),
            ApprovalRequestDisposition::AutoApprove
        );
        // A session grant still auto-approves under Never (legacy order), and
        // Never denies everything else promptable.
        assert_eq!(
            resolve_approval_request_disposition(&never, true, false, false),
            ApprovalRequestDisposition::AutoApprove
        );
        assert_eq!(
            resolve_approval_request_disposition(&never, false, false, false),
            ApprovalRequestDisposition::AutoDenyNeverPosture
        );
        for force_prompt in [false, true] {
            assert_eq!(
                resolve_approval_request_disposition(&auto, false, false, force_prompt),
                ApprovalRequestDisposition::AutoDenyAutoReview
            );
        }
        // Ask posture with no grant opens the modal.
        assert_eq!(
            resolve_approval_request_disposition(&ask, false, false, false),
            ApprovalRequestDisposition::Prompt
        );
    }
}
