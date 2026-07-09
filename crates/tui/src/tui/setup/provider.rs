use codewhale_config::{StepEntry, StepStatus};

pub(super) fn step_status(provider_ready: bool) -> StepStatus {
    if provider_ready {
        StepStatus::Verified
    } else {
        StepStatus::NeedsAction
    }
}

pub(super) fn step_entry(
    provider_ready: bool,
    checkpoint_version: &str,
    result: impl Into<String>,
) -> StepEntry {
    StepEntry::new(step_status(provider_ready), true, checkpoint_version).with_result(result.into())
}
