//! Turn-level policy resolution.
//!
//! Prompt wording can describe or hint at intent, but the effective authority
//! for a turn is derived from structured runtime state only.

use std::path::{Path, PathBuf};

use crate::core::ops::UserInputProvenance;
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;

use super::authority::TurnAuthority;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SessionMode(AppMode);

impl SessionMode {
    fn new(mode: AppMode) -> Self {
        Self(mode)
    }

    pub(super) fn as_app_mode(self) -> AppMode {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EffectiveTurnMode(AppMode);

impl EffectiveTurnMode {
    fn new(mode: AppMode) -> Self {
        Self(mode)
    }

    pub(super) fn as_app_mode(self) -> AppMode {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ApprovalAuthority {
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: ApprovalMode,
}

impl ApprovalAuthority {
    fn from_authority(authority: &TurnAuthority) -> Self {
        Self {
            trust_mode: authority.posture.trust_mode,
            auto_approve: authority.posture.auto_approve,
            approval_mode: authority.posture.approval_mode,
        }
    }

    pub(super) fn trust_mode(self) -> bool {
        self.trust_mode
    }

    pub(super) fn auto_approve(self) -> bool {
        self.auto_approve
    }

    pub(super) fn approval_mode(self) -> ApprovalMode {
        self.approval_mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InputProvenanceAuthority {
    Authoritative(UserInputProvenance),
    NonAuthoritative(UserInputProvenance),
}

impl InputProvenanceAuthority {
    fn from_policy(provenance: UserInputProvenance, narrowing_reason: Option<&str>) -> Self {
        if narrowing_reason.is_some() {
            Self::NonAuthoritative(provenance)
        } else {
            Self::Authoritative(provenance)
        }
    }

    pub(super) fn provenance(self) -> UserInputProvenance {
        match self {
            Self::Authoritative(provenance) | Self::NonAuthoritative(provenance) => provenance,
        }
    }

    pub(super) fn can_inherit_standing_auto_authority(self) -> bool {
        matches!(self, Self::Authoritative(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PolicyNarrowingEvent {
    reason: PolicyNarrowingReason,
    message: String,
}

impl PolicyNarrowingEvent {
    fn new(reason: PolicyNarrowingReason, message: String) -> Self {
        Self { reason, message }
    }

    pub(super) fn reason(&self) -> PolicyNarrowingReason {
        self.reason
    }

    pub(super) fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PolicyNarrowingReason {
    NonAuthoritativeInput(UserInputProvenance),
}

impl PolicyNarrowingReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::NonAuthoritativeInput(_) => "non_authoritative_input",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct EffectiveInputPolicy {
    session_mode: SessionMode,
    effective_mode: EffectiveTurnMode,
    authority: TurnAuthority,
    approval_authority: ApprovalAuthority,
    provenance_authority: InputProvenanceAuthority,
    dynamic_active_tools: Vec<&'static str>,
    narrowing_event: Option<PolicyNarrowingEvent>,
    intent_advisory: Option<TurnIntentAdvisory>,
}

impl EffectiveInputPolicy {
    pub(super) fn session_mode(&self) -> SessionMode {
        self.session_mode
    }

    pub(super) fn effective_mode(&self) -> EffectiveTurnMode {
        self.effective_mode
    }

    pub(super) fn mode(&self) -> AppMode {
        self.effective_mode.as_app_mode()
    }

    pub(super) fn allow_shell(&self) -> bool {
        self.authority.posture.allow_shell
    }

    pub(super) fn trust_mode(&self) -> bool {
        self.approval_authority.trust_mode()
    }

    pub(super) fn auto_approve(&self) -> bool {
        self.approval_authority.auto_approve()
    }

    pub(super) fn approval_mode(&self) -> ApprovalMode {
        self.approval_authority.approval_mode()
    }

    pub(super) fn dynamic_active_tools(&self) -> &[&'static str] {
        &self.dynamic_active_tools
    }

    pub(super) fn status(&self) -> Option<String> {
        self.narrowing_event
            .as_ref()
            .map(|event| event.message().to_string())
    }

    pub(super) fn narrowing_event(&self) -> Option<&PolicyNarrowingEvent> {
        self.narrowing_event.as_ref()
    }

    pub(super) fn provenance_authority(&self) -> InputProvenanceAuthority {
        self.provenance_authority
    }

    pub(super) fn intent_advisory(&self) -> Option<TurnIntentAdvisory> {
        self.intent_advisory
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnIntentAdvisory {
    ReviewOrInspection,
}

#[derive(Debug, Clone)]
pub(super) struct TurnPolicyResolver<'a> {
    provenance: UserInputProvenance,
    requested_mode: AppMode,
    workspace: PathBuf,
    content: &'a str,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: ApprovalMode,
}

impl<'a> TurnPolicyResolver<'a> {
    pub(super) fn new(
        provenance: UserInputProvenance,
        requested_mode: AppMode,
        workspace: &Path,
        content: &'a str,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
    ) -> Self {
        Self {
            provenance,
            requested_mode,
            workspace: workspace.to_path_buf(),
            content,
            allow_shell,
            trust_mode,
            auto_approve,
            approval_mode,
        }
    }

    pub(super) fn resolve(&self) -> EffectiveInputPolicy {
        let authority = TurnAuthority::for_input(
            self.provenance,
            self.requested_mode,
            &self.workspace,
            self.allow_shell,
            self.trust_mode,
            self.auto_approve,
            self.approval_mode,
        );
        let narrowing_event = authority.narrowing_reason.as_ref().map(|message| {
            PolicyNarrowingEvent::new(
                PolicyNarrowingReason::NonAuthoritativeInput(self.provenance),
                message.clone(),
            )
        });
        let provenance_authority = InputProvenanceAuthority::from_policy(
            self.provenance,
            authority.narrowing_reason.as_deref(),
        );
        let approval_authority = ApprovalAuthority::from_authority(&authority);

        EffectiveInputPolicy {
            session_mode: SessionMode::new(self.requested_mode),
            effective_mode: EffectiveTurnMode::new(authority.posture.mode),
            authority,
            approval_authority,
            provenance_authority,
            dynamic_active_tools: Vec::new(),
            narrowing_event,
            intent_advisory: self.intent_advisory(),
        }
    }

    fn intent_advisory(&self) -> Option<TurnIntentAdvisory> {
        if matches!(self.provenance, UserInputProvenance::ExternalUser)
            && looks_like_review_or_inspection(self.content)
        {
            Some(TurnIntentAdvisory::ReviewOrInspection)
        } else {
            None
        }
    }
}

pub(super) fn effective_input_policy(
    provenance: UserInputProvenance,
    requested_mode: AppMode,
    workspace: &Path,
    content: &str,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
    approval_mode: ApprovalMode,
) -> EffectiveInputPolicy {
    TurnPolicyResolver::new(
        provenance,
        requested_mode,
        workspace,
        content,
        allow_shell,
        trust_mode,
        auto_approve,
        approval_mode,
    )
    .resolve()
}

fn looks_like_review_or_inspection(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    ["look", "check", "review", "inspect", "scan", "audit"]
        .iter()
        .any(|needle| lower.contains(needle))
}
