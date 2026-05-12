//! Lightweight localization registry for high-visibility TUI strings.
//!
//! This intentionally covers UI chrome only. It does not change model prompts,
//! model output language, provider behavior, or media payload semantics.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    Ltr,
    Rtl,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocaleCoverage {
    English,
    V076Core,
    PlannedQa,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocaleSpec {
    pub tag: &'static str,
    pub display_name: &'static str,
    pub script: &'static str,
    pub direction: TextDirection,
    pub fallback: &'static str,
    pub coverage: LocaleCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    En,
    Ja,
    ZhHans,
    PtBr,
}

impl Locale {
    pub fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ja => "ja",
            Self::ZhHans => "zh-Hans",
            Self::PtBr => "pt-BR",
        }
    }

    #[allow(dead_code)]
    pub fn spec(self) -> LocaleSpec {
        match self {
            Self::En => LocaleSpec {
                tag: "en",
                display_name: "English",
                script: "Latin",
                direction: TextDirection::Ltr,
                fallback: "en",
                coverage: LocaleCoverage::English,
            },
            Self::Ja => LocaleSpec {
                tag: "ja",
                display_name: "Japanese",
                script: "Jpan",
                direction: TextDirection::Ltr,
                fallback: "en",
                coverage: LocaleCoverage::V076Core,
            },
            Self::ZhHans => LocaleSpec {
                tag: "zh-Hans",
                display_name: "Chinese Simplified",
                script: "Hans",
                direction: TextDirection::Ltr,
                fallback: "en",
                coverage: LocaleCoverage::V076Core,
            },
            Self::PtBr => LocaleSpec {
                tag: "pt-BR",
                display_name: "Portuguese (Brazil)",
                script: "Latin",
                direction: TextDirection::Ltr,
                fallback: "en",
                coverage: LocaleCoverage::V076Core,
            },
        }
    }

    #[allow(dead_code)]
    pub fn shipped() -> &'static [Self] {
        &[Self::En, Self::Ja, Self::ZhHans, Self::PtBr]
    }
}

#[allow(dead_code)]
pub const PLANNED_QA_LOCALES: &[LocaleSpec] = &[
    LocaleSpec {
        tag: "ar",
        display_name: "Arabic",
        script: "Arab",
        direction: TextDirection::Rtl,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "hi",
        display_name: "Hindi",
        script: "Deva",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "bn",
        display_name: "Bengali",
        script: "Beng",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "id",
        display_name: "Indonesian",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "vi",
        display_name: "Vietnamese",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "sw",
        display_name: "Swahili",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "ha",
        display_name: "Hausa",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "yo",
        display_name: "Yoruba",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "es-419",
        display_name: "Spanish (Latin America)",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "fr",
        display_name: "French",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
    LocaleSpec {
        tag: "fil",
        display_name: "Filipino/Tagalog",
        script: "Latin",
        direction: TextDirection::Ltr,
        fallback: "en",
        coverage: LocaleCoverage::PlannedQa,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageId {
    ComposerPlaceholder,
    HistorySearchPlaceholder,
    HistorySearchTitle,
    HistoryHintMove,
    HistoryHintAccept,
    HistoryHintRestore,
    HistoryNoMatches,
    ConfigTitle,
    ConfigModalTitle,
    ConfigSearchPlaceholder,
    ConfigNoSettings,
    ConfigNoMatchesPrefix,
    ConfigFilteredSettings,
    ConfigShowing,
    ConfigFooterDefault,
    ConfigFooterScrollable,
    ConfigFooterFiltered,
    HelpTitle,
    HelpFilterPlaceholder,
    HelpFilterPrefix,
    HelpNoMatches,
    HelpSlashCommands,
    HelpKeybindings,
    HelpFooterTypeFilter,
    HelpFooterMove,
    HelpFooterJump,
    HelpFooterClose,
    CmdAttachDescription,
    CmdAnchorDescription,
    CmdCacheDescription,
    CmdClearDescription,
    CmdCompactDescription,
    CmdConfigDescription,
    CmdContextDescription,
    CmdCostDescription,
    CmdCycleDescription,
    CmdCyclesDescription,
    CmdDiffDescription,
    CmdEditDescription,
    CmdExitDescription,
    CmdExportDescription,
    CmdFeedbackDescription,
    CmdHelpDescription,
    CmdHomeDescription,
    CmdHooksDescription,
    CmdGoalDescription,
    CmdInitDescription,
    CmdJobsDescription,
    CmdLinksDescription,
    CmdLoadDescription,
    CmdLogoutDescription,
    CmdMcpDescription,
    CmdMemoryDescription,
    CmdModeDescription,
    CmdModelDescription,
    CmdModelsDescription,
    CmdNetworkDescription,
    CmdNoteDescription,
    CmdThemeDescription,
    CmdProviderDescription,
    CmdQueueDescription,
    CmdRecallDescription,
    CmdRenameDescription,
    CmdRestoreDescription,
    CmdRetryDescription,
    CmdReviewDescription,
    CmdRlmDescription,
    CmdSaveDescription,
    CmdSessionsDescription,
    CmdSettingsDescription,
    CmdSkillDescription,
    CmdSkillsDescription,
    CmdStashDescription,
    CmdStatusDescription,
    CmdStatuslineDescription,
    CmdSubagentsDescription,
    CmdSwarmDescription,
    CmdSystemDescription,
    CmdTaskDescription,
    CmdTokensDescription,
    CmdTrustDescription,
    CmdLspDescription,
    CmdShareDescription,
    CmdUndoDescription,
    CmdVerboseDescription,
    CmdCacheAdvice,
    CmdCacheFootnote,
    CmdCacheHeader,
    CmdCacheNoData,
    CmdCacheTotals,
    CmdCostReport,
    CmdTokensCacheBoth,
    CmdTokensCacheHitOnly,
    CmdTokensCacheMissOnly,
    CmdTokensContextUnknownWindow,
    CmdTokensContextWithWindow,
    CmdTokensNotReported,
    CmdTokensReport,
    FooterAgentSingular,
    FooterAgentsPlural,
    FooterPressCtrlCAgain,
    FooterWorking,
    HelpSectionActions,
    HelpSectionClipboard,
    HelpSectionEditing,
    HelpSectionHelp,
    HelpSectionModes,
    HelpSectionNavigation,
    HelpSectionSessions,
    KbScrollTranscript,
    KbNavigateHistory,
    KbScrollTranscriptAlt,
    KbScrollPage,
    KbJumpTopBottom,
    KbJumpTopBottomEmpty,
    KbJumpToolBlocks,
    KbMoveCursor,
    KbJumpLineStartEnd,
    KbDeleteChar,
    KbClearDraft,
    KbStashDraft,
    KbSearchHistory,
    KbInsertNewline,
    KbSendDraft,
    KbCloseMenu,
    KbCancelOrExit,
    KbShellControls,
    KbExitEmpty,
    KbCommandPalette,
    KbFuzzyFilePicker,
    KbCompactInspector,
    KbLastMessagePager,
    KbSelectedDetails,
    KbToolDetailsPager,
    KbThinkingPager,
    KbLiveTranscript,
    KbBacktrackMessage,
    KbCompleteCycleModes,
    KbJumpPlanAgentYolo,
    KbAltJumpPlanAgentYolo,
    KbFocusSidebar,
    KbTogglePlanAgent,
    KbSessionPicker,
    KbPasteAttach,
    KbCopySelection,
    KbContextMenu,
    KbAttachPath,
    KbHelpOverlay,
    KbToggleHelp,
    KbToggleHelpSlash,
    HelpUsageLabel,
    HelpAliasesLabel,
    SettingsTitle,
    SettingsConfigFile,
    ClearConversation,
    ClearConversationBusy,
    ModelChanged,
    LinksTitle,
    LinksDashboard,
    LinksDocs,
    LinksTip,
    SubagentsFetching,
    HelpUnknownCommand,
    HomeDashboardTitle,
    HomeModel,
    HomeMode,
    HomeWorkspace,
    HomeHistory,
    HomeTokens,
    HomeQueued,
    HomeSubagents,
    HomeSkill,
    HomeQuickActions,
    HomeQuickLinks,
    HomeQuickSkills,
    HomeQuickConfig,
    HomeQuickSettings,
    HomeQuickModel,
    HomeQuickSubagents,
    HomeQuickTaskList,
    HomeQuickHelp,
    HomeModeTips,
    HomeAgentModeTip,
    HomeAgentModeReviewTip,
    HomeAgentModeYoloTip,
    HomeYoloModeTip,
    HomeYoloModeCaution,
    HomePlanModeTip,
    HomePlanModeChecklistTip,
    // Onboarding screens — language picker.
    OnboardLanguageTitle,
    OnboardLanguageBlurb,
    OnboardLanguageFooter,
    // Onboarding screens — API key entry.
    OnboardApiKeyTitle,
    OnboardApiKeyStep1,
    OnboardApiKeyStep2,
    OnboardApiKeySavedHint,
    OnboardApiKeyFormatHint,
    OnboardApiKeyPlaceholder,
    OnboardApiKeyLabel,
    OnboardApiKeyFooter,
    // Onboarding screens — workspace trust prompt.
    OnboardTrustTitle,
    OnboardTrustQuestion,
    OnboardTrustLocationPrefix,
    OnboardTrustRiskHint,
    OnboardTrustEffectHint,
    OnboardTrustFooterPrefix,
    OnboardTrustFooterMiddle,
    OnboardTrustFooterSuffix,
    // Onboarding screens — final tips screen.
    OnboardTipsTitle,
    OnboardTipsLine1,
    OnboardTipsLine2,
    OnboardTipsLine3,
    OnboardTipsLine4,
    OnboardTipsFooterEnter,
    OnboardTipsFooterAction,
}

#[allow(dead_code)]
pub const ALL_MESSAGE_IDS: &[MessageId] = &[
    MessageId::ComposerPlaceholder,
    MessageId::HistorySearchPlaceholder,
    MessageId::HistorySearchTitle,
    MessageId::HistoryHintMove,
    MessageId::HistoryHintAccept,
    MessageId::HistoryHintRestore,
    MessageId::HistoryNoMatches,
    MessageId::ConfigTitle,
    MessageId::ConfigModalTitle,
    MessageId::ConfigSearchPlaceholder,
    MessageId::ConfigNoSettings,
    MessageId::ConfigNoMatchesPrefix,
    MessageId::ConfigFilteredSettings,
    MessageId::ConfigShowing,
    MessageId::ConfigFooterDefault,
    MessageId::ConfigFooterScrollable,
    MessageId::ConfigFooterFiltered,
    MessageId::HelpTitle,
    MessageId::HelpFilterPlaceholder,
    MessageId::HelpFilterPrefix,
    MessageId::HelpNoMatches,
    MessageId::HelpSlashCommands,
    MessageId::HelpKeybindings,
    MessageId::HelpFooterTypeFilter,
    MessageId::HelpFooterMove,
    MessageId::HelpFooterJump,
    MessageId::HelpFooterClose,
    MessageId::CmdAnchorDescription,
    MessageId::CmdAttachDescription,
    MessageId::CmdCacheDescription,
    MessageId::CmdClearDescription,
    MessageId::CmdCompactDescription,
    MessageId::CmdConfigDescription,
    MessageId::CmdContextDescription,
    MessageId::CmdCostDescription,
    MessageId::CmdCycleDescription,
    MessageId::CmdCyclesDescription,
    MessageId::CmdDiffDescription,
    MessageId::CmdEditDescription,
    MessageId::CmdExitDescription,
    MessageId::CmdExportDescription,
    MessageId::CmdFeedbackDescription,
    MessageId::CmdHelpDescription,
    MessageId::CmdHomeDescription,
    MessageId::CmdHooksDescription,
    MessageId::CmdInitDescription,
    MessageId::CmdJobsDescription,
    MessageId::CmdLinksDescription,
    MessageId::CmdLoadDescription,
    MessageId::CmdLogoutDescription,
    MessageId::CmdMcpDescription,
    MessageId::CmdMemoryDescription,
    MessageId::CmdModeDescription,
    MessageId::CmdModelDescription,
    MessageId::CmdModelsDescription,
    MessageId::CmdNetworkDescription,
    MessageId::CmdNoteDescription,
    MessageId::CmdProviderDescription,
    MessageId::CmdQueueDescription,
    MessageId::CmdRecallDescription,
    MessageId::CmdRenameDescription,
    MessageId::CmdRestoreDescription,
    MessageId::CmdRetryDescription,
    MessageId::CmdReviewDescription,
    MessageId::CmdRlmDescription,
    MessageId::CmdSaveDescription,
    MessageId::CmdSessionsDescription,
    MessageId::CmdSettingsDescription,
    MessageId::CmdSkillDescription,
    MessageId::CmdSkillsDescription,
    MessageId::CmdStashDescription,
    MessageId::CmdStatusDescription,
    MessageId::CmdStatuslineDescription,
    MessageId::CmdSubagentsDescription,
    MessageId::CmdSwarmDescription,
    MessageId::CmdSystemDescription,
    MessageId::CmdTaskDescription,
    MessageId::CmdTokensDescription,
    MessageId::CmdTrustDescription,
    MessageId::CmdLspDescription,
    MessageId::CmdShareDescription,
    MessageId::CmdUndoDescription,
    MessageId::CmdVerboseDescription,
    MessageId::CmdCacheAdvice,
    MessageId::CmdCacheFootnote,
    MessageId::CmdCacheHeader,
    MessageId::CmdCacheNoData,
    MessageId::CmdCacheTotals,
    MessageId::CmdCostReport,
    MessageId::CmdTokensCacheBoth,
    MessageId::CmdTokensCacheHitOnly,
    MessageId::CmdTokensCacheMissOnly,
    MessageId::CmdTokensContextUnknownWindow,
    MessageId::CmdTokensContextWithWindow,
    MessageId::CmdTokensNotReported,
    MessageId::CmdTokensReport,
    MessageId::FooterAgentSingular,
    MessageId::FooterAgentsPlural,
    MessageId::FooterPressCtrlCAgain,
    MessageId::FooterWorking,
    MessageId::HelpSectionActions,
    MessageId::HelpSectionClipboard,
    MessageId::HelpSectionEditing,
    MessageId::HelpSectionHelp,
    MessageId::HelpSectionModes,
    MessageId::HelpSectionNavigation,
    MessageId::HelpSectionSessions,
    MessageId::KbScrollTranscript,
    MessageId::KbNavigateHistory,
    MessageId::KbScrollTranscriptAlt,
    MessageId::KbScrollPage,
    MessageId::KbJumpTopBottom,
    MessageId::KbJumpTopBottomEmpty,
    MessageId::KbJumpToolBlocks,
    MessageId::KbMoveCursor,
    MessageId::KbJumpLineStartEnd,
    MessageId::KbDeleteChar,
    MessageId::KbClearDraft,
    MessageId::KbStashDraft,
    MessageId::KbSearchHistory,
    MessageId::KbInsertNewline,
    MessageId::KbSendDraft,
    MessageId::KbCloseMenu,
    MessageId::KbCancelOrExit,
    MessageId::KbShellControls,
    MessageId::KbExitEmpty,
    MessageId::KbCommandPalette,
    MessageId::KbFuzzyFilePicker,
    MessageId::KbCompactInspector,
    MessageId::KbLastMessagePager,
    MessageId::KbSelectedDetails,
    MessageId::KbToolDetailsPager,
    MessageId::KbThinkingPager,
    MessageId::KbLiveTranscript,
    MessageId::KbBacktrackMessage,
    MessageId::KbCompleteCycleModes,
    MessageId::KbJumpPlanAgentYolo,
    MessageId::KbAltJumpPlanAgentYolo,
    MessageId::KbFocusSidebar,
    MessageId::KbTogglePlanAgent,
    MessageId::KbSessionPicker,
    MessageId::KbPasteAttach,
    MessageId::KbCopySelection,
    MessageId::KbContextMenu,
    MessageId::KbAttachPath,
    MessageId::KbHelpOverlay,
    MessageId::KbToggleHelp,
    MessageId::KbToggleHelpSlash,
    MessageId::HelpUsageLabel,
    MessageId::HelpAliasesLabel,
    MessageId::SettingsTitle,
    MessageId::SettingsConfigFile,
    MessageId::ClearConversation,
    MessageId::ClearConversationBusy,
    MessageId::ModelChanged,
    MessageId::LinksTitle,
    MessageId::LinksDashboard,
    MessageId::LinksDocs,
    MessageId::LinksTip,
    MessageId::SubagentsFetching,
    MessageId::HelpUnknownCommand,
    MessageId::HomeDashboardTitle,
    MessageId::HomeModel,
    MessageId::HomeMode,
    MessageId::HomeWorkspace,
    MessageId::HomeHistory,
    MessageId::HomeTokens,
    MessageId::HomeQueued,
    MessageId::HomeSubagents,
    MessageId::HomeSkill,
    MessageId::HomeQuickActions,
    MessageId::HomeQuickLinks,
    MessageId::HomeQuickSkills,
    MessageId::HomeQuickConfig,
    MessageId::HomeQuickSettings,
    MessageId::HomeQuickModel,
    MessageId::HomeQuickSubagents,
    MessageId::HomeQuickTaskList,
    MessageId::HomeQuickHelp,
    MessageId::HomeModeTips,
    MessageId::HomeAgentModeTip,
    MessageId::HomeAgentModeReviewTip,
    MessageId::HomeAgentModeYoloTip,
    MessageId::HomeYoloModeTip,
    MessageId::HomeYoloModeCaution,
    MessageId::HomePlanModeTip,
    MessageId::HomePlanModeChecklistTip,
    MessageId::OnboardLanguageTitle,
    MessageId::OnboardLanguageBlurb,
    MessageId::OnboardLanguageFooter,
    MessageId::OnboardApiKeyTitle,
    MessageId::OnboardApiKeyStep1,
    MessageId::OnboardApiKeyStep2,
    MessageId::OnboardApiKeySavedHint,
    MessageId::OnboardApiKeyFormatHint,
    MessageId::OnboardApiKeyPlaceholder,
    MessageId::OnboardApiKeyLabel,
    MessageId::OnboardApiKeyFooter,
    MessageId::OnboardTrustTitle,
    MessageId::OnboardTrustQuestion,
    MessageId::OnboardTrustLocationPrefix,
    MessageId::OnboardTrustRiskHint,
    MessageId::OnboardTrustEffectHint,
    MessageId::OnboardTrustFooterPrefix,
    MessageId::OnboardTrustFooterMiddle,
    MessageId::OnboardTrustFooterSuffix,
    MessageId::OnboardTipsTitle,
    MessageId::OnboardTipsLine1,
    MessageId::OnboardTipsLine2,
    MessageId::OnboardTipsLine3,
    MessageId::OnboardTipsLine4,
    MessageId::OnboardTipsFooterEnter,
    MessageId::OnboardTipsFooterAction,
];

pub fn tr(locale: Locale, id: MessageId) -> &'static str {
    fallback_translation(translation(locale, id), id)
}

#[allow(dead_code)]
pub fn missing_message_ids(locale: Locale) -> Vec<MessageId> {
    ALL_MESSAGE_IDS
        .iter()
        .copied()
        .filter(|id| translation(locale, *id).is_none())
        .collect()
}

pub fn normalize_configured_locale(input: &str) -> Option<&'static str> {
    let normalized = normalize_locale_input(input);
    if matches!(normalized.as_str(), "" | "auto" | "system") {
        return Some("auto");
    }
    parse_locale(&normalized).map(Locale::tag)
}

pub fn resolve_locale(setting: &str) -> Locale {
    resolve_locale_with_env(setting, |key| std::env::var(key).ok())
}

pub fn resolve_locale_with_env<F>(setting: &str, env: F) -> Locale
where
    F: Fn(&str) -> Option<String>,
{
    let normalized = normalize_locale_input(setting);
    if !matches!(normalized.as_str(), "" | "auto" | "system") {
        return parse_locale(&normalized).unwrap_or(Locale::En);
    }

    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(value) = env(key)
            && let Some(locale) = parse_locale(&normalize_locale_input(&value))
        {
            return locale;
        }
    }

    Locale::En
}

