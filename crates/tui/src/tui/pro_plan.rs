use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProPlanPhase {
    Plan,
    Execute,
    Review,
    Done,
}

impl Default for ProPlanPhase {
    fn default() -> Self {
        ProPlanPhase::Plan
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProPlanFollowUp {
    ReviewImplementation,
    AddressReviewFeedback,
}

#[derive(Debug, Clone)]
pub struct ProPlanConfig {
    pub plan_model: &'static str,
    pub execute_model: &'static str,
    pub review_model: &'static str,
}

impl Default for ProPlanConfig {
    fn default() -> Self {
        Self {
            plan_model: "deepseek-v4-pro",
            execute_model: "deepseek-v4-flash",
            review_model: "deepseek-v4-pro",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProPlanState {
    pub phase: ProPlanPhase,
    pub has_generated_plan: bool,
    pub plan_turns: u32,
    pub execute_turns: u32,
}

impl Default for ProPlanState {
    fn default() -> Self {
        Self {
            phase: ProPlanPhase::default(),
            has_generated_plan: false,
            plan_turns: 0,
            execute_turns: 0,
        }
    }
}

pub struct ProPlanRouter {
    config: ProPlanConfig,
    state: ProPlanState,
}

impl ProPlanRouter {
    pub fn new(config: ProPlanConfig) -> Self {
        Self {
            config,
            state: ProPlanState::default(),
        }
    }

    pub fn current_model(&self) -> &'static str {
        match self.state.phase {
            ProPlanPhase::Plan => self.config.plan_model,
            ProPlanPhase::Execute => self.config.execute_model,
            ProPlanPhase::Review => self.config.review_model,
            ProPlanPhase::Done => self.config.review_model,
        }
    }

    pub fn phase(&self) -> ProPlanPhase {
        self.state.phase
    }

    pub fn state(&self) -> &ProPlanState {
        &self.state
    }

    pub fn transition(&mut self, msg: &str) -> ProPlanPhase {
        let msg_lower = msg.to_ascii_lowercase();

        match self.state.phase {
            ProPlanPhase::Plan => {
                self.state.plan_turns += 1;
                if ProPlanRouter::contains_plan_marker(&msg_lower) {
                    self.state.has_generated_plan = true;
                }
            }
            ProPlanPhase::Execute => {
                self.state.execute_turns += 1;
                if Self::execute_complete(&msg_lower) {
                    self.state.phase = ProPlanPhase::Review;
                    return ProPlanPhase::Review;
                }
                if Self::should_replan(&msg_lower) {
                    self.state.phase = ProPlanPhase::Plan;
                    self.state.has_generated_plan = false;
                    self.state.plan_turns = 0;
                    self.state.execute_turns = 0;
                    return ProPlanPhase::Plan;
                }
            }
            ProPlanPhase::Review => {
                if Self::review_rejected(&msg_lower) {
                    self.state.phase = ProPlanPhase::Execute;
                    return ProPlanPhase::Execute;
                }
                if Self::review_approved(&msg_lower) {
                    self.state.phase = ProPlanPhase::Done;
                    return ProPlanPhase::Done;
                }
            }
            ProPlanPhase::Done => {}
        }

        self.state.phase
    }

    pub fn mark_plan_ready(&mut self) {
        self.state.has_generated_plan = true;
    }

    pub fn start_execution(&mut self) {
        self.state.phase = ProPlanPhase::Execute;
    }

    pub fn reset(&mut self) {
        self.state = ProPlanState::default();
    }

    pub fn follow_up_after_transition(
        before: ProPlanPhase,
        after: ProPlanPhase,
    ) -> Option<ProPlanFollowUp> {
        match (before, after) {
            (ProPlanPhase::Execute, ProPlanPhase::Review) => {
                Some(ProPlanFollowUp::ReviewImplementation)
            }
            (ProPlanPhase::Review, ProPlanPhase::Execute) => {
                Some(ProPlanFollowUp::AddressReviewFeedback)
            }
            _ => None,
        }
    }

    fn contains_plan_marker(msg: &str) -> bool {
        let markers = ["<pro_plan plan_ready=\"true\"", "<pro_plan_plan_ready>"];
        markers.iter().any(|m| msg.contains(m))
    }

    fn execute_complete(msg: &str) -> bool {
        let keywords = [
            "<pro_plan execute_complete=\"true\"",
            "<pro_plan_execute_complete>",
        ];
        keywords.iter().any(|k| msg.contains(k))
    }

    fn should_replan(msg: &str) -> bool {
        let keywords = [
            "<pro_plan replan=\"true\"",
            "<pro_plan_replan>",
            "<pro_plan plan_ready=\"false\"",
        ];
        keywords.iter().any(|k| msg.contains(k))
    }

    fn review_approved(msg: &str) -> bool {
        let keywords = [
            "<pro_plan review=\"approved\"",
            "<pro_plan_review_approved>",
        ];
        keywords.iter().any(|k| msg.contains(k))
    }

    fn review_rejected(msg: &str) -> bool {
        let keywords = [
            "<pro_plan review=\"changes_requested\"",
            "<pro_plan_review_changes_requested>",
        ];
        keywords.iter().any(|k| msg.contains(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_phase_is_plan() {
        let config = ProPlanConfig::default();
        let router = ProPlanRouter::new(config);
        assert_eq!(router.phase(), ProPlanPhase::Plan);
        assert_eq!(router.current_model(), "deepseek-v4-pro");
    }

    #[test]
    fn test_plan_to_execute_transition() {
        let config = ProPlanConfig::default();
        let mut router = ProPlanRouter::new(config);

        assert_eq!(router.phase(), ProPlanPhase::Plan);
        router.transition("Here is my plan:\n<pro_plan plan_ready=\"true\">");
        assert_eq!(router.phase(), ProPlanPhase::Plan);
        assert!(router.state.has_generated_plan);
        router.start_execution();
        assert_eq!(router.current_model(), "deepseek-v4-flash");
    }

    #[test]
    fn ordinary_numbered_answer_does_not_mark_plan_ready() {
        let config = ProPlanConfig::default();
        let mut router = ProPlanRouter::new(config);

        router.transition("1. ProPlan exists\n2. /mode pro-plan works\n3. No changes needed");

        assert_eq!(router.phase(), ProPlanPhase::Plan);
        assert!(!router.state.has_generated_plan);
    }

    #[test]
    fn test_execute_to_review_transition_requires_completion_marker() {
        let config = ProPlanConfig::default();
        let mut router = ProPlanRouter::new(config);

        router.state.phase = ProPlanPhase::Execute;
        router.state.has_generated_plan = true;

        router.transition("please review this");
        assert_eq!(router.phase(), ProPlanPhase::Execute);

        router.transition("<pro_plan execute_complete=\"true\">");
        assert_eq!(router.phase(), ProPlanPhase::Review);
        assert_eq!(router.current_model(), "deepseek-v4-pro");
    }

    #[test]
    fn test_review_approved_to_done_requires_marker() {
        let config = ProPlanConfig::default();
        let mut router = ProPlanRouter::new(config);

        router.state.phase = ProPlanPhase::Review;
        router.transition("lgtm");
        assert_eq!(router.phase(), ProPlanPhase::Review);

        router.transition("<pro_plan review=\"approved\">");
        assert_eq!(router.phase(), ProPlanPhase::Done);
        assert_eq!(router.current_model(), "deepseek-v4-pro");
    }

    #[test]
    fn test_review_rejected_to_plan() {
        let config = ProPlanConfig::default();
        let mut router = ProPlanRouter::new(config);

        router.state.phase = ProPlanPhase::Review;
        router.state.has_generated_plan = true;
        router.state.execute_turns = 5;

        router.transition("not good, please replan");
        assert_eq!(router.phase(), ProPlanPhase::Review);

        router.transition("<pro_plan review=\"changes_requested\">");
        assert_eq!(router.phase(), ProPlanPhase::Execute);
        assert!(router.state.has_generated_plan);
    }

    #[test]
    fn test_replan_during_execute() {
        let config = ProPlanConfig::default();
        let mut router = ProPlanRouter::new(config);

        router.state.phase = ProPlanPhase::Execute;
        router.state.has_generated_plan = true;
        router.state.execute_turns = 3;

        router.transition("replan this");
        assert_eq!(router.phase(), ProPlanPhase::Execute);

        router.transition("<pro_plan replan=\"true\">");
        assert_eq!(router.phase(), ProPlanPhase::Plan);
        assert!(!router.state.has_generated_plan);
    }

    #[test]
    fn follow_up_actions_only_emit_on_real_phase_edges() {
        assert_eq!(
            ProPlanRouter::follow_up_after_transition(ProPlanPhase::Execute, ProPlanPhase::Review),
            Some(ProPlanFollowUp::ReviewImplementation)
        );
        assert_eq!(
            ProPlanRouter::follow_up_after_transition(ProPlanPhase::Review, ProPlanPhase::Execute),
            Some(ProPlanFollowUp::AddressReviewFeedback)
        );
        assert_eq!(
            ProPlanRouter::follow_up_after_transition(ProPlanPhase::Review, ProPlanPhase::Review),
            None
        );
        assert_eq!(
            ProPlanRouter::follow_up_after_transition(ProPlanPhase::Execute, ProPlanPhase::Execute),
            None
        );
    }
}
