//! Generic handler transport for staged command migration.
//!
//! The output type is generic so FEAT-014 does not move or duplicate the
//! TUI-owned `CommandResult`. During in-place adoption, the TUI instantiates
//! `CommandHandler<crate::commands::CommandResult>`.

use crate::facets::{
    CommandCostContext, CommandMediaContext, CommandMemoryContext, CommandModePolicyContext,
    CommandModelContext, CommandPresentationContext, CommandProjectContext, CommandSessionContext,
    CommandSkillGroupContext, CommandSkillsContext, CommandSystemPromptContext,
    CommandWorkspaceContext,
};

/// Exact host capabilities exposed to one contextual command handler.
///
/// The set lives in the external contract crate so command registrations can
/// declare least authority without naming the TUI host. The dispatcher uses
/// the declaration to populate only those slots in [`CommandContexts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CommandCapabilities(u16);

impl CommandCapabilities {
    pub const NONE: Self = Self(0);
    pub const SESSION: Self = Self(1 << 0);
    pub const MODEL: Self = Self(1 << 1);
    pub const COST: Self = Self(1 << 2);
    pub const MODE_POLICY: Self = Self(1 << 3);
    pub const SYSTEM_PROMPT: Self = Self(1 << 4);
    pub const SKILLS: Self = Self(1 << 5);
    pub const WORKSPACE: Self = Self(1 << 6);
    pub const PRESENTATION: Self = Self(1 << 7);
    pub const MEDIA: Self = Self(1 << 8);
    /// Memory-group host data (FEAT-019 D1).
    pub const MEMORY: Self = Self(1 << 9);
    /// Project-group host data (FEAT-021 D1).
    pub const PROJECT: Self = Self(1 << 10);
    /// Skills-group host data (FEAT-022 D1).
    pub const SKILL_GROUP: Self = Self(1 << 11);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, capability: Self) -> bool {
        !capability.is_empty() && self.0 & capability.0 == capability.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for CommandCapabilities {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// A command handler that is either argument-only or capability-scoped.
#[derive(Clone, Copy)]
pub enum CommandHandler<R> {
    Pure(fn(Option<&str>) -> R),
    Contextual {
        capabilities: CommandCapabilities,
        handler: fn(CommandContexts<'_>, Option<&str>) -> R,
    },
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
    memory: Option<&'a mut dyn CommandMemoryContext>,
    project: Option<&'a mut dyn CommandProjectContext>,
    skill_group: Option<&'a mut dyn CommandSkillGroupContext>,
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
    pub memory: Option<&'a mut dyn CommandMemoryContext>,
    pub project: Option<&'a mut dyn CommandProjectContext>,
    pub skill_group: Option<&'a mut dyn CommandSkillGroupContext>,
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
            memory: None,
            project: None,
            skill_group: None,
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
            memory: self.memory,
            project: self.project,
            skill_group: self.skill_group,
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

    pub fn with_memory(mut self, value: &'a mut dyn CommandMemoryContext) -> Self {
        assert!(
            self.memory.replace(value).is_none(),
            "memory facet already set"
        );
        self
    }

    pub fn with_project(mut self, value: &'a mut dyn CommandProjectContext) -> Self {
        assert!(
            self.project.replace(value).is_none(),
            "project facet already set"
        );
        self
    }

    pub fn with_skill_group(mut self, value: &'a mut dyn CommandSkillGroupContext) -> Self {
        assert!(
            self.skill_group.replace(value).is_none(),
            "skill-group facet already set"
        );
        self
    }
}

impl Default for CommandContexts<'_> {
    fn default() -> Self {
        Self::empty()
    }
}