#[allow(dead_code)]
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }

    let ellipsis_width = '…'.width().unwrap_or(1);
    if max_width <= ellipsis_width {
        return "…".to_string();
    }

    let limit = max_width - ellipsis_width;
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

fn normalize_locale_input(input: &str) -> String {
    input
        .split('.')
        .next()
        .unwrap_or(input)
        .split('@')
        .next()
        .unwrap_or(input)
        .trim()
        .replace('_', "-")
        .to_lowercase()
}

fn parse_locale(value: &str) -> Option<Locale> {
    if value == "c" || value == "posix" || value.starts_with("en") {
        return Some(Locale::En);
    }
    if value.starts_with("ja") {
        return Some(Locale::Ja);
    }
    if value.starts_with("zh") {
        if value.contains("hant")
            || value.contains("-tw")
            || value.contains("-hk")
            || value.contains("-mo")
        {
            return None;
        }
        return Some(Locale::ZhHans);
    }
    if value.starts_with("pt") || value == "br" {
        return Some(Locale::PtBr);
    }
    None
}

fn fallback_translation(candidate: Option<&'static str>, id: MessageId) -> &'static str {
    candidate.unwrap_or_else(|| english(id))
}

fn english(id: MessageId) -> &'static str {
    match id {
        MessageId::ComposerPlaceholder => "Write a task or use /.",
        MessageId::HistorySearchPlaceholder => "Search prompt history...",
        MessageId::HistorySearchTitle => "History Search",
        MessageId::HistoryHintMove => "Up/Down move",
        MessageId::HistoryHintAccept => "Enter accept",
        MessageId::HistoryHintRestore => "Esc restore",
        MessageId::HistoryNoMatches => "  No matches",
        MessageId::ConfigTitle => "Session Configuration",
        MessageId::ConfigModalTitle => " Config ",
        MessageId::ConfigSearchPlaceholder => "type to filter",
        MessageId::ConfigNoSettings => "  No settings available.",
        MessageId::ConfigNoMatchesPrefix => "  No settings match ",
        MessageId::ConfigFilteredSettings => "  Filtered settings",
        MessageId::ConfigShowing => "  Showing",
        MessageId::ConfigFooterDefault => {
            " type=filter, Up/Down=select, Enter/e=edit, Esc/q=close "
        }
        MessageId::ConfigFooterScrollable => {
            " type=filter, Up/Down=select, Enter/e=edit, PgUp/PgDn=scroll, Esc/q=close "
        }
        MessageId::ConfigFooterFiltered => {
            " type=filter, Backspace=delete, Ctrl+U/Esc=clear, Enter=edit "
        }
        MessageId::HelpTitle => "Help",
        MessageId::HelpFilterPlaceholder => "Type to filter",
        MessageId::HelpFilterPrefix => "Filter: ",
        MessageId::HelpNoMatches => "  No matches.",
        MessageId::HelpSlashCommands => "Slash commands",
        MessageId::HelpKeybindings => "Keybindings",
        MessageId::HelpFooterTypeFilter => " type to filter ",
        MessageId::HelpFooterMove => "  Up/Down move ",
        MessageId::HelpFooterJump => " PgUp/PgDn jump ",
        MessageId::HelpFooterClose => " Esc close ",
        MessageId::CmdAnchorDescription => {
            "Pin a fact that survives compaction (auto-injected into context)"
        }
        MessageId::CmdAttachDescription => {
            "Attach image/video media; use @path for text files or directories"
        }
        MessageId::CmdCacheDescription => {
            "Show DeepSeek prefix-cache hit/miss stats for the last N turns"
        }
        MessageId::CmdClearDescription => "Clear conversation history",
        MessageId::CmdCompactDescription => {
            "Trigger context compaction to free up space (legacy; v0.6.6 prefers cycle restart)"
        }
        MessageId::CmdConfigDescription => "Open interactive configuration editor",
        MessageId::CmdContextDescription => "Open compact session context inspector",
        MessageId::CmdCostDescription => "Show session cost breakdown",
        MessageId::CmdCycleDescription => "Show the carry-forward briefing for a specific cycle",
        MessageId::CmdCyclesDescription => "List checkpoint-restart cycle handoffs in this session",
        MessageId::CmdDiffDescription => "Show file changes since session start",
        MessageId::CmdEditDescription => "Revise and resubmit the last message",
        MessageId::CmdExitDescription => "Exit the application",
        MessageId::CmdExportDescription => "Export conversation to markdown",
        MessageId::CmdFeedbackDescription => "Generate a GitHub feedback URL",
        MessageId::CmdHelpDescription => "Show help information",
        MessageId::CmdHomeDescription => "Show home dashboard with stats and quick actions",
        MessageId::CmdHooksDescription => "List configured lifecycle hooks (read-only)",
        MessageId::CmdGoalDescription => "Set a session goal with optional token budget",
        MessageId::CmdInitDescription => "Generate AGENTS.md for project",
        MessageId::CmdLspDescription => "Toggle LSP diagnostics on or off",
        MessageId::CmdShareDescription => "Export current session as a shareable web URL",
        MessageId::CmdJobsDescription => "Inspect and control background shell jobs",
        MessageId::CmdLinksDescription => "Show DeepSeek dashboard and docs links",
        MessageId::CmdLoadDescription => "Load session from file",
        MessageId::CmdLogoutDescription => "Clear API key and return to setup",
        MessageId::CmdMcpDescription => "Open or manage MCP servers",
        MessageId::CmdMemoryDescription => "Inspect or manage the persistent user-memory file",
        MessageId::CmdModeDescription => {
            "Switch mode or open picker: /mode [agent|plan|yolo|1|2|3]"
        }
        MessageId::CmdModelDescription => "Switch or view current model",
        MessageId::CmdModelsDescription => "List available models from API",
        MessageId::CmdNetworkDescription => "Manage network allow and deny rules",
        MessageId::CmdNoteDescription => "Add, list, edit, or remove workspace notes",
        MessageId::CmdThemeDescription => "Toggle between dark and light theme",
        MessageId::CmdProviderDescription => {
            "Switch or view the active LLM backend (deepseek | nvidia-nim | ollama)"
        }
        MessageId::CmdQueueDescription => "View or edit queued messages",
        MessageId::CmdRecallDescription => "Search prior cycle archives (BM25 over message text)",
        MessageId::CmdRenameDescription => "Rename the current session",
        MessageId::CmdRestoreDescription => {
            "Roll back the workspace to a prior pre/post-turn snapshot. With no arg, lists recent snapshots."
        }
        MessageId::CmdRetryDescription => "Retry the last request",
        MessageId::CmdReviewDescription => "Run a structured code review on a file, diff, or PR",
        MessageId::CmdRlmDescription => {
            "Recursive Language Model (RLM) turn — store the prompt in a Python REPL and let the model write code to process it, with `llm_query()` / `sub_rlm()` for sub-LLM calls."
        }
        MessageId::CmdSaveDescription => "Save session to file",
        MessageId::CmdSessionsDescription => "Open session picker",
        MessageId::CmdSettingsDescription => "Show persistent settings",
        MessageId::CmdSkillDescription => {
            "Activate a skill, or install/update/uninstall/trust a community skill"
        }
        MessageId::CmdSkillsDescription => {
            "List local skills (filter by `/skills <prefix>`; --remote browses the curated registry)"
        }
        MessageId::CmdStashDescription => {
            "Park or restore a composer draft (Ctrl+S to push, /stash list/pop)"
        }
        MessageId::CmdStatusDescription => "Show runtime session status",
        MessageId::CmdStatuslineDescription => "Configure which items appear in the footer",
        MessageId::CmdSubagentsDescription => "List sub-agent status",
        MessageId::CmdSwarmDescription => {
            "Run a multi-agent fanout turn (sequential | mixture | distill | deliberate)"
        }
        MessageId::CmdSystemDescription => "Show current system prompt",
        MessageId::CmdTaskDescription => "Manage background tasks",
        MessageId::CmdTokensDescription => "Show token usage for session",
        MessageId::CmdTrustDescription => {
            "Manage workspace trust and per-path allowlist (`/trust add <path>`, `/trust list`, `/trust on|off`)"
        }
        MessageId::CmdUndoDescription => "Remove last message pair",
        MessageId::CmdVerboseDescription => "Toggle full live thinking in the transcript",
        MessageId::CmdCacheAdvice => {
            "Hit/miss ratios over ~70% after the third turn indicate a stable cache prefix; \n\
             lower than that on long sessions suggests prefix churn worth investigating (#263)."
        }
        MessageId::CmdCacheFootnote => {
            "* miss inferred from input − hit when the provider did not report it explicitly.\n"
        }
        MessageId::CmdCacheHeader => {
            "Cache telemetry — last {count} of {total} turn(s) (model: {model})\n"
        }
        MessageId::CmdCacheNoData => {
            "Cache history: no turns recorded yet.\n\n\
             DeepSeek surfaces `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` \
             on every API turn that the model supports it (V4 family). Run a turn \
             and try /cache again."
        }
        MessageId::CmdCacheTotals => {
            "Σ in: {sum_in}   Σ hit: {sum_hit}   Σ miss: {sum_miss}   avg hit ratio: {avg}\n"
        }
        MessageId::CmdCostReport => {
            "Session Cost:\n\
             ─────────────────────────────\n\
             Approx total spent: {cost}\n\n\
             Cost estimates are approximate and use provider usage telemetry when available.\n\n\
             DeepSeek API Pricing:\n\
             ─────────────────────────────\n\
             Pricing details are not configured in this CLI."
        }
        MessageId::CmdTokensCacheBoth => "{hit} hit / {miss} miss",
        MessageId::CmdTokensCacheHitOnly => "{hit} hit / miss not reported",
        MessageId::CmdTokensCacheMissOnly => "hit not reported / {miss} miss",
        MessageId::CmdTokensContextUnknownWindow => "~{estimated} / unknown window",
        MessageId::CmdTokensContextWithWindow => "~{used} / {window} ({percent}%)",
        MessageId::FooterAgentSingular => "1 agent",
        MessageId::FooterAgentsPlural => "{count} agents",
        MessageId::FooterPressCtrlCAgain => "Press Ctrl+C again to quit",
        MessageId::FooterWorking => "working",
        MessageId::HelpSectionActions => "Actions",
        MessageId::HelpSectionClipboard => "Clipboard",
        MessageId::HelpSectionEditing => "Input editing",
        MessageId::HelpSectionHelp => "Help",
        MessageId::HelpSectionModes => "Modes",
        MessageId::HelpSectionNavigation => "Navigation",
        MessageId::HelpSectionSessions => "Sessions",
        MessageId::CmdTokensNotReported => "not reported",
        MessageId::CmdTokensReport => {
            "Token Usage:\n\
             ─────────────────────────────\n\
             Active context:        {active}\n\
             Last API input:        {input} (turn telemetry; may count repeated prefix across tool rounds)\n\
             Last API output:       {output}\n\
             Cache hit/miss:        {cache} (telemetry/cost only)\n\
             Cumulative tokens:     {total} (session usage telemetry)\n\
             Approx session cost:   {cost}\n\
             API messages:          {api_messages}\n\
             Chat messages:         {chat_messages}\n\
             Model:                 {model}"
        }
        MessageId::KbScrollTranscript => {
            "Scroll transcript, navigate input history, or select composer attachments"
        }
        MessageId::KbNavigateHistory => "Navigate input history",
        MessageId::KbScrollTranscriptAlt => "Scroll transcript",
        MessageId::KbScrollPage => "Scroll transcript by page",
        MessageId::KbJumpTopBottom => "Jump to top / bottom of transcript",
        MessageId::KbJumpTopBottomEmpty => "Jump to top / bottom (when input is empty)",
        MessageId::KbJumpToolBlocks => "Jump between tool output blocks",
        MessageId::KbMoveCursor => "Move cursor in composer",
        MessageId::KbJumpLineStartEnd => "Jump to start / end of line",
        MessageId::KbDeleteChar => {
            "Delete character before / after the cursor, or remove selected attachment"
        }
        MessageId::KbClearDraft => "Clear the current draft",
        MessageId::KbStashDraft => "Stash the current draft (`/stash pop` to restore)",
        MessageId::KbSearchHistory => "Search prompt history and recover local drafts",
        MessageId::KbInsertNewline => "Insert a newline in the composer",
        MessageId::KbSendDraft => "Send the current draft",
        MessageId::KbCloseMenu => "Close menu, cancel request, discard draft, or clear input",
        MessageId::KbCancelOrExit => "Cancel request, or exit when idle",
        MessageId::KbShellControls => "Open shell controls for a running foreground command",
        MessageId::KbExitEmpty => "Exit when input is empty",
        MessageId::KbCommandPalette => "Open the command palette",
        MessageId::KbFuzzyFilePicker => "Open the fuzzy file picker (insert @path on Enter)",
        MessageId::KbCompactInspector => "Open compact session context inspector",
        MessageId::KbLastMessagePager => "Open pager for the last message (when input is empty)",
        MessageId::KbSelectedDetails => {
            "Open details for the selected tool or message (when input is empty)"
        }
        MessageId::KbToolDetailsPager => "Open tool-details pager",
        MessageId::KbThinkingPager => "Open thinking pager",
        MessageId::KbLiveTranscript => "Open live transcript overlay (sticky-tail auto-scroll)",
        MessageId::KbBacktrackMessage => {
            "Backtrack to a previous user message (Left/Right step, Enter to rewind)"
        }
        MessageId::KbCompleteCycleModes => {
            "Complete /command, queue running-turn follow-up, cycle modes; Shift+Tab cycles reasoning effort"
        }
        MessageId::KbJumpPlanAgentYolo => "Jump directly to Plan / Agent / YOLO mode",
        MessageId::KbAltJumpPlanAgentYolo => "Alternative jump to Plan / Agent / YOLO mode",
        MessageId::KbFocusSidebar => "Focus Plan / Todos / Tasks / Agents / Auto sidebar",
        MessageId::KbTogglePlanAgent => "Toggle between Plan and Agent modes",
        MessageId::KbSessionPicker => "Open the session picker",
        MessageId::KbPasteAttach => "Paste text or attach a clipboard image",
        MessageId::KbCopySelection => "Copy the current selection (Cmd+C on macOS)",
        MessageId::KbContextMenu => {
            "Open context actions for paste, selection, message details, context, and help"
        }
        MessageId::KbAttachPath => "Add a local text file or directory to context",
        MessageId::KbHelpOverlay => "Open this help overlay (when input is empty)",
        MessageId::KbToggleHelp => "Toggle help overlay",
        MessageId::KbToggleHelpSlash => "Toggle help overlay",
        MessageId::HelpUsageLabel => "Usage:",
        MessageId::HelpAliasesLabel => "Aliases:",
        MessageId::SettingsTitle => "Settings:",
        MessageId::SettingsConfigFile => "Config file:",
        MessageId::ClearConversation => "Conversation cleared",
        MessageId::ClearConversationBusy => {
            "Conversation cleared (plan state busy; run /clear again if needed)"
        }
        MessageId::ModelChanged => "Model changed: {old} \u{2192} {new}",
        MessageId::LinksTitle => "DeepSeek Links:",
        MessageId::LinksDashboard => "Dashboard:",
        MessageId::LinksDocs => "Docs:",
        MessageId::LinksTip => "Tip: API keys are available in the dashboard console.",
        MessageId::SubagentsFetching => "Fetching sub-agent status...",
        MessageId::HelpUnknownCommand => "Unknown command: {topic}",
        MessageId::HomeDashboardTitle => "DeepSeek TUI Home Dashboard",
        MessageId::HomeModel => "Model:",
        MessageId::HomeMode => "Mode:",
        MessageId::HomeWorkspace => "Workspace:",
        MessageId::HomeHistory => "History:",
        MessageId::HomeTokens => "Tokens:",
        MessageId::HomeQueued => "Queued:",
        MessageId::HomeSubagents => "Sub-agents:",
        MessageId::HomeSkill => "Skill:",
        MessageId::HomeQuickActions => "Quick Actions",
        MessageId::HomeQuickLinks => "/links      - Dashboard & API links",
        MessageId::HomeQuickSkills => "/skills      - List available skills",
        MessageId::HomeQuickConfig => "/config      - Open interactive configuration editor",
        MessageId::HomeQuickSettings => "/settings    - Show persistent settings",
        MessageId::HomeQuickModel => "/model       - Switch or view model",
        MessageId::HomeQuickSubagents => "/subagents   - List sub-agent status",
        MessageId::HomeQuickTaskList => "/task list   - Show background task queue",
        MessageId::HomeQuickHelp => "/help        - Show help",
        MessageId::HomeModeTips => "Mode Tips",
        MessageId::HomeAgentModeTip => "Agent mode - Use tools for autonomous tasks",
        MessageId::HomeAgentModeReviewTip => "  Use Ctrl+X to review in Plan mode before executing",
        MessageId::HomeAgentModeYoloTip => "  Type /mode yolo to enable full tool access",
        MessageId::HomeYoloModeTip => "YOLO mode - Full tool access, no approvals",
        MessageId::HomeYoloModeCaution => "  Be careful with destructive operations!",
        MessageId::HomePlanModeTip => "Plan mode - Design before implementing",
        MessageId::HomePlanModeChecklistTip => "  Use /mode plan to create structured checklists",
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "Choose your language",
        MessageId::OnboardLanguageBlurb => {
            "Pick the UI language. You can change it any time with `/settings set locale <tag>`."
        }
        MessageId::OnboardLanguageFooter => {
            "Press 1-5 to choose, or Enter to keep the current setting"
        }
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "Connect your DeepSeek API key",
        MessageId::OnboardApiKeyStep1 => {
            "Step 1.  Open https://platform.deepseek.com/api_keys and create a key."
        }
        MessageId::OnboardApiKeyStep2 => "Step 2.  Paste it below and press Enter.",
        MessageId::OnboardApiKeySavedHint => {
            "Saved to ~/.deepseek/config.toml so it works from any folder."
        }
        MessageId::OnboardApiKeyFormatHint => {
            "Paste the full key exactly as issued (no spaces or newlines)."
        }
        MessageId::OnboardApiKeyPlaceholder => "(paste key here)",
        MessageId::OnboardApiKeyLabel => "Key: ",
        MessageId::OnboardApiKeyFooter => "Press Enter to save, Esc to go back.",
        // Onboarding — workspace trust.
        MessageId::OnboardTrustTitle => "Trust Workspace",
        MessageId::OnboardTrustQuestion => "Do you trust the contents of this directory?",
        MessageId::OnboardTrustLocationPrefix => "You are in ",
        MessageId::OnboardTrustRiskHint => {
            "Working with untrusted contents comes with higher risk of prompt injection."
        }
        MessageId::OnboardTrustEffectHint => {
            "Trusting this directory records it in global config and enables trusted workspace mode."
        }
        MessageId::OnboardTrustFooterPrefix => "Press ",
        MessageId::OnboardTrustFooterMiddle => " to trust and continue, ",
        MessageId::OnboardTrustFooterSuffix => " to quit",
        // Onboarding — final tips.
        MessageId::OnboardTipsTitle => "Start Simple",
        MessageId::OnboardTipsLine1 => {
            "Write the task in plain language. Use /help or Ctrl+K when you want a command."
        }
        MessageId::OnboardTipsLine2 => {
            "The bottom composer is multi-line: Enter sends, Alt+Enter or Ctrl+J adds a new line."
        }
        MessageId::OnboardTipsLine3 => {
            "Switch modes only when the job changes: Plan for review-first work, Agent for execution, YOLO when you want auto-approval."
        }
        MessageId::OnboardTipsLine4 => {
            "Ctrl+R resumes earlier sessions, and Esc backs out of the current draft or overlay."
        }
        MessageId::OnboardTipsFooterEnter => "Press Enter",
        MessageId::OnboardTipsFooterAction => " to open the workspace",
    }
}

