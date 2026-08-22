//! Generic handler transport for staged command migration.
//!
//! The output type is generic so FEAT-014 does not move or duplicate the
//! TUI-owned `CommandResult`. During in-place adoption, the TUI instantiates
//! `CommandHandler<crate::commands::CommandResult>`.

use crate::facets::{
    CommandCostContext, CommandMediaContext, CommandModePolicyContext, CommandModelContext,
    CommandPresentationContext, CommandSessionContext, CommandSkillsContext,
    CommandSystemPromptContext, CommandWorkspaceContext,
};

/// A command handler that is either argument-only or capability-scoped.
#[derive(Clone, Copy)]
pub enum CommandHandler<R> {
    Pure(fn(Option<&str>) -> R),
    Contextual(fn(CommandContexts<'_>, Option<&str>) -> R),
}

/// Transport envelope with one independently optional facet slot.
pub struct CommandContexts<'a> {
    session: Option<&'a mut dyn CommandSessionContext>,
    model: Option<&'a mut dyn CommandModelContext>,
    cost: Option<&'a mut dyn CommandCostContext>,
    mode_policy: Option<&'a mut dyn CommandModePolicyContext>,
    system_prompt: Option<&'a mut dyn CommandSystemPromptContext>,
    skills: Option<&'a mut dyn CommandSkillsContext>,
    workspace: Option<&'a mut dyn CommandWorkspaceContext>,
    presentation: Option<&'a mut dyn CommandPresentationContext>,
    media: Option<&'a mut dyn CommandMediaContext>,
}

/// Consumed envelope used when one handler needs several independent facets.
pub struct ContextParts<'a> {
    pub session: Option<&'a mut dyn CommandSessionContext>,
    pub model: Option<&'a mut dyn CommandModelContext>,
    pub cost: Option<&'a mut dyn CommandCostContext>,
    pub mode_policy: Option<&'a mut dyn CommandModePolicyContext>,
    pub system_prompt: Option<&'a mut dyn CommandSystemPromptContext>,
    pub skills: Option<&'a mut dyn CommandSkillsContext>,
    pub workspace: Option<&'a mut dyn CommandWorkspaceContext>,
    pub presentation: Option<&'a mut dyn CommandPresentationContext>,
    pub media: Option<&'a mut dyn CommandMediaContext>,
}

impl<'a> CommandContexts<'a> {
    pub fn empty() -> Self {
        Self {
            session: None,
            model: None,
            cost: None,
            mode_policy: None,
            system_prompt: None,
            skills: None,
            workspace: None,
            presentation: None,
            media: None,
        }
    }

    pub fn into_parts(self) -> ContextParts<'a> {
        ContextParts {
            session: self.session,
            model: self.model,
            cost: self.cost,
            mode_policy: self.mode_policy,
            system_prompt: self.system_prompt,
            skills: self.skills,
            workspace: self.workspace,
            presentation: self.presentation,
            media: self.media,
        }
    }

    pub fn with_session(mut self, value: &'a mut dyn CommandSessionContext) -> Self {
        assert!(
            self.session.replace(value).is_none(),
            "session facet already set"
        );
        self
    }

    pub fn with_model(mut self, value: &'a mut dyn CommandModelContext) -> Self {
        assert!(
            self.model.replace(value).is_none(),
            "model facet already set"
        );
        self
    }

    pub fn with_cost(mut self, value: &'a mut dyn CommandCostContext) -> Self {
        assert!(self.cost.replace(value).is_none(), "cost facet already set");
        self
    }

    pub fn with_mode_policy(mut self, value: &'a mut dyn CommandModePolicyContext) -> Self {
        assert!(
            self.mode_policy.replace(value).is_none(),
            "mode-policy facet already set"
        );
        self
    }

    pub fn with_system_prompt(mut self, value: &'a mut dyn CommandSystemPromptContext) -> Self {
        assert!(
            self.system_prompt.replace(value).is_none(),
            "system-prompt facet already set"
        );
        self
    }

    pub fn with_skills(mut self, value: &'a mut dyn CommandSkillsContext) -> Self {
        assert!(
            self.skills.replace(value).is_none(),
            "skills facet already set"
        );
        self
    }

    pub fn with_workspace(mut self, value: &'a mut dyn CommandWorkspaceContext) -> Self {
        assert!(
            self.workspace.replace(value).is_none(),
            "workspace facet already set"
        );
        self
    }

    pub fn with_presentation(mut self, value: &'a mut dyn CommandPresentationContext) -> Self {
        assert!(
            self.presentation.replace(value).is_none(),
            "presentation facet already set"
        );
        self
    }

    pub fn with_media(mut self, value: &'a mut dyn CommandMediaContext) -> Self {
        assert!(
            self.media.replace(value).is_none(),
            "media facet already set"
        );
        self
    }
}

impl Default for CommandContexts<'_> {
    fn default() -> Self {
        Self::empty()
    }
}
