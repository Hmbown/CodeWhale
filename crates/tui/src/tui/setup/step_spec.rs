//! Step ordering and navigation for the constitution-first setup wizard.
//!
//! Owns the [`SetupWizardStep`] trait, the static step spec table, and the
//! helpers that map [`SetupStep`] to wizard position and initial selection.

use codewhale_config::{SetupState, SetupStep, StepStatus};

use crate::localization::MessageId;

use super::CONSTITUTION_CHECKPOINT_VERSION;

pub trait SetupWizardStep {
    fn id(&self) -> SetupStep;
    fn title_id(&self) -> MessageId;
    fn why_id(&self) -> MessageId;
    fn required(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StaticSetupStep {
    pub(crate) id: SetupStep,
    pub(crate) title_id: MessageId,
    pub(crate) why_id: MessageId,
    pub(crate) required: bool,
}

impl SetupWizardStep for StaticSetupStep {
    fn id(&self) -> SetupStep {
        self.id
    }

    fn title_id(&self) -> MessageId {
        self.title_id
    }

    fn why_id(&self) -> MessageId {
        self.why_id
    }

    fn required(&self) -> bool {
        self.required
    }
}

pub(crate) const STEP_SPECS: [StaticSetupStep; 8] = [
    StaticSetupStep {
        id: SetupStep::Language,
        title_id: MessageId::SetupStepLanguageTitle,
        why_id: MessageId::SetupStepLanguageWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::ProviderModel,
        title_id: MessageId::SetupStepProviderModelTitle,
        why_id: MessageId::SetupStepProviderModelWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::TrustSandbox,
        title_id: MessageId::SetupStepTrustSandboxTitle,
        why_id: MessageId::SetupStepTrustSandboxWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::ToolsMcp,
        title_id: MessageId::SetupStepToolsMcpTitle,
        why_id: MessageId::SetupStepToolsMcpWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Hotbar,
        title_id: MessageId::SetupStepHotbarTitle,
        why_id: MessageId::SetupStepHotbarWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::RemoteRuntime,
        title_id: MessageId::SetupStepRemoteRuntimeTitle,
        why_id: MessageId::SetupStepRemoteRuntimeWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Constitution,
        title_id: MessageId::SetupStepConstitutionTitle,
        why_id: MessageId::SetupStepConstitutionWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::Verification,
        title_id: MessageId::SetupStepVerificationTitle,
        why_id: MessageId::SetupStepVerificationWhy,
        required: false,
    },
];

#[must_use]
pub(crate) fn step_index(step: SetupStep) -> usize {
    STEP_SPECS
        .iter()
        .position(|spec| spec.id() == step)
        .expect("all setup-state steps should have wizard specs")
}

#[must_use]
pub(crate) fn initial_step_index(state: &SetupState) -> usize {
    if state.needs_constitution_checkpoint(CONSTITUTION_CHECKPOINT_VERSION) {
        return step_index(SetupStep::Constitution);
    }
    STEP_SPECS
        .iter()
        .position(|step| {
            step.required()
                && !matches!(
                    state.status(step.id()),
                    StepStatus::Verified
                        | StepStatus::NeedsAction
                        | StepStatus::Deferred
                        | StepStatus::Optional
                        | StepStatus::Skipped
                )
        })
        .unwrap_or_else(|| step_index(SetupStep::Verification))
}