fn translation(locale: Locale, id: MessageId) -> Option<&'static str> {
    match locale {
        Locale::En => Some(english(id)),
        Locale::Ja => japanese(id),
        Locale::ZhHans => chinese_simplified(id),
        Locale::PtBr => portuguese_brazil(id),
    }
}

fn japanese(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ComposerPlaceholder => "タスクを書くか / を使う。",
        MessageId::HistorySearchPlaceholder => "プロンプト履歴を検索...",
        MessageId::HistorySearchTitle => "履歴検索",
        MessageId::HistoryHintMove => "Up/Down 移動",
        MessageId::HistoryHintAccept => "Enter 確定",
        MessageId::HistoryHintRestore => "Esc 復元",
        MessageId::HistoryNoMatches => "  一致なし",
        MessageId::ConfigTitle => "セッション設定",
        MessageId::ConfigModalTitle => " 設定 ",
        MessageId::ConfigSearchPlaceholder => "入力して絞り込み",
        MessageId::ConfigNoSettings => "  設定がありません。",
        MessageId::ConfigNoMatchesPrefix => "  一致する設定なし: ",
        MessageId::ConfigFilteredSettings => "  絞り込み後の設定",
        MessageId::ConfigShowing => "  表示",
        MessageId::ConfigFooterDefault => {
            " 入力=絞り込み, Up/Down=選択, Enter/e=編集, Esc/q=閉じる "
        }
        MessageId::ConfigFooterScrollable => {
            " 入力=絞り込み, Up/Down=選択, Enter/e=編集, PgUp/PgDn=スクロール, Esc/q=閉じる "
        }
        MessageId::ConfigFooterFiltered => {
            " 入力=絞り込み, Backspace=削除, Ctrl+U/Esc=クリア, Enter=編集 "
        }
        MessageId::HelpTitle => "ヘルプ",
        MessageId::HelpFilterPlaceholder => "入力して絞り込み",
        MessageId::HelpFilterPrefix => "絞り込み: ",
        MessageId::HelpNoMatches => "  一致なし。",
        MessageId::HelpSlashCommands => "スラッシュコマンド",
        MessageId::HelpKeybindings => "キー操作",
        MessageId::HelpFooterTypeFilter => " 入力して絞り込み ",
        MessageId::HelpFooterMove => "  Up/Down 移動 ",
        MessageId::HelpFooterJump => " PgUp/PgDn ジャンプ ",
        MessageId::HelpFooterClose => " Esc 閉じる ",
        MessageId::CmdAnchorDescription => {
            "コンパクション後も保持される重要な事実をピン留め（コンテキストに自動注入）"
        }
        MessageId::CmdAttachDescription => {
            "画像・動画メディアを添付（テキストファイルやディレクトリは @path）"
        }
        MessageId::CmdCacheDescription => {
            "直近 N ターンの DeepSeek プレフィックスキャッシュのヒット/ミス統計を表示"
        }
        MessageId::CmdClearDescription => "会話履歴をクリア",
        MessageId::CmdCompactDescription => {
            "コンテキスト圧縮で容量を確保（旧式：v0.6.6 以降はサイクル再起動を推奨）"
        }
        MessageId::CmdConfigDescription => "インタラクティブな設定エディタを開く",
        MessageId::CmdContextDescription => "コンパクトなセッションコンテキスト検査ツールを開く",
        MessageId::CmdCostDescription => "セッションのコスト内訳を表示",
        MessageId::CmdCycleDescription => "指定したサイクルの引き継ぎブリーフィングを表示",
        MessageId::CmdCyclesDescription => {
            "セッション内のチェックポイント再起動サイクルの引き継ぎを一覧表示"
        }
        MessageId::CmdDiffDescription => "セッション開始以降のファイル変更を表示",
        MessageId::CmdEditDescription => "最後のメッセージを編集して再送信",
        MessageId::CmdExitDescription => "アプリを終了",
        MessageId::CmdExportDescription => "会話を Markdown にエクスポート",
        MessageId::CmdFeedbackDescription => "GitHub フィードバック URL を生成",
        MessageId::CmdHelpDescription => "ヘルプを表示",
        MessageId::CmdHomeDescription => "統計とクイックアクション付きのホームダッシュボードを表示",
        MessageId::CmdHooksDescription => {
            "設定済みのライフサイクルフックを一覧表示（読み取り専用）"
        }
        MessageId::CmdGoalDescription => "トークンバジェット付きのセッション目標を設定",
        MessageId::CmdInitDescription => "プロジェクト用に AGENTS.md を生成",
        MessageId::CmdLspDescription => "LSP 診断のオン・オフを切り替え",
        MessageId::CmdShareDescription => "現在のセッションを共有可能な Web URL としてエクスポート",
        MessageId::CmdJobsDescription => "バックグラウンドのシェルジョブを確認・制御",
        MessageId::CmdLinksDescription => "DeepSeek ダッシュボードとドキュメントへのリンクを表示",
        MessageId::CmdLoadDescription => "ファイルからセッションを読み込み",
        MessageId::CmdLogoutDescription => "API キーを消去してセットアップに戻る",
        MessageId::CmdMcpDescription => "MCP サーバを開く・管理する",
        MessageId::CmdMemoryDescription => "永続ユーザーメモリファイルを確認・管理",
        MessageId::CmdModeDescription => {
            "動作モードを切り替え、または選択画面を開く: /mode [agent|plan|yolo|1|2|3]"
        }
        MessageId::CmdModelDescription => "現在のモデルを切り替え・確認",
        MessageId::CmdModelsDescription => "API から利用可能なモデルを一覧表示",
        MessageId::CmdNetworkDescription => "ネットワーク許可・拒否ルールを管理",
        MessageId::CmdNoteDescription => "ワークスペースノートの追加、一覧、編集、削除",
        MessageId::CmdThemeDescription => "テーマ（ダーク/ライト）を切り替え",
        MessageId::CmdProviderDescription => {
            "現在の LLM バックエンドを切り替え・確認（deepseek | nvidia-nim | ollama）"
        }
        MessageId::CmdQueueDescription => "キューされたメッセージを確認・編集",
        MessageId::CmdRecallDescription => {
            "過去のサイクルアーカイブを検索（メッセージ本文への BM25 検索）"
        }
        MessageId::CmdRenameDescription => "現在のセッションの名前を変更",
        MessageId::CmdRestoreDescription => {
            "ワークスペースを以前のターン前/後スナップショットへロールバック。引数なしで最近のスナップショットを一覧表示。"
        }
        MessageId::CmdRetryDescription => "直前のリクエストを再試行",
        MessageId::CmdReviewDescription => "ファイル・diff・PR に対して構造化コードレビューを実行",
        MessageId::CmdRlmDescription => {
            "再帰言語モデル（RLM）ターン — プロンプトを Python REPL に格納し、モデルが処理コードを記述。サブ LLM 呼び出しは `llm_query()` / `sub_rlm()`。"
        }
        MessageId::CmdSaveDescription => "セッションをファイルに保存",
        MessageId::CmdSessionsDescription => "セッションピッカーを開く",
        MessageId::CmdSettingsDescription => "永続化された設定を表示",
        MessageId::CmdSkillDescription => {
            "スキルを有効化、またはコミュニティスキルをインストール／更新／アンインストール／信頼"
        }
        MessageId::CmdSkillsDescription => {
            "ローカルスキルを一覧表示（`/skills <prefix>` で絞り込み、--remote で精選レジストリを参照）"
        }
        MessageId::CmdStashDescription => {
            "コンポーザーの下書きを退避／復元（Ctrl+S で退避、/stash list|pop）"
        }
        MessageId::CmdStatusDescription => "実行中のセッション状態を表示",
        MessageId::CmdStatuslineDescription => "フッターに表示する項目を設定",
        MessageId::CmdSubagentsDescription => "サブエージェントの状態を一覧表示",
        MessageId::CmdSwarmDescription => {
            "マルチエージェントのファンアウトターンを実行（sequential | mixture | distill | deliberate）"
        }
        MessageId::CmdSystemDescription => "現在のシステムプロンプトを表示",
        MessageId::CmdTaskDescription => "バックグラウンドタスクを管理",
        MessageId::CmdTokensDescription => "セッションのトークン使用量を表示",
        MessageId::CmdTrustDescription => {
            "ワークスペースの信頼設定とパス別許可リストを管理（`/trust add <path>`、`/trust list`、`/trust on|off`）"
        }
        MessageId::CmdUndoDescription => "最後のメッセージ対を削除",
        MessageId::CmdVerboseDescription => "ライブ思考表示の詳細モードを切り替え",
        MessageId::CmdCacheAdvice => {
            "3 ターン目以降にヒット率が ~70% 以上で安定していれば、プレフィックスキャッシュは健全。\n\
             長いセッションでこれを下回る場合はプレフィックスのドリフトの可能性あり (#263)。"
        }
        MessageId::CmdCacheFootnote => {
            "* プロバイダがミスを単独で報告しない場合は「入力 − ヒット」から推定。\n"
        }
        MessageId::CmdCacheHeader => {
            "キャッシュテレメトリ — 直近 {count} / {total} ターン（モデル: {model}）\n"
        }
        MessageId::CmdCacheNoData => {
            "キャッシュ履歴: まだターンを記録していません。\n\n\
             DeepSeek は対応モデル (V4 系) の各 API ターンで `prompt_cache_hit_tokens` / \
             `prompt_cache_miss_tokens` を返します。1 ターン実行してから /cache を再度試してください。"
        }
        MessageId::CmdCacheTotals => {
            "Σ 入力: {sum_in}   Σ ヒット: {sum_hit}   Σ ミス: {sum_miss}   平均ヒット率: {avg}\n"
        }
        MessageId::CmdCostReport => {
            "セッション費用:\n\
             ─────────────────────────────\n\
             累計概算: {cost}\n\n\
             費用は概算値。プロバイダの使用量テレメトリがあれば優先して使用します。\n\n\
             DeepSeek API 料金:\n\
             ─────────────────────────────\n\
             本 CLI には詳細な料金表は組み込まれていません。"
        }
        MessageId::CmdTokensCacheBoth => "ヒット {hit} / ミス {miss}",
        MessageId::CmdTokensCacheHitOnly => "ヒット {hit} / ミスは未報告",
        MessageId::CmdTokensCacheMissOnly => "ヒットは未報告 / ミス {miss}",
        MessageId::CmdTokensContextUnknownWindow => "~{estimated} / コンテキスト窓不明",
        MessageId::CmdTokensContextWithWindow => "~{used} / {window} ({percent}%)",
        MessageId::FooterAgentSingular => "1 エージェント",
        MessageId::FooterAgentsPlural => "{count} エージェント",
        MessageId::FooterPressCtrlCAgain => "もう一度 Ctrl+C で終了",
        MessageId::FooterWorking => "処理中",
        MessageId::HelpSectionActions => "操作",
        MessageId::HelpSectionClipboard => "クリップボード",
        MessageId::HelpSectionEditing => "入力編集",
        MessageId::HelpSectionHelp => "ヘルプ",
        MessageId::HelpSectionModes => "モード",
        MessageId::HelpSectionNavigation => "ナビゲーション",
        MessageId::HelpSectionSessions => "セッション",
        MessageId::CmdTokensNotReported => "未報告",
        MessageId::CmdTokensReport => {
            "トークン使用量:\n\
             ─────────────────────────────\n\
             アクティブコンテキスト: {active}\n\
             直近の API 入力:        {input}（ターン単位のテレメトリ。複数回のツール往復で同じプレフィックスが重複してカウントされる場合あり）\n\
             直近の API 出力:        {output}\n\
             キャッシュヒット/ミス:  {cache}（テレメトリ/コスト用のみ）\n\
             累計トークン:           {total}（セッション使用量テレメトリ）\n\
             セッション費用概算:     {cost}\n\
             API メッセージ:         {api_messages}\n\
             チャットメッセージ:     {chat_messages}\n\
             モデル:                 {model}"
        }
        MessageId::KbScrollTranscript => {
            "会話履歴をスクロール、入力履歴を移動、または添付ファイルを選択"
        }
        MessageId::KbNavigateHistory => "入力履歴を移動",
        MessageId::KbScrollTranscriptAlt => "会話履歴をスクロール",
        MessageId::KbScrollPage => "ページ単位で会話履歴をスクロール",
        MessageId::KbJumpTopBottom => "会話履歴の先頭/末尾へジャンプ",
        MessageId::KbJumpTopBottomEmpty => "先頭/末尾へジャンプ（入力が空の時）",
        MessageId::KbJumpToolBlocks => "ツール出力ブロック間をジャンプ",
        MessageId::KbMoveCursor => "コンポーザー内でカーソルを移動",
        MessageId::KbJumpLineStartEnd => "行の先頭/末尾へジャンプ",
        MessageId::KbDeleteChar => "カーソル前/後の文字を削除、または選択中の添付を削除",
        MessageId::KbClearDraft => "現在の下書きをクリア",
        MessageId::KbStashDraft => "現在の下書きをスタッシュ（`/stash pop`で復元）",
        MessageId::KbSearchHistory => "プロンプト履歴を検索してローカル下書きを復元",
        MessageId::KbInsertNewline => "コンポーザーに改行を挿入",
        MessageId::KbSendDraft => "現在の下書きを送信",
        MessageId::KbCloseMenu => {
            "メニューを閉じる、リクエストをキャンセル、下書きを破棄、または入力をクリア"
        }
        MessageId::KbCancelOrExit => "リクエストをキャンセル、またはアイドル時に終了",
        MessageId::KbShellControls => "実行中のフォアグラウンドコマンドのシェル制御を開く",
        MessageId::KbExitEmpty => "入力が空の時に終了",
        MessageId::KbCommandPalette => "コマンドパレットを開く",
        MessageId::KbFuzzyFilePicker => "ファジーファイルピッカーを開く（Enter で @path を挿入）",
        MessageId::KbCompactInspector => "コンパクトなセッションコンテキスト検査ツールを開く",
        MessageId::KbLastMessagePager => "最後のメッセージのページャーを開く（入力が空の時）",
        MessageId::KbSelectedDetails => {
            "選択中のツールまたはメッセージの詳細を開く（入力が空の時）"
        }
        MessageId::KbToolDetailsPager => "ツール詳細のページャーを開く",
        MessageId::KbThinkingPager => "思考内容のページャーを開く",
        MessageId::KbLiveTranscript => "ライブ会話履歴オーバーレイを開く（自動追尾スクロール）",
        MessageId::KbBacktrackMessage => {
            "前のユーザーメッセージに戻る（左右でステップ、Enter で巻き戻し）"
        }
        MessageId::KbCompleteCycleModes => {
            "/command を補完、実行中ターンのフォローアップをキュー、モードを切り替え；Shift+Tab で推論強度を切り替え"
        }
        MessageId::KbJumpPlanAgentYolo => "Plan / Agent / YOLO モードに直接ジャンプ",
        MessageId::KbAltJumpPlanAgentYolo => "Plan / Agent / YOLO モードへの代替ジャンプ",
        MessageId::KbFocusSidebar => "Plan / Todos / Tasks / Agents / Auto サイドバーにフォーカス",
        MessageId::KbTogglePlanAgent => "Plan モードと Agent モードを切り替え",
        MessageId::KbSessionPicker => "セッションピッカーを開く",
        MessageId::KbPasteAttach => "テキストを貼り付けまたはクリップボード画像を添付",
        MessageId::KbCopySelection => "現在の選択をコピー（macOS は Cmd+C）",
        MessageId::KbContextMenu => {
            "貼り付け、選択、メッセージ詳細、コンテキスト、ヘルプのコンテキスト操作を開く"
        }
        MessageId::KbAttachPath => {
            "ローカルのテキストファイルまたはディレクトリをコンテキストに追加"
        }
        MessageId::KbHelpOverlay => "このヘルプオーバーレイを開く（入力が空の時）",
        MessageId::KbToggleHelp => "ヘルプオーバーレイを切り替え",
        MessageId::KbToggleHelpSlash => "ヘルプオーバーレイを切り替え",
        MessageId::HelpUsageLabel => "使い方：",
        MessageId::HelpAliasesLabel => "エイリアス：",
        MessageId::SettingsTitle => "設定：",
        MessageId::SettingsConfigFile => "設定ファイル：",
        MessageId::ClearConversation => "会話履歴をクリアしました",
        MessageId::ClearConversationBusy => {
            "会話履歴をクリアしました（plan 状態が忙しい；必要なら /clear を再度実行）"
        }
        MessageId::ModelChanged => "モデルを変更しました: {old} → {new}",
        MessageId::LinksTitle => "DeepSeek リンク：",
        MessageId::LinksDashboard => "ダッシュボード：",
        MessageId::LinksDocs => "ドキュメント：",
        MessageId::LinksTip => "ヒント: API キーはダッシュボードコンソールで取得できます。",
        MessageId::SubagentsFetching => "サブエージェントの状態を取得中...",
        MessageId::HelpUnknownCommand => "不明なコマンド: {topic}",
        MessageId::HomeDashboardTitle => "DeepSeek TUI ホームダッシュボード",
        MessageId::HomeModel => "モデル：",
        MessageId::HomeMode => "モード：",
        MessageId::HomeWorkspace => "ワークスペース：",
        MessageId::HomeHistory => "履歴：",
        MessageId::HomeTokens => "トークン：",
        MessageId::HomeQueued => "キュー：",
        MessageId::HomeSubagents => "サブエージェント：",
        MessageId::HomeSkill => "スキル：",
        MessageId::HomeQuickActions => "クイックアクション",
        MessageId::HomeQuickLinks => "/links      - ダッシュボードと API リンク",
        MessageId::HomeQuickSkills => "/skills      - 利用可能なスキルを一覧",
        MessageId::HomeQuickConfig => "/config      - インタラクティブな設定エディタを開く",
        MessageId::HomeQuickSettings => "/settings    - 永続化された設定を表示",
        MessageId::HomeQuickModel => "/model       - モデルを切り替え・確認",
        MessageId::HomeQuickSubagents => "/subagents   - サブエージェントの状態を一覧",
        MessageId::HomeQuickTaskList => "/task list   - バックグラウンドタスクキューを表示",
        MessageId::HomeQuickHelp => "/help        - ヘルプを表示",
        MessageId::HomeModeTips => "モードヒント",
        MessageId::HomeAgentModeTip => "Agent モード - ツールを使って自律的なタスクを実行",
        MessageId::HomeAgentModeReviewTip => "  実行前に Ctrl+X で Plan モードでレビュー",
        MessageId::HomeAgentModeYoloTip => "  /mode yolo と入力して完全なツールアクセスを有効化",
        MessageId::HomeYoloModeTip => "YOLO モード - 完全なツールアクセス、承認なし",
        MessageId::HomeYoloModeCaution => "  破壊的な操作には注意してください！",
        MessageId::HomePlanModeTip => "Plan モード - 実装前に設計",
        MessageId::HomePlanModeChecklistTip => {
            "  /mode plan を使って構造化されたチェックリストを作成"
        }
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "言語を選択",
        MessageId::OnboardLanguageBlurb => {
            "UI 言語を選んでください。`/settings set locale <tag>` でいつでも変更できます。"
        }
        MessageId::OnboardLanguageFooter => "1〜5 で選択、または Enter で現在の設定を維持",
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "DeepSeek API キーを設定",
        MessageId::OnboardApiKeyStep1 => {
            "ステップ 1. https://platform.deepseek.com/api_keys を開いてキーを作成。"
        }
        MessageId::OnboardApiKeyStep2 => "ステップ 2. 下に貼り付けて Enter を押してください。",
        MessageId::OnboardApiKeySavedHint => {
            "~/.deepseek/config.toml に保存されるので、どのフォルダからでも有効になります。"
        }
        MessageId::OnboardApiKeyFormatHint => {
            "発行されたキーをそのまま貼り付けてください（空白や改行を含めない）。"
        }
        MessageId::OnboardApiKeyPlaceholder => "（ここにキーを貼り付け）",
        MessageId::OnboardApiKeyLabel => "キー: ",
        MessageId::OnboardApiKeyFooter => "Enter で保存、Esc で戻る。",
        // Onboarding — workspace trust.
        MessageId::OnboardTrustTitle => "ワークスペースを信頼",
        MessageId::OnboardTrustQuestion => "このディレクトリの内容を信頼しますか？",
        MessageId::OnboardTrustLocationPrefix => "現在の場所: ",
        MessageId::OnboardTrustRiskHint => {
            "信頼されていない内容を扱うとプロンプトインジェクションのリスクが高くなります。"
        }
        MessageId::OnboardTrustEffectHint => {
            "信頼するとグローバル設定に記録され、信頼済みワークスペースモードが有効になります。"
        }
        MessageId::OnboardTrustFooterPrefix => "キー ",
        MessageId::OnboardTrustFooterMiddle => " で信頼して続行、",
        MessageId::OnboardTrustFooterSuffix => " で終了",
        // Onboarding — final tips.
        MessageId::OnboardTipsTitle => "シンプルに始めよう",
        MessageId::OnboardTipsLine1 => {
            "タスクを自然な言葉で記入。コマンドが必要な時は /help や Ctrl+K を使ってください。"
        }
        MessageId::OnboardTipsLine2 => {
            "下の入力欄は複数行対応です。Enter で送信、Alt+Enter または Ctrl+J で改行。"
        }
        MessageId::OnboardTipsLine3 => {
            "用途に応じてモードを切り替え：Plan は事前レビュー、Agent は実行、YOLO は自動承認。"
        }
        MessageId::OnboardTipsLine4 => {
            "Ctrl+R で過去のセッションを再開、Esc で現在の入力やオーバーレイをキャンセル。"
        }
        MessageId::OnboardTipsFooterEnter => "Enter を押す",
        MessageId::OnboardTipsFooterAction => " とワークスペースが開きます",
    })
}

