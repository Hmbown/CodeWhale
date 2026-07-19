//! Accepted-Plan transition into the Work Graph.

/// The outcomes that can follow the Plan confirmation prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAcceptance {
    AcceptAct,
    AcceptFullAccess,
    Revise,
    Exit,
}

impl PlanAcceptance {
    fn approval_reference(self) -> Option<&'static str> {
        match self {
            Self::AcceptAct => Some("accept_act"),
            Self::AcceptFullAccess => Some("accept_full_access"),
            Self::Revise | Self::Exit => None,
        }
    }
}

/// Record accepted Plan projection through `AcceptPlanDiff`; the retired
/// Plan→To-do writer has no fallback path.
pub async fn project_accepted_plan(
    work: Option<&crate::work_graph::SharedWorkRuntime>,
    session_id: Option<&str>,
    acceptance: PlanAcceptance,
    expected_proposal_id: Option<&crate::work_graph::ProposalId>,
) -> Result<usize, String> {
    let Some(approval_reference) = acceptance.approval_reference() else {
        return Ok(0);
    };
    let work = work.ok_or_else(|| "Work Graph runtime is not attached".to_string())?;
    work.accept_plan(session_id, approval_reference, expected_proposal_id)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revise_and_exit_need_no_graph_and_never_write() {
        assert_eq!(
            project_accepted_plan(None, None, PlanAcceptance::Revise, None).await,
            Ok(0)
        );
        assert_eq!(
            project_accepted_plan(None, None, PlanAcceptance::Exit, None).await,
            Ok(0)
        );
    }

    #[tokio::test]
    async fn acceptance_fails_closed_without_graph_authority() {
        assert_eq!(
            project_accepted_plan(None, None, PlanAcceptance::AcceptAct, None)
                .await
                .unwrap_err(),
            "Work Graph runtime is not attached"
        );
    }
}
