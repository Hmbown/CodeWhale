//! Turn-level policy resolution.
//!
//! Prompt wording can describe or hint at intent, but the effective authority
//! for a turn is derived from structured runtime state only.

use std::path::{Path, PathBuf};

use crate::core::ops::UserInputProvenance;
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;

use super::authority::TurnAuthority;

#[derive(Debug, Clone)]
pub(super) struct EffectiveInputPolicy {
    pub(super) mode: AppMode,
    pub(super) allow_shell: bool,
    pub(super) trust_mode: bool,
    pub(super) auto_approve: bool,
    pub(super) approval_mode: ApprovalMode,
    pub(super) dynamic_active_tools: Vec<&'static str>,
    pub(super) status: Option<String>,
    pub(super) intent_advisory: Option<TurnIntentAdvisory>,
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

        EffectiveInputPolicy {
            mode: authority.posture.mode,
            allow_shell: authority.posture.allow_shell,
            trust_mode: authority.posture.trust_mode,
            auto_approve: authority.posture.auto_approve,
            approval_mode: authority.posture.approval_mode,
            dynamic_active_tools: Vec::new(),
            status: authority.narrowing_reason,
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