fn chinese_simplified(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ComposerPlaceholder => "编写任务或使用 /。",
        MessageId::HistorySearchPlaceholder => "搜索提示历史...",
        MessageId::HistorySearchTitle => "历史搜索",
        MessageId::HistoryHintMove => "Up/Down 移动",
        MessageId::HistoryHintAccept => "Enter 接受",
        MessageId::HistoryHintRestore => "Esc 还原",
        MessageId::HistoryNoMatches => "  无匹配",
        MessageId::ConfigTitle => "会话配置",
        MessageId::ConfigModalTitle => " 配置 ",
        MessageId::ConfigSearchPlaceholder => "输入以筛选",
        MessageId::ConfigNoSettings => "  没有可用设置。",
        MessageId::ConfigNoMatchesPrefix => "  没有匹配设置: ",
        MessageId::ConfigFilteredSettings => "  已筛选设置",
        MessageId::ConfigShowing => "  显示",
        MessageId::ConfigFooterDefault => " 输入=筛选, Up/Down=选择, Enter/e=编辑, Esc/q=关闭 ",
        MessageId::ConfigFooterScrollable => {
            " 输入=筛选, Up/Down=选择, Enter/e=编辑, PgUp/PgDn=滚动, Esc/q=关闭 "
        }
        MessageId::ConfigFooterFiltered => {
            " 输入=筛选, Backspace=删除, Ctrl+U/Esc=清除, Enter=编辑 "
        }
        MessageId::HelpTitle => "帮助",
        MessageId::HelpFilterPlaceholder => "输入以筛选",
        MessageId::HelpFilterPrefix => "筛选: ",
        MessageId::HelpNoMatches => "  无匹配。",
        MessageId::HelpSlashCommands => "斜杠命令",
        MessageId::HelpKeybindings => "快捷键",
        MessageId::HelpFooterTypeFilter => " 输入以筛选 ",
        MessageId::HelpFooterMove => "  Up/Down 移动 ",
        MessageId::HelpFooterJump => " PgUp/PgDn 跳转 ",
        MessageId::HelpFooterClose => " Esc 关闭 ",
        MessageId::CmdAnchorDescription => "钉选关键事实，在压缩后自动注入上下文",
        MessageId::CmdAttachDescription => "附加图片或视频媒体；文本文件或目录请使用 @path",
        MessageId::CmdCacheDescription => "显示最近 N 轮的 DeepSeek 前缀缓存命中/未命中统计",
        MessageId::CmdClearDescription => "清除对话历史",
        MessageId::CmdCompactDescription => {
            "触发上下文压缩以释放空间（旧版命令；v0.6.6 起建议改用循环重启）"
        }
        MessageId::CmdConfigDescription => "打开交互式配置编辑器",
        MessageId::CmdContextDescription => "打开紧凑会话上下文检查器",
        MessageId::CmdCostDescription => "显示本次会话的费用明细",
        MessageId::CmdCycleDescription => "显示指定循环的延续简报",
        MessageId::CmdCyclesDescription => "列出本次会话中的检查点重启循环交接",
        MessageId::CmdDiffDescription => "显示会话开始以来的文件变更",
        MessageId::CmdEditDescription => "修改并重新提交最后一条消息",
        MessageId::CmdExitDescription => "退出应用",
        MessageId::CmdExportDescription => "将对话导出为 Markdown",
        MessageId::CmdFeedbackDescription => "生成 GitHub 反馈链接",
        MessageId::CmdHelpDescription => "显示帮助信息",
        MessageId::CmdHomeDescription => "显示主页面板，含统计与快捷操作",
        MessageId::CmdHooksDescription => "列出已配置的生命周期钩子（只读）",
        MessageId::CmdGoalDescription => "设置带有可选令牌预算的会话目标",
        MessageId::CmdInitDescription => "为项目生成 AGENTS.md",
        MessageId::CmdLspDescription => "切换 LSP 诊断的开启或关闭",
        MessageId::CmdShareDescription => "将当前会话导出为可共享的 Web URL",
        MessageId::CmdJobsDescription => "查看并管理后台 shell 作业",
        MessageId::CmdLinksDescription => "显示 DeepSeek 控制台与文档链接",
        MessageId::CmdLoadDescription => "从文件加载会话",
        MessageId::CmdLogoutDescription => "清除 API 密钥并返回设置",
        MessageId::CmdMcpDescription => "打开或管理 MCP 服务器",
        MessageId::CmdMemoryDescription => "查看或管理持久用户记忆文件",
        MessageId::CmdModeDescription => "切换运行模式或打开选择器：/mode [agent|plan|yolo|1|2|3]",
        MessageId::CmdModelDescription => "切换或查看当前模型",
        MessageId::CmdModelsDescription => "列出 API 中可用的模型",
        MessageId::CmdNetworkDescription => "管理网络允许和拒绝规则",
        MessageId::CmdNoteDescription => "添加、列出、编辑或删除工作区笔记",
        MessageId::CmdThemeDescription => "在浅色和深色主题之间切换",
        MessageId::CmdProviderDescription => {
            "切换或查看当前 LLM 后端（deepseek | nvidia-nim | ollama）"
        }
        MessageId::CmdQueueDescription => "查看或编辑已排队的消息",
        MessageId::CmdRecallDescription => "搜索此前的循环归档（基于消息文本的 BM25 检索）",
        MessageId::CmdRenameDescription => "重命名当前会话",
        MessageId::CmdRestoreDescription => {
            "将工作区回滚到此前的轮次前/后快照。不带参数时列出最近的快照。"
        }
        MessageId::CmdRetryDescription => "重试上一次请求",
        MessageId::CmdReviewDescription => "对文件、diff 或 PR 进行结构化代码审查",
        MessageId::CmdRlmDescription => {
            "递归语言模型（RLM）轮次 —— 将提示词存入 Python REPL，让模型编写代码进行处理；可用 `llm_query()` / `sub_rlm()` 调用子 LLM。"
        }
        MessageId::CmdSaveDescription => "将会话保存到文件",
        MessageId::CmdSessionsDescription => "打开会话选择器",
        MessageId::CmdSettingsDescription => "显示持久化设置",
        MessageId::CmdSkillDescription => "激活技能，或安装/更新/卸载/信任社区技能",
        MessageId::CmdSkillsDescription => {
            "列出本地技能（用 `/skills <prefix>` 按名称前缀过滤，--remote 浏览精选注册表）"
        }
        MessageId::CmdStashDescription => "暂存或恢复输入草稿（Ctrl+S 暂存，/stash list|pop）",
        MessageId::CmdStatusDescription => "显示当前运行状态",
        MessageId::CmdStatuslineDescription => "配置底栏要显示哪些条目",
        MessageId::CmdSubagentsDescription => "列出子代理状态",
        MessageId::CmdSwarmDescription => {
            "运行多代理扇出轮次（sequential | mixture | distill | deliberate）"
        }
        MessageId::CmdSystemDescription => "显示当前系统提示词",
        MessageId::CmdTaskDescription => "管理后台任务",
        MessageId::CmdTokensDescription => "显示本次会话的 token 用量",
        MessageId::CmdTrustDescription => {
            "管理工作区信任与按路径的白名单（`/trust add <path>`、`/trust list`、`/trust on|off`）"
        }
        MessageId::CmdUndoDescription => "移除最后一组消息对",
        MessageId::CmdVerboseDescription => "切换实时思考内容的完整显示",
        MessageId::CmdCacheAdvice => {
            "第 3 轮起命中率稳定在 ~70% 以上即表示前缀缓存稳定；\n\
             长会话中明显偏低则意味着前缀有抖动，值得排查（#263）。"
        }
        MessageId::CmdCacheFootnote => "* 当提供方未单独上报未命中时，由「输入 − 命中」推算。\n",
        MessageId::CmdCacheHeader => "缓存遥测 —— 最近 {count} / {total} 轮（模型：{model}）\n",
        MessageId::CmdCacheNoData => {
            "缓存历史：尚未记录任何轮次。\n\n\
             DeepSeek 在受支持的模型（V4 系列）每个 API 轮次都会返回 `prompt_cache_hit_tokens` / \
             `prompt_cache_miss_tokens`。请先运行一个轮次再试 /cache。"
        }
        MessageId::CmdCacheTotals => {
            "Σ 输入：{sum_in}   Σ 命中：{sum_hit}   Σ 未命中：{sum_miss}   平均命中率：{avg}\n"
        }
        MessageId::CmdCostReport => {
            "会话费用：\n\
             ─────────────────────────────\n\
             预估累计消耗：{cost}\n\n\
             费用为估算值；如有提供方用量遥测会优先使用。\n\n\
             DeepSeek API 计费：\n\
             ─────────────────────────────\n\
             此 CLI 中未配置详细计费规则。"
        }
        MessageId::CmdTokensCacheBoth => "命中 {hit} / 未命中 {miss}",
        MessageId::CmdTokensCacheHitOnly => "命中 {hit} / 未命中未上报",
        MessageId::CmdTokensCacheMissOnly => "命中未上报 / 未命中 {miss}",
        MessageId::CmdTokensContextUnknownWindow => "~{estimated} / 窗口未知",
        MessageId::CmdTokensContextWithWindow => "~{used} / {window}（{percent}%）",
        MessageId::FooterAgentSingular => "1 个子代理",
        MessageId::FooterAgentsPlural => "{count} 个子代理",
        MessageId::FooterPressCtrlCAgain => "再次按 Ctrl+C 退出",
        MessageId::FooterWorking => "工作中",
        MessageId::HelpSectionActions => "操作",
        MessageId::HelpSectionClipboard => "剪贴板",
        MessageId::HelpSectionEditing => "输入编辑",
        MessageId::HelpSectionHelp => "帮助",
        MessageId::HelpSectionModes => "模式",
        MessageId::HelpSectionNavigation => "导航",
        MessageId::HelpSectionSessions => "会话",
        MessageId::CmdTokensNotReported => "未上报",
        MessageId::CmdTokensReport => {
            "令牌用量：\n\
             ─────────────────────────────\n\
             活动上下文：       {active}\n\
             上次 API 输入：    {input}（来自轮次遥测；多轮工具调用中相同前缀可能被重复计入）\n\
             上次 API 输出：    {output}\n\
             缓存命中/未命中：  {cache}（仅用于遥测/计费）\n\
             累计令牌：         {total}（会话用量遥测）\n\
             预估会话费用：     {cost}\n\
             API 消息数：       {api_messages}\n\
             聊天消息数：       {chat_messages}\n\
             模型：             {model}"
        }
        MessageId::KbScrollTranscript => "滚动对话记录、浏览输入历史或选择附件",
        MessageId::KbNavigateHistory => "浏览输入历史",
        MessageId::KbScrollTranscriptAlt => "滚动对话记录",
        MessageId::KbScrollPage => "按页滚动对话记录",
        MessageId::KbJumpTopBottom => "跳转到对话顶部/底部",
        MessageId::KbJumpTopBottomEmpty => "跳转到顶部/底部（输入框为空时）",
        MessageId::KbJumpToolBlocks => "在工具输出块之间跳转",
        MessageId::KbMoveCursor => "在输入框中移动光标",
        MessageId::KbJumpLineStartEnd => "跳转到行首/行尾",
        MessageId::KbDeleteChar => "删除光标前/后的字符，或移除已选附件",
        MessageId::KbClearDraft => "清空当前草稿",
        MessageId::KbStashDraft => "暂存当前草稿（用 `/stash pop` 恢复）",
        MessageId::KbSearchHistory => "搜索提示历史并恢复本地草稿",
        MessageId::KbInsertNewline => "在输入框中插入换行",
        MessageId::KbSendDraft => "发送当前草稿",
        MessageId::KbCloseMenu => "关闭菜单、取消请求、丢弃草稿或清空输入",
        MessageId::KbCancelOrExit => "取消请求，或空闲时退出",
        MessageId::KbShellControls => "打开正在运行的前台命令的 shell 控制",
        MessageId::KbExitEmpty => "输入框为空时退出",
        MessageId::KbCommandPalette => "打开命令面板",
        MessageId::KbFuzzyFilePicker => "打开模糊文件选择器（按 Enter 插入 @path）",
        MessageId::KbCompactInspector => "打开紧凑会话上下文检查器",
        MessageId::KbLastMessagePager => "打开最后一条消息的分页器（输入框为空时）",
        MessageId::KbSelectedDetails => "打开选中工具或消息的详情（输入框为空时）",
        MessageId::KbToolDetailsPager => "打开工具详情分页器",
        MessageId::KbThinkingPager => "打开思考内容分页器",
        MessageId::KbLiveTranscript => "打开实时对话覆盖层（自动滚动尾随）",
        MessageId::KbBacktrackMessage => "回退到之前的用户消息（左右键步进，Enter 回退）",
        MessageId::KbCompleteCycleModes => {
            "补全 /command、排队运行轮次跟进、切换模式；Shift+Tab 切换推理强度"
        }
        MessageId::KbJumpPlanAgentYolo => "直接跳转到 Plan / Agent / YOLO 模式",
        MessageId::KbAltJumpPlanAgentYolo => "替代快捷键跳转到 Plan / Agent / YOLO 模式",
        MessageId::KbFocusSidebar => "聚焦 Plan / 待办 / 任务 / 代理 / 代理 / 自动侧边栏",
        MessageId::KbTogglePlanAgent => "在 Plan 和 Agent 模式之间切换",
        MessageId::KbSessionPicker => "打开会话选择器",
        MessageId::KbPasteAttach => "粘贴文本或附加剪贴板图片",
        MessageId::KbCopySelection => "复制当前选中内容（macOS 为 Cmd+C）",
        MessageId::KbContextMenu => "打开上下文操作菜单，用于粘贴、选择、消息详情、上下文和帮助",
        MessageId::KbAttachPath => "添加本地文本文件或目录到上下文",
        MessageId::KbHelpOverlay => "打开此帮助覆盖层（输入框为空时）",
        MessageId::KbToggleHelp => "切换帮助覆盖层",
        MessageId::KbToggleHelpSlash => "切换帮助覆盖层",
        MessageId::HelpUsageLabel => "用法：",
        MessageId::HelpAliasesLabel => "别名：",
        MessageId::SettingsTitle => "设置：",
        MessageId::SettingsConfigFile => "配置文件：",
        MessageId::ClearConversation => "对话已清空",
        MessageId::ClearConversationBusy => {
            "对话已清空（Plan 状态忙碌；如需再次清空请运行 /clear）"
        }
        MessageId::ModelChanged => "模型已切换：{old} \u{2192} {new}",
        MessageId::LinksTitle => "DeepSeek 链接：",
        MessageId::LinksDashboard => "控制台：",
        MessageId::LinksDocs => "文档：",
        MessageId::LinksTip => "提示：API 密钥可在控制台中获取。",
        MessageId::SubagentsFetching => "正在获取子代理状态...",
        MessageId::HelpUnknownCommand => "未知命令：{topic}",
        MessageId::HomeDashboardTitle => "DeepSeek TUI 主面板",
        MessageId::HomeModel => "模型：",
        MessageId::HomeMode => "模式：",
        MessageId::HomeWorkspace => "工作区：",
        MessageId::HomeHistory => "历史：",
        MessageId::HomeTokens => "令牌：",
        MessageId::HomeQueued => "队列：",
        MessageId::HomeSubagents => "子代理：",
        MessageId::HomeSkill => "技能：",
        MessageId::HomeQuickActions => "快捷操作",
        MessageId::HomeQuickLinks => "/links      - 控制台与 API 链接",
        MessageId::HomeQuickSkills => "/skills      - 列出可用技能",
        MessageId::HomeQuickConfig => "/config      - 打开交互式配置编辑器",
        MessageId::HomeQuickSettings => "/settings    - 显示持久化设置",
        MessageId::HomeQuickModel => "/model       - 切换或查看模型",
        MessageId::HomeQuickSubagents => "/subagents   - 列出子代理状态",
        MessageId::HomeQuickTaskList => "/task list   - 显示后台任务队列",
        MessageId::HomeQuickHelp => "/help        - 显示帮助",
        MessageId::HomeModeTips => "模式提示",
        MessageId::HomeAgentModeTip => "Agent 模式 - 使用工具执行自主任务",
        MessageId::HomeAgentModeReviewTip => "  按 Ctrl+X 可在 Plan 模式下审查后再执行",
        MessageId::HomeAgentModeYoloTip => "  输入 /mode yolo 启用完整工具访问",
        MessageId::HomeYoloModeTip => "YOLO 模式 - 完整工具访问，无需审批",
        MessageId::HomeYoloModeCaution => "  请小心破坏性操作！",
        MessageId::HomePlanModeTip => "Plan 模式 - 先设计再实现",
        MessageId::HomePlanModeChecklistTip => "  使用 /mode plan 创建结构化检查清单",
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "选择语言",
        MessageId::OnboardLanguageBlurb => {
            "选择界面语言。可随时使用 `/settings set locale <tag>` 修改。"
        }
        MessageId::OnboardLanguageFooter => "按 1-5 选择，或按 Enter 保留当前设置",
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "连接你的 DeepSeek API 密钥",
        MessageId::OnboardApiKeyStep1 => {
            "步骤 1.  打开 https://platform.deepseek.com/api_keys 创建一个密钥。"
        }
        MessageId::OnboardApiKeyStep2 => "步骤 2.  把密钥粘贴到下方并按 Enter。",
        MessageId::OnboardApiKeySavedHint => {
            "保存到 ~/.deepseek/config.toml，因此在任何目录下都生效。"
        }
        MessageId::OnboardApiKeyFormatHint => "请完整粘贴密钥（不要含空格或换行）。",
        MessageId::OnboardApiKeyPlaceholder => "（在此粘贴密钥）",
        MessageId::OnboardApiKeyLabel => "密钥: ",
        MessageId::OnboardApiKeyFooter => "Enter 保存，Esc 返回。",
        // Onboarding — workspace trust.
        MessageId::OnboardTrustTitle => "信任工作目录",
        MessageId::OnboardTrustQuestion => "你信任此目录中的内容吗？",
        MessageId::OnboardTrustLocationPrefix => "当前位置：",
        MessageId::OnboardTrustRiskHint => "处理不受信任的内容会增加提示词注入的风险。",
        MessageId::OnboardTrustEffectHint => {
            "信任此目录会记录在全局配置中，并启用受信任工作区模式。"
        }
        MessageId::OnboardTrustFooterPrefix => "按 ",
        MessageId::OnboardTrustFooterMiddle => " 信任并继续，",
        MessageId::OnboardTrustFooterSuffix => " 退出",
        // Onboarding — final tips.
        MessageId::OnboardTipsTitle => "从简开始",
        MessageId::OnboardTipsLine1 => "用自然语言描述任务。需要命令时使用 /help 或 Ctrl+K。",
        MessageId::OnboardTipsLine2 => "底部输入框支持多行：Enter 发送，Alt+Enter 或 Ctrl+J 换行。",
        MessageId::OnboardTipsLine3 => {
            "按需切换模式：Plan 适合先审后行，Agent 用于执行，YOLO 启用自动批准。"
        }
        MessageId::OnboardTipsLine4 => "Ctrl+R 恢复历史会话，Esc 退出当前输入或弹层。",
        MessageId::OnboardTipsFooterEnter => "按 Enter",
        MessageId::OnboardTipsFooterAction => " 进入工作区",
    })
}

