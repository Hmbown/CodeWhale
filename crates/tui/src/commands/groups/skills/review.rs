//! Review command: activate review skill and send a target immediately.
//!
//! FEAT-022 Phase 4: portable contextual dispatch. The host performs the
//! discovery + side effects (`CommandSkillGroupContext::run_review`); the
//! portable handler composes the exact error text and the `SendMessage` action.

use codewhale_command_contract::facets::{CommandSkillGroupContext, ReviewOutcome};
use codewhale_command_contract::handler::{CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::commands::CommandResult;
use crate::tui::app::AppAction;

/// Render the review warnings suffix (baseline `warnings_suffix`).
fn warnings_suffix(warnings: &[String]) -> String {
    if warnings.is_empty() {
        return String::new();
    }

    format!("\n\nWarnings:\n- {}", warnings.join("\n- "))
}

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "review",
    aliases: &["shencha"],
    usage: "/review <target>",
    description_key: "cmd_review_description",
};

pub(in crate::commands) struct ReviewCmd;

impl RegisterCommand<CommandResult> for ReviewCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: codewhale_command_contract::handler::CommandCapabilities::SKILL_GROUP,
            handler: review_contextual,
        }
    }
}

/// Contextual `/review` dispatch: exactly the skill-group facet. The baseline
/// command never refreshed the shared skill cache, so `/review` must not
/// request the unrelated SKILLS facet.
fn review_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let mut parts = contexts.into_parts();
    let Some(skill_group) = parts.skill_group.as_deref_mut() else {
        return CommandResult::error("Command capability unavailable: skill_group");
    };
    review(skill_group, arg)
}

/// Portable `/review` dispatch — byte-identical to the baseline handler.
///
/// The host performs discovery, warning merge, session-message insertion, and
/// active-skill mutation (`run_review`); the handler validates the target,
/// renders the not-found error, and emits the `SendMessage` action. The
/// baseline success path renders no message and does not refresh the cache.
fn review(group: &mut dyn CommandSkillGroupContext, arg: Option<&str>) -> CommandResult {
    let target = arg.unwrap_or("").trim();
    if target.is_empty() {
        return CommandResult::error("Usage: /review <target>");
    }

    match group.run_review() {
        Ok(ReviewOutcome::Ready) => {
            CommandResult::action(AppAction::SendMessage(target.to_string()))
        }
        Ok(ReviewOutcome::NotFound {
            skills_dir,
            global_dir,
            warnings,
        }) => {
            let warnings = warnings_suffix(&warnings);
            CommandResult::error(format!(
                "Review skill not found in {} or {}. Create ~/.codewhale/skills/review/SKILL.md.{}",
                skills_dir, global_dir, warnings
            ))
        }
        Err(err) => CommandResult::error(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_command_contract::facets::{
        CommandApprovalState, RemoteRegistryOutcome, SkillActivationError, SkillMutationReceipt,
        SkillRecommendation, SkillSyncOutcome, SkillTargetScope, SnapshotEntry,
    };

    struct FakeSkillGroup {
        review: Result<ReviewOutcome, String>,
        approval: CommandApprovalState,
    }
    impl FakeSkillGroup {
        fn ready() -> Self {
            Self {
                review: Ok(ReviewOutcome::Ready),
                approval: CommandApprovalState {
                    yolo: true,
                    trust_mode: false,
                },
            }
        }
    }
    impl CommandSkillGroupContext for FakeSkillGroup {
        fn skill_registry_projection(
            &self,
        ) -> codewhale_command_contract::facets::SkillRegistryProjection {
            unimplemented!("not used by review tests")
        }
        fn activate_skill(
            &mut self,
            _name: &str,
        ) -> Result<codewhale_command_contract::facets::SkillActivationOutcome, SkillActivationError>
        {
            unimplemented!("not used by review tests")
        }
        fn install_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _spec: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by review tests")
        }
        fn update_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _name: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by review tests")
        }
        fn uninstall_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _name: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by review tests")
        }
        fn trust_skill(
            &mut self,
            _scope: Option<SkillTargetScope>,
            _name: &str,
        ) -> Result<SkillMutationReceipt, String> {
            unimplemented!("not used by review tests")
        }
        fn fetch_remote_registry(&mut self) -> Result<RemoteRegistryOutcome, String> {
            unimplemented!("not used by review tests")
        }
        fn recommend_skills(&mut self, _task: &str) -> Result<Vec<SkillRecommendation>, String> {
            unimplemented!("not used by review tests")
        }
        fn sync_registry(&mut self) -> Result<SkillSyncOutcome, String> {
            unimplemented!("not used by review tests")
        }
        fn run_review(&mut self) -> Result<ReviewOutcome, String> {
            self.review.clone()
        }
        fn snapshot_list(&mut self, _limit: usize) -> Result<Vec<SnapshotEntry>, String> {
            unimplemented!("not used by review tests")
        }
        fn restore_snapshot(&mut self, _id: &str) -> Result<(), String> {
            unimplemented!("not used by review tests")
        }
        fn approval_state(&self) -> CommandApprovalState {
            self.approval
        }
    }

    #[test]
    fn review_without_target_prints_usage() {
        let mut group = FakeSkillGroup::ready();
        let result = review(&mut group, None);
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Usage: /review"));
    }

    #[test]
    fn review_ready_sends_target_without_skills_context() {
        let mut group = FakeSkillGroup::ready();
        let contexts = CommandContexts::empty().with_skill_group(&mut group);
        let result = review_contextual(contexts, Some("file.rs"));
        assert!(result.message.is_none());
        assert!(matches!(
            result.action,
            Some(AppAction::SendMessage(ref t)) if t == "file.rs"
        ));
    }

    #[test]
    fn review_not_found_renders_exact_error_with_warnings() {
        let mut group = FakeSkillGroup::ready();
        group.review = Ok(ReviewOutcome::NotFound {
            skills_dir: "/ws/skills".to_string(),
            global_dir: "/home/u/.codewhale/skills".to_string(),
            warnings: vec!["one warning".to_string()],
        });
        let result = review(&mut group, Some("file.rs"));
        assert!(result.is_error);
        let msg = result.message.unwrap();
        assert!(
            msg.contains(
                "Review skill not found in /ws/skills or /home/u/.codewhale/skills. Create ~/.codewhale/skills/review/SKILL.md."
            ),
            "{msg}"
        );
        assert!(msg.contains("Warnings:\n- one warning"), "{msg}");
    }

    #[test]
    fn review_missing_facet_errors_are_safe() {
        let result = review_contextual(CommandContexts::empty(), Some("file.rs"));
        assert!(result.is_error);
        assert_eq!(
            result.message.unwrap(),
            "Error: Command capability unavailable: skill_group"
        );
    }
}