fn portuguese_brazil(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ComposerPlaceholder => "Escreva uma tarefa ou use /.",
        MessageId::HistorySearchPlaceholder => "Pesquisar histórico de prompts...",
        MessageId::HistorySearchTitle => "Busca no histórico",
        MessageId::HistoryHintMove => "Up/Down move",
        MessageId::HistoryHintAccept => "Enter aceita",
        MessageId::HistoryHintRestore => "Esc restaura",
        MessageId::HistoryNoMatches => "  Sem resultados",
        MessageId::ConfigTitle => "Configuração da sessão",
        MessageId::ConfigModalTitle => " Config ",
        MessageId::ConfigSearchPlaceholder => "digite para filtrar",
        MessageId::ConfigNoSettings => "  Nenhuma configuração disponível.",
        MessageId::ConfigNoMatchesPrefix => "  Nenhuma configuração corresponde a ",
        MessageId::ConfigFilteredSettings => "  Configurações filtradas",
        MessageId::ConfigShowing => "  Mostrando",
        MessageId::ConfigFooterDefault => {
            " digite=filtrar, Up/Down=selecionar, Enter/e=editar, Esc/q=fechar "
        }
        MessageId::ConfigFooterScrollable => {
            " digite=filtrar, Up/Down=selecionar, Enter/e=editar, PgUp/PgDn=rolar, Esc/q=fechar "
        }
        MessageId::ConfigFooterFiltered => {
            " digite=filtrar, Backspace=apagar, Ctrl+U/Esc=limpar, Enter=editar "
        }
        MessageId::HelpTitle => "Ajuda",
        MessageId::HelpFilterPlaceholder => "Digite para filtrar",
        MessageId::HelpFilterPrefix => "Filtro: ",
        MessageId::HelpNoMatches => "  Sem resultados.",
        MessageId::HelpSlashCommands => "Comandos com barra",
        MessageId::HelpKeybindings => "Atalhos",
        MessageId::HelpFooterTypeFilter => " digite para filtrar ",
        MessageId::HelpFooterMove => "  Up/Down move ",
        MessageId::HelpFooterJump => " PgUp/PgDn salta ",
        MessageId::HelpFooterClose => " Esc fecha ",
        MessageId::CmdAnchorDescription => {
            "Fixar um fato que sobrevive à compactação (injetado automaticamente no contexto)"
        }
        MessageId::CmdAttachDescription => {
            "Anexar imagem ou vídeo; use @path para arquivos de texto ou diretórios"
        }
        MessageId::CmdCacheDescription => {
            "Exibir estatísticas de hit/miss do cache de prefixo DeepSeek nas últimas N rodadas"
        }
        MessageId::CmdClearDescription => "Limpar o histórico da conversa",
        MessageId::CmdCompactDescription => {
            "Compactar o contexto para liberar espaço (legado; a v0.6.6 prefere o reinício de ciclo)"
        }
        MessageId::CmdConfigDescription => "Abrir o editor interativo de configuração",
        MessageId::CmdContextDescription => "Abrir o inspetor compacto de contexto da sessão",
        MessageId::CmdCostDescription => "Exibir o detalhamento de custo da sessão",
        MessageId::CmdCycleDescription => {
            "Exibir o briefing de continuidade de um ciclo específico"
        }
        MessageId::CmdCyclesDescription => {
            "Listar as transferências dos ciclos checkpoint-restart desta sessão"
        }
        MessageId::CmdDiffDescription => "Mostrar alterações em arquivos desde o início da sessão",
        MessageId::CmdEditDescription => "Revisar e reenviar a última mensagem",
        MessageId::CmdExitDescription => "Sair do aplicativo",
        MessageId::CmdExportDescription => "Exportar a conversa para markdown",
        MessageId::CmdFeedbackDescription => "Gerar uma URL de feedback no GitHub",
        MessageId::CmdHelpDescription => "Exibir informações de ajuda",
        MessageId::CmdHomeDescription => "Exibir o painel inicial com estatísticas e ações rápidas",
        MessageId::CmdHooksDescription => {
            "Listar hooks de ciclo de vida configurados (somente leitura)"
        }
        MessageId::CmdGoalDescription => {
            "Definir uma meta de sessão com orçamento de tokens opcional"
        }
        MessageId::CmdInitDescription => "Gerar AGENTS.md para o projeto",
        MessageId::CmdLspDescription => "Alternar diagnóstico LSP ligado ou desligado",
        MessageId::CmdShareDescription => "Exportar a sessão atual como uma URL web compartilhável",
        MessageId::CmdJobsDescription => "Inspecionar e controlar jobs de shell em segundo plano",
        MessageId::CmdLinksDescription => "Exibir links do painel e da documentação do DeepSeek",
        MessageId::CmdLoadDescription => "Carregar a sessão de um arquivo",
        MessageId::CmdLogoutDescription => "Limpar a chave de API e voltar à configuração",
        MessageId::CmdMcpDescription => "Abrir ou gerenciar servidores MCP",
        MessageId::CmdMemoryDescription => {
            "Inspecionar ou gerenciar o arquivo persistente de memória do usuário"
        }
        MessageId::CmdModeDescription => {
            "Alternar modo ou abrir seletor: /mode [agent|plan|yolo|1|2|3]"
        }
        MessageId::CmdModelDescription => "Trocar ou exibir o modelo atual",
        MessageId::CmdModelsDescription => "Listar os modelos disponíveis pela API",
        MessageId::CmdNetworkDescription => "Gerenciar regras de rede permitidas e bloqueadas",
        MessageId::CmdNoteDescription => "Adicionar, listar, editar ou remover notas do workspace",
        MessageId::CmdThemeDescription => "Alternar entre o tema claro e escuro",
        MessageId::CmdProviderDescription => {
            "Trocar ou exibir o backend LLM ativo (deepseek | nvidia-nim | ollama)"
        }
        MessageId::CmdQueueDescription => "Ver ou editar mensagens enfileiradas",
        MessageId::CmdRecallDescription => {
            "Buscar arquivos de ciclos anteriores (BM25 sobre o texto das mensagens)"
        }
        MessageId::CmdRenameDescription => "Renomear a sessão atual",
        MessageId::CmdRestoreDescription => {
            "Reverter o workspace a um snapshot pré/pós-turno anterior. Sem argumento, lista os snapshots recentes."
        }
        MessageId::CmdRetryDescription => "Repetir a última requisição",
        MessageId::CmdReviewDescription => {
            "Executar uma revisão de código estruturada em um arquivo, diff ou PR"
        }
        MessageId::CmdRlmDescription => {
            "Turno do Recursive Language Model (RLM) — guarda o prompt em um REPL Python e deixa o modelo escrever o código que o processa; use `llm_query()` / `sub_rlm()` para chamadas a sub-LLMs."
        }
        MessageId::CmdSaveDescription => "Salvar a sessão em arquivo",
        MessageId::CmdSessionsDescription => "Abrir o seletor de sessões",
        MessageId::CmdSettingsDescription => "Exibir as configurações persistidas",
        MessageId::CmdSkillDescription => {
            "Ativar uma skill, ou instalar/atualizar/desinstalar/confiar em uma skill da comunidade"
        }
        MessageId::CmdSkillsDescription => {
            "Listar skills locais (filtre com `/skills <prefixo>`; --remote navega pelo registro curado)"
        }
        MessageId::CmdStashDescription => {
            "Estacionar ou restaurar rascunho do compositor (Ctrl+S estaciona, /stash list|pop)"
        }
        MessageId::CmdStatusDescription => "Exibir o status da sessão em execução",
        MessageId::CmdStatuslineDescription => "Configurar quais itens aparecem no rodapé",
        MessageId::CmdSubagentsDescription => "Listar o status dos sub-agentes",
        MessageId::CmdSwarmDescription => {
            "Executar turno fanout multi-agente (sequential | mixture | distill | deliberate)"
        }
        MessageId::CmdSystemDescription => "Exibir o prompt de sistema atual",
        MessageId::CmdTaskDescription => "Gerenciar tarefas em segundo plano",
        MessageId::CmdTokensDescription => "Exibir o uso de tokens da sessão",
        MessageId::CmdTrustDescription => {
            "Gerenciar a confiança do workspace e a allowlist por caminho (`/trust add <path>`, `/trust list`, `/trust on|off`)"
        }
        MessageId::CmdUndoDescription => "Remover o último par de mensagens",
        MessageId::CmdVerboseDescription => "Alternar pensamento ao vivo completo no transcript",
        MessageId::CmdCacheAdvice => {
            "Taxas de hit/miss acima de ~70% a partir do terceiro turno indicam um prefixo de cache estável;\n\
             valores menores em sessões longas sugerem instabilidade no prefixo, vale investigar (#263)."
        }
        MessageId::CmdCacheFootnote => {
            "* miss inferido a partir de entrada − hit quando o provedor não o reporta separadamente.\n"
        }
        MessageId::CmdCacheHeader => {
            "Telemetria do cache — últimos {count} de {total} turno(s) (modelo: {model})\n"
        }
        MessageId::CmdCacheNoData => {
            "Histórico do cache: nenhum turno registrado ainda.\n\n\
             O DeepSeek expõe `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` em cada turno \
             da API onde o modelo suporta (família V4). Execute um turno e tente /cache de novo."
        }
        MessageId::CmdCacheTotals => {
            "Σ entrada: {sum_in}   Σ hit: {sum_hit}   Σ miss: {sum_miss}   taxa média de hit: {avg}\n"
        }
        MessageId::CmdCostReport => {
            "Custo da sessão:\n\
             ─────────────────────────────\n\
             Total aproximado: {cost}\n\n\
             Estimativas de custo são aproximadas e usam a telemetria de uso do provedor quando disponível.\n\n\
             Preços da API DeepSeek:\n\
             ─────────────────────────────\n\
             Os detalhes de preço não estão configurados nesta CLI."
        }
        MessageId::CmdTokensCacheBoth => "{hit} hit / {miss} miss",
        MessageId::CmdTokensCacheHitOnly => "{hit} hit / miss não reportado",
        MessageId::CmdTokensCacheMissOnly => "hit não reportado / {miss} miss",
        MessageId::CmdTokensContextUnknownWindow => "~{estimated} / janela desconhecida",
        MessageId::CmdTokensContextWithWindow => "~{used} / {window} ({percent}%)",
        MessageId::FooterAgentSingular => "1 sub-agente",
        MessageId::FooterAgentsPlural => "{count} sub-agentes",
        MessageId::FooterPressCtrlCAgain => "Pressione Ctrl+C novamente para sair",
        MessageId::FooterWorking => "trabalhando",
        MessageId::HelpSectionActions => "Ações",
        MessageId::HelpSectionClipboard => "Área de transferência",
        MessageId::HelpSectionEditing => "Edição de entrada",
        MessageId::HelpSectionHelp => "Ajuda",
        MessageId::HelpSectionModes => "Modos",
        MessageId::HelpSectionNavigation => "Navegação",
        MessageId::HelpSectionSessions => "Sessões",
        MessageId::CmdTokensNotReported => "não reportado",
        MessageId::CmdTokensReport => {
            "Uso de tokens:\n\
             ─────────────────────────────\n\
             Contexto ativo:           {active}\n\
             Última entrada da API:    {input} (telemetria por turno; pode contar o mesmo prefixo várias vezes em rodadas com ferramentas)\n\
             Última saída da API:      {output}\n\
             Hit/miss do cache:        {cache} (apenas para telemetria/custo)\n\
             Tokens acumulados:        {total} (telemetria de uso da sessão)\n\
             Custo aproximado:         {cost}\n\
             Mensagens da API:         {api_messages}\n\
             Mensagens do chat:        {chat_messages}\n\
             Modelo:                   {model}"
        }
        MessageId::KbScrollTranscript => {
            "Rolar transcrição, navegar histórico de entrada ou selecionar anexos do compositor"
        }
        MessageId::KbNavigateHistory => "Navegar histórico de entrada",
        MessageId::KbScrollTranscriptAlt => "Rolar transcrição",
        MessageId::KbScrollPage => "Rolar transcrição por página",
        MessageId::KbJumpTopBottom => "Pular para topo / fim da transcrição",
        MessageId::KbJumpTopBottomEmpty => "Pular para topo / fim (quando entrada vazia)",
        MessageId::KbJumpToolBlocks => "Pular entre blocos de saída de ferramentas",
        MessageId::KbMoveCursor => "Mover cursor no compositor",
        MessageId::KbJumpLineStartEnd => "Pular para início / fim da linha",
        MessageId::KbDeleteChar => {
            "Excluir caractere antes / depois do cursor, ou remover anexo selecionado"
        }
        MessageId::KbClearDraft => "Limpar rascunho atual",
        MessageId::KbStashDraft => "Estacionar rascunho atual (`/stash pop` restaura)",
        MessageId::KbSearchHistory => "Buscar histórico de prompts e recuperar rascunhos locais",
        MessageId::KbInsertNewline => "Inserir nova linha no compositor",
        MessageId::KbSendDraft => "Enviar rascunho atual",
        MessageId::KbCloseMenu => {
            "Fechar menu, cancelar requisição, descartar rascunho ou limpar entrada"
        }
        MessageId::KbCancelOrExit => "Cancelar requisição ou sair quando ocioso",
        MessageId::KbShellControls => "Abrir controles de shell para comando em primeiro plano",
        MessageId::KbExitEmpty => "Sair quando entrada vazia",
        MessageId::KbCommandPalette => "Abrir paleta de comandos",
        MessageId::KbFuzzyFilePicker => {
            "Abrir seletor de arquivo fuzzy (insere @path ao pressionar Enter)"
        }
        MessageId::KbCompactInspector => "Abrir inspetor compacto de contexto da sessão",
        MessageId::KbLastMessagePager => {
            "Abrir paginador para última mensagem (quando entrada vazia)"
        }
        MessageId::KbSelectedDetails => {
            "Abrir detalhes da ferramenta ou mensagem selecionada (quando entrada vazia)"
        }
        MessageId::KbToolDetailsPager => "Abrir paginador de detalhes da ferramenta",
        MessageId::KbThinkingPager => "Abrir paginador de raciocínio",
        MessageId::KbLiveTranscript => "Abrir sobreposição de transcrição ao vivo (auto-scroll)",
        MessageId::KbBacktrackMessage => {
            "Retroceder para mensagem anterior do usuário (esquerda/direita, Enter para rebobinar)"
        }
        MessageId::KbCompleteCycleModes => {
            "Completar /command, enfileirar follow-up, ciclar modos; Shift+Tab cicla esforço de raciocínio"
        }
        MessageId::KbJumpPlanAgentYolo => "Pular direto para modo Plan / Agent / YOLO",
        MessageId::KbAltJumpPlanAgentYolo => "Salto alternativo para modo Plan / Agent / YOLO",
        MessageId::KbFocusSidebar => "Focar barra lateral Plan / Todos / Tasks / Agents / Auto",
        MessageId::KbTogglePlanAgent => "Alternar entre modos Plan e Agent",
        MessageId::KbSessionPicker => "Abrir seletor de sessões",
        MessageId::KbPasteAttach => "Colar texto ou anexar imagem da área de transferência",
        MessageId::KbCopySelection => "Copiar seleção atual (Cmd+C no macOS)",
        MessageId::KbContextMenu => {
            "Abrir ações de contexto para colar, seleção, detalhes, contexto e ajuda"
        }
        MessageId::KbAttachPath => "Adicionar arquivo ou diretório local ao contexto",
        MessageId::KbHelpOverlay => "Abrir esta sobreposição de ajuda (quando entrada vazia)",
        MessageId::KbToggleHelp => "Alternar sobreposição de ajuda",
        MessageId::KbToggleHelpSlash => "Alternar sobreposição de ajuda",
        MessageId::HelpUsageLabel => "Uso:",
        MessageId::HelpAliasesLabel => "Apelidos:",
        MessageId::SettingsTitle => "Configurações:",
        MessageId::SettingsConfigFile => "Arquivo de configuração:",
        MessageId::ClearConversation => "Conversa limpa",
        MessageId::ClearConversationBusy => {
            "Conversa limpa (estado do plano ocupado; execute /clear novamente se necessário)"
        }
        MessageId::ModelChanged => "Modelo alterado: {old} \u{2192} {new}",
        MessageId::LinksTitle => "Links do DeepSeek:",
        MessageId::LinksDashboard => "Painel:",
        MessageId::LinksDocs => "Documentação:",
        MessageId::LinksTip => "Dica: chaves de API estão disponíveis no console do painel.",
        MessageId::SubagentsFetching => "Buscando status dos sub-agentes...",
        MessageId::HelpUnknownCommand => "Comando desconhecido: {topic}",
        MessageId::HomeDashboardTitle => "Painel Inicial do DeepSeek TUI",
        MessageId::HomeModel => "Modelo:",
        MessageId::HomeMode => "Modo:",
        MessageId::HomeWorkspace => "Workspace:",
        MessageId::HomeHistory => "Histórico:",
        MessageId::HomeTokens => "Tokens:",
        MessageId::HomeQueued => "Enfileirado:",
        MessageId::HomeSubagents => "Sub-agentes:",
        MessageId::HomeSkill => "Skill:",
        MessageId::HomeQuickActions => "Ações Rápidas",
        MessageId::HomeQuickLinks => "/links      - Links do painel e API",
        MessageId::HomeQuickSkills => "/skills      - Listar skills disponíveis",
        MessageId::HomeQuickConfig => "/config      - Abrir editor interativo de configuração",
        MessageId::HomeQuickSettings => "/settings    - Exibir configurações persistentes",
        MessageId::HomeQuickModel => "/model       - Alternar ou visualizar modelo",
        MessageId::HomeQuickSubagents => "/subagents   - Listar status dos sub-agentes",
        MessageId::HomeQuickTaskList => "/task list   - Exibir fila de tarefas em segundo plano",
        MessageId::HomeQuickHelp => "/help        - Exibir ajuda",
        MessageId::HomeModeTips => "Dicas de Modo",
        MessageId::HomeAgentModeTip => "Modo Agent - Use ferramentas para tarefas autônomas",
        MessageId::HomeAgentModeReviewTip => {
            "  Use Ctrl+X para revisar no modo Plan antes de executar"
        }
        MessageId::HomeAgentModeYoloTip => {
            "  Digite /mode yolo para habilitar acesso total às ferramentas"
        }
        MessageId::HomeYoloModeTip => "Modo YOLO - Acesso total a ferramentas, sem aprovações",
        MessageId::HomeYoloModeCaution => "  Tenha cuidado com operações destrutivas!",
        MessageId::HomePlanModeTip => "Modo Plan - Planeje antes de implementar",
        MessageId::HomePlanModeChecklistTip => {
            "  Use /mode plan para criar checklists estruturados"
        }
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "Escolha o idioma",
        MessageId::OnboardLanguageBlurb => {
            "Escolha o idioma da interface. Você pode mudá-lo a qualquer momento com `/settings set locale <tag>`."
        }
        MessageId::OnboardLanguageFooter => {
            "Pressione 1-5 para escolher, ou Enter para manter a configuração atual"
        }
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "Conecte sua chave de API DeepSeek",
        MessageId::OnboardApiKeyStep1 => {
            "Passo 1.  Abra https://platform.deepseek.com/api_keys e crie uma chave."
        }
        MessageId::OnboardApiKeyStep2 => "Passo 2.  Cole abaixo e pressione Enter.",
        MessageId::OnboardApiKeySavedHint => {
            "Salvo em ~/.deepseek/config.toml para funcionar em qualquer pasta."
        }
        MessageId::OnboardApiKeyFormatHint => {
            "Cole a chave inteira como foi emitida (sem espaços ou quebras de linha)."
        }
        MessageId::OnboardApiKeyPlaceholder => "(cole a chave aqui)",
        MessageId::OnboardApiKeyLabel => "Chave: ",
        MessageId::OnboardApiKeyFooter => "Enter para salvar, Esc para voltar.",
        // Onboarding — workspace trust.
        MessageId::OnboardTrustTitle => "Confiar no diretório",
        MessageId::OnboardTrustQuestion => "Você confia no conteúdo deste diretório?",
        MessageId::OnboardTrustLocationPrefix => "Você está em ",
        MessageId::OnboardTrustRiskHint => {
            "Trabalhar com conteúdo não confiável aumenta o risco de injeção de prompt."
        }
        MessageId::OnboardTrustEffectHint => {
            "Confiar neste diretório o registra na configuração global e habilita o modo workspace confiável."
        }
        MessageId::OnboardTrustFooterPrefix => "Pressione ",
        MessageId::OnboardTrustFooterMiddle => " para confiar e continuar, ",
        MessageId::OnboardTrustFooterSuffix => " para sair",
        // Onboarding — final tips.
        MessageId::OnboardTipsTitle => "Comece simples",
        MessageId::OnboardTipsLine1 => {
            "Escreva a tarefa em linguagem natural. Use /help ou Ctrl+K para comandos."
        }
        MessageId::OnboardTipsLine2 => {
            "O composer inferior é multilinhas: Enter envia, Alt+Enter ou Ctrl+J adiciona uma nova linha."
        }
        MessageId::OnboardTipsLine3 => {
            "Mude de modo apenas quando o trabalho mudar: Plan para revisar antes, Agent para execução, YOLO para auto-aprovação."
        }
        MessageId::OnboardTipsLine4 => {
            "Ctrl+R retoma sessões anteriores, e Esc cancela o rascunho ou overlay atual."
        }
        MessageId::OnboardTipsFooterEnter => "Pressione Enter",
        MessageId::OnboardTipsFooterAction => " para abrir o workspace",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        widgets::{Paragraph, Widget, Wrap},
    };

    #[test]
    fn locale_setting_normalizes_supported_tags() {
        assert_eq!(normalize_configured_locale("auto"), Some("auto"));
        assert_eq!(normalize_configured_locale("ja_JP.UTF-8"), Some("ja"));
        assert_eq!(normalize_configured_locale("zh-CN"), Some("zh-Hans"));
        assert_eq!(normalize_configured_locale("pt"), Some("pt-BR"));
        assert_eq!(normalize_configured_locale("pt-PT"), Some("pt-BR"));
        assert_eq!(normalize_configured_locale("zh-TW"), None);
    }

    #[test]
    fn locale_resolution_uses_config_then_environment_then_english() {
        assert_eq!(
            resolve_locale_with_env("ja", |_| Some("pt_BR.UTF-8".to_string())),
            Locale::Ja
        );
        assert_eq!(
            resolve_locale_with_env("auto", |key| {
                (key == "LANG").then(|| "zh_CN.UTF-8".to_string())
            }),
            Locale::ZhHans
        );
        assert_eq!(resolve_locale_with_env("auto", |_| None), Locale::En);
    }

    #[test]
    fn shipped_first_pack_has_no_missing_core_messages() {
        for locale in Locale::shipped() {
            assert!(
                missing_message_ids(*locale).is_empty(),
                "{} is missing messages",
                locale.tag()
            );
        }
    }

    #[test]
    fn unsupported_locale_falls_back_to_english() {
        assert_eq!(
            resolve_locale_with_env("ar", |_| None),
            Locale::En,
            "Arabic is planned for QA but not shipped in the v0.7.6 core pack"
        );
    }

    #[test]
    fn missing_translation_falls_back_to_english() {
        assert_eq!(
            fallback_translation(None, MessageId::ComposerPlaceholder),
            english(MessageId::ComposerPlaceholder)
        );
    }

    #[test]
    fn width_truncation_handles_cjk_rtl_indic_and_latin_samples() {
        let samples = [
            ("zh-Hans", "输入以筛选配置"),
            ("ar", "تصفية الإعدادات"),
            ("hi", "सेटिंग खोजें"),
            ("pt-BR", "configurações filtradas"),
        ];

        for (tag, sample) in samples {
            let truncated = truncate_to_width(sample, 12);
            assert!(
                truncated.width() <= 12,
                "{tag} sample overflowed: {truncated:?}"
            );
        }
    }

    #[test]
    fn planned_script_samples_render_in_narrow_terminal_buffer() {
        let samples = [
            ("CJK", "输入以筛选配置"),
            ("RTL", "تصفية الإعدادات"),
            ("Indic", "सेटिंग खोजें"),
            ("Latin Global South", "configurações filtradas"),
        ];

        for (label, sample) in samples {
            let area = Rect::new(0, 0, 18, 4);
            let mut buf = Buffer::empty(area);
            Paragraph::new(sample)
                .wrap(Wrap { trim: false })
                .render(area, &mut buf);
            let dump = buffer_text(&buf, area);

            assert!(
                dump.chars().any(|ch| !ch.is_whitespace()),
                "{label} sample produced an empty render"
            );
        }
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}
