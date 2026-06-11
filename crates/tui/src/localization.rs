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
    ZhHant,
    PtBr,
    Es419,
    Vi,
}

impl Locale {
    pub fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ja => "ja",
            Self::ZhHans => "zh-Hans",
            Self::ZhHant => "zh-Hant",
            Self::PtBr => "pt-BR",
            Self::Es419 => "es-419",
            Self::Vi => "vi",
        }
    }

    pub fn translation_target_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Ja => "Japanese (日本語)",
            Self::ZhHans => "Simplified Chinese (简体中文)",
            Self::ZhHant => "Traditional Chinese (繁體中文)",
            Self::PtBr => "Brazilian Portuguese (Português do Brasil)",
            Self::Es419 => "Latin American Spanish (Español latinoamericano)",
            Self::Vi => "Vietnamese (Tiếng Việt)",
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
            Self::ZhHant => LocaleSpec {
                tag: "zh-Hant",
                display_name: "Chinese Traditional",
                script: "Hant",
                direction: TextDirection::Ltr,
                fallback: "zh-Hans",
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
            Self::Es419 => LocaleSpec {
                tag: "es-419",
                display_name: "Spanish (Latin America)",
                script: "Latin",
                direction: TextDirection::Ltr,
                fallback: "en",
                coverage: LocaleCoverage::V076Core,
            },
            Self::Vi => LocaleSpec {
                tag: "vi",
                display_name: "Vietnamese",
                script: "Latin",
                direction: TextDirection::Ltr,
                fallback: "en",
                coverage: LocaleCoverage::V076Core,
            },
        }
    }

    #[allow(dead_code)]
    pub fn shipped() -> &'static [Self] {
        &[
            Self::En,
            Self::Ja,
            Self::ZhHans,
            Self::ZhHant,
            Self::PtBr,
            Self::Es419,
            Self::Vi,
        ]
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
    // StatusPicker — `/statusline` multi-select footer-item picker.
    StatusPickerTitle,
    StatusPickerInstruction,
    StatusPickerActionToggle,
    StatusPickerActionAll,
    StatusPickerActionNone,
    StatusPickerActionSave,
    StatusPickerActionCancel,
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
    CmdChangeDescription,
    CmdChangeHeader,
    CmdChangeTranslationQueued,
    CmdChangeTranslationUnavailable,
    CmdChangePreviousVersion,
    CmdBalanceDescription,
    CmdClearDescription,
    CmdCompactDescription,
    CmdPurgeDescription,
    CmdConfigDescription,
    CmdContextDescription,
    CmdCostDescription,
    CmdDiffDescription,
    CmdEditDescription,
    CmdExitDescription,
    CmdExportDescription,
    CmdFeedbackDescription,
    CmdHfDescription,
    CmdHelpDescription,
    CmdHomeDescription,
    CmdHooksDescription,
    CmdAgentDescription,
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
    CmdQueueUsage,
    CmdQueueDraftHeader,
    CmdQueueNoMessages,
    CmdQueueListHeader,
    CmdQueueTip,
    CmdQueueAlreadyEditing,
    CmdQueueNotFound,
    CmdQueueEditingStatus,
    CmdQueueEditingMessage,
    CmdQueueDropped,
    CmdQueueAlreadyEmpty,
    CmdQueueCleared,
    CmdQueueMissingIndex,
    CmdQueueIndexPositive,
    CmdQueueIndexMin,
    CmdRelayDescription,
    CmdRenameDescription,
    CmdRestoreDescription,
    CmdRetryDescription,
    CmdReviewDescription,
    CmdRlmDescription,
    CmdSaveDescription,
    CmdForkDescription,
    CmdNewDescription,
    CmdSessionsDescription,
    CmdSettingsDescription,
    CmdSidebarDescription,
    CmdSkillDescription,
    CmdSkillsDescription,
    CmdSlopDescription,
    CmdStashDescription,
    CmdStatusDescription,
    CmdStatuslineDescription,
    CmdSubagentsDescription,
    CmdSwarmDescription,
    CmdSystemDescription,
    CmdTaskDescription,
    CmdTokensDescription,
    CmdTranslateDescription,
    CmdTranslateOff,
    CmdTranslateOn,
    TranslationInProgress,
    TranslationComplete,
    TranslationFailed,
    CmdTrustDescription,
    CmdLspDescription,
    CmdShareDescription,
    CmdWorkspaceDescription,
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
    FooterBalancePrefix,
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
    KbBrowseHistory,
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
    HomeGoalModeTip,
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
    // Context menu.
    CtxMenuTitle,
    CtxMenuCopySelection,
    CtxMenuCopySelectionDesc,
    CtxMenuOpenSelection,
    CtxMenuOpenSelectionDesc,
    CtxMenuClearSelection,
    CtxMenuOpenDetails,
    CtxMenuCopyMessage,
    CtxMenuCopyMessageDesc,
    CtxMenuOpenInEditor,
    CtxMenuOpenInEditorDesc,
    CtxMenuShowCell,
    CtxMenuShowCellDesc,
    CtxMenuHideCell,
    CtxMenuHideCellDesc,
    CtxMenuShowHidden,
    CtxMenuShowHiddenDesc,
    CtxMenuPaste,
    CtxMenuPasteDesc,
    CtxMenuCmdPalette,
    CtxMenuCmdPaletteDesc,
    CtxMenuContextInspector,
    CtxMenuContextInspectorDesc,
    CtxMenuHelp,
    CtxMenuHelpDesc,
    // Agent fanout card.
    FanoutCounts,

    // Approval dialog — risk badges, category labels, field labels, options.
    ApprovalRiskReview,
    ApprovalRiskDestructive,
    ApprovalCategorySafe,
    ApprovalCategoryFileWrite,
    ApprovalCategoryShell,
    ApprovalCategoryNetwork,
    ApprovalCategoryMcpRead,
    ApprovalCategoryMcpAction,
    ApprovalCategoryUnknown,
    ApprovalFieldType,
    ApprovalFieldAbout,
    ApprovalFieldImpact,
    ApprovalFieldParams,
    ApprovalOptionApproveOnce,
    ApprovalOptionApproveAlways,
    ApprovalOptionDeny,
    ApprovalOptionAbortTurn,
    ApprovalBlockTitle,
    ApprovalControlsHint,
    ApprovalChooseHint,
    ApprovalChooseAction,
    ApprovalIntentLabel,
    ApprovalMoreLines,

    CtxInspTitle,
    CtxInspSessionContext,
    CtxInspSystemPrompt,
    CtxInspReferences,
    CtxInspRecentTools,
    CtxInspModel,
    CtxInspWorkspace,
    CtxInspSession,
    CtxInspContext,
    CtxInspTranscript,
    CtxInspWorkspaceStatus,
    CtxInspNotSampledYet,
    CtxInspOk,
    CtxInspHigh,
    CtxInspCritical,
    CtxInspIncluded,
    CtxInspAttached,
    CtxInspNotIncluded,
    CtxInspOutputCaptured,
    CtxInspNoOutputYet,
    CtxInspNoSystemPrompt,
    CtxInspNoReferences,
    CtxInspNoToolActivity,
    CtxInspAltVHint,
    CtxInspCells,
    CtxInspApiMessages,
    CtxInspActive,
    CtxInspCell,
    CtxInspMoreReferences,
    CtxInspStablePrefix,
    CtxInspVolatileWorkingSet,
    CtxInspFirstLine,
    CtxInspTotal,
    CtxInspTextPromptLayers,
    CtxInspSingleTextBlob,
    CtxInspBlocks,
    CtxInspBlock,
    CtxInspTokens,
    CtxInspLayers,
    CtxInspNone,
    CtxInspEmpty,
    CtxInspCacheFriendly,
    CtxInspChangesByTurn,
    CtxInspStablePrefixOnly,
    CtxInspCacheTip,
    // Voice command
    CmdVoiceDescription,
    CmdVoiceSendDescription,
    CmdVoiceControlDescription,
    VoiceEnabled,
    VoiceDisabled,
    VoiceSendEnabled,
    VoiceSendDisabled,
    VoiceControlEnabled,
    VoiceControlDisabled,
    VoiceErrNoAuth,
    VoiceErrNoRecorder,
    VoiceErrNetwork,
    VoiceErrEmptySend,
    VoiceErrUnknownAgent,
    VoiceRecording,
    VoiceProcessing,
    VoiceTranscribed,
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
    MessageId::StatusPickerTitle,
    MessageId::StatusPickerInstruction,
    MessageId::StatusPickerActionToggle,
    MessageId::StatusPickerActionAll,
    MessageId::StatusPickerActionNone,
    MessageId::StatusPickerActionSave,
    MessageId::StatusPickerActionCancel,
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
    MessageId::CmdBalanceDescription,
    MessageId::CmdCacheDescription,
    MessageId::CmdClearDescription,
    MessageId::CmdCompactDescription,
    MessageId::CmdPurgeDescription,
    MessageId::CmdConfigDescription,
    MessageId::CmdContextDescription,
    MessageId::CmdCostDescription,
    MessageId::CmdDiffDescription,
    MessageId::CmdEditDescription,
    MessageId::CmdExitDescription,
    MessageId::CmdExportDescription,
    MessageId::CmdFeedbackDescription,
    MessageId::CmdHfDescription,
    MessageId::CmdHelpDescription,
    MessageId::CmdHomeDescription,
    MessageId::CmdHooksDescription,
    MessageId::CmdAgentDescription,
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
    MessageId::CmdQueueUsage,
    MessageId::CmdQueueDraftHeader,
    MessageId::CmdQueueNoMessages,
    MessageId::CmdQueueListHeader,
    MessageId::CmdQueueTip,
    MessageId::CmdQueueAlreadyEditing,
    MessageId::CmdQueueNotFound,
    MessageId::CmdQueueEditingStatus,
    MessageId::CmdQueueEditingMessage,
    MessageId::CmdQueueDropped,
    MessageId::CmdQueueAlreadyEmpty,
    MessageId::CmdQueueCleared,
    MessageId::CmdQueueMissingIndex,
    MessageId::CmdQueueIndexPositive,
    MessageId::CmdQueueIndexMin,
    MessageId::CmdRelayDescription,
    MessageId::CmdRenameDescription,
    MessageId::CmdRestoreDescription,
    MessageId::CmdRetryDescription,
    MessageId::CmdReviewDescription,
    MessageId::CmdRlmDescription,
    MessageId::CmdSaveDescription,
    MessageId::CmdNewDescription,
    MessageId::CmdSessionsDescription,
    MessageId::CmdSettingsDescription,
    MessageId::CmdSidebarDescription,
    MessageId::CmdSkillDescription,
    MessageId::CmdSkillsDescription,
    MessageId::CmdSlopDescription,
    MessageId::CmdStashDescription,
    MessageId::CmdStatusDescription,
    MessageId::CmdStatuslineDescription,
    MessageId::CmdSubagentsDescription,
    MessageId::CmdSwarmDescription,
    MessageId::CmdSystemDescription,
    MessageId::CmdTaskDescription,
    MessageId::CmdTokensDescription,
    MessageId::CmdTranslateDescription,
    MessageId::CmdTranslateOff,
    MessageId::CmdTranslateOn,
    MessageId::TranslationInProgress,
    MessageId::TranslationComplete,
    MessageId::TranslationFailed,
    MessageId::CmdTrustDescription,
    MessageId::CmdLspDescription,
    MessageId::CmdShareDescription,
    MessageId::CmdWorkspaceDescription,
    MessageId::CmdUndoDescription,
    MessageId::CmdVerboseDescription,
    MessageId::CmdCacheAdvice,
    MessageId::CmdCacheFootnote,
    MessageId::CmdCacheHeader,
    MessageId::CmdCacheNoData,
    MessageId::CmdCacheTotals,
    MessageId::CmdChangeDescription,
    MessageId::CmdChangeHeader,
    MessageId::CmdChangeTranslationQueued,
    MessageId::CmdChangeTranslationUnavailable,
    MessageId::CmdChangePreviousVersion,
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
    MessageId::FooterBalancePrefix,
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
    MessageId::KbBrowseHistory,
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
    MessageId::HomeGoalModeTip,
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
    // Context menu.
    MessageId::CtxMenuTitle,
    MessageId::CtxMenuCopySelection,
    MessageId::CtxMenuCopySelectionDesc,
    MessageId::CtxMenuOpenSelection,
    MessageId::CtxMenuOpenSelectionDesc,
    MessageId::CtxMenuClearSelection,
    MessageId::CtxMenuOpenDetails,
    MessageId::CtxMenuCopyMessage,
    MessageId::CtxMenuCopyMessageDesc,
    MessageId::CtxMenuOpenInEditor,
    MessageId::CtxMenuOpenInEditorDesc,
    MessageId::CtxMenuShowCell,
    MessageId::CtxMenuShowCellDesc,
    MessageId::CtxMenuHideCell,
    MessageId::CtxMenuHideCellDesc,
    MessageId::CtxMenuShowHidden,
    MessageId::CtxMenuShowHiddenDesc,
    MessageId::CtxMenuPaste,
    MessageId::CtxMenuPasteDesc,
    MessageId::CtxMenuCmdPalette,
    MessageId::CtxMenuCmdPaletteDesc,
    MessageId::CtxMenuContextInspector,
    MessageId::CtxMenuContextInspectorDesc,
    MessageId::CtxMenuHelp,
    MessageId::CtxMenuHelpDesc,
    MessageId::FanoutCounts,
    MessageId::ApprovalRiskReview,
    MessageId::ApprovalRiskDestructive,
    MessageId::ApprovalCategorySafe,
    MessageId::ApprovalCategoryFileWrite,
    MessageId::ApprovalCategoryShell,
    MessageId::ApprovalCategoryNetwork,
    MessageId::ApprovalCategoryMcpRead,
    MessageId::ApprovalCategoryMcpAction,
    MessageId::ApprovalCategoryUnknown,
    MessageId::ApprovalFieldType,
    MessageId::ApprovalFieldAbout,
    MessageId::ApprovalFieldImpact,
    MessageId::ApprovalFieldParams,
    MessageId::ApprovalOptionApproveOnce,
    MessageId::ApprovalOptionApproveAlways,
    MessageId::ApprovalOptionDeny,
    MessageId::ApprovalOptionAbortTurn,
    MessageId::ApprovalBlockTitle,
    MessageId::ApprovalControlsHint,
    MessageId::ApprovalChooseHint,
    MessageId::ApprovalChooseAction,
    MessageId::ApprovalIntentLabel,
    MessageId::ApprovalMoreLines,
    MessageId::CtxInspTitle,
    MessageId::CtxInspSessionContext,
    MessageId::CtxInspSystemPrompt,
    MessageId::CtxInspReferences,
    MessageId::CtxInspRecentTools,
    MessageId::CtxInspModel,
    MessageId::CtxInspWorkspace,
    MessageId::CtxInspSession,
    MessageId::CtxInspContext,
    MessageId::CtxInspTranscript,
    MessageId::CtxInspWorkspaceStatus,
    MessageId::CtxInspNotSampledYet,
    MessageId::CtxInspOk,
    MessageId::CtxInspHigh,
    MessageId::CtxInspCritical,
    MessageId::CtxInspIncluded,
    MessageId::CtxInspAttached,
    MessageId::CtxInspNotIncluded,
    MessageId::CtxInspOutputCaptured,
    MessageId::CtxInspNoOutputYet,
    MessageId::CtxInspNoSystemPrompt,
    MessageId::CtxInspNoReferences,
    MessageId::CtxInspNoToolActivity,
    MessageId::CtxInspAltVHint,
    MessageId::CtxInspCells,
    MessageId::CtxInspApiMessages,
    MessageId::CtxInspActive,
    MessageId::CtxInspCell,
    MessageId::CtxInspMoreReferences,
    MessageId::CtxInspStablePrefix,
    MessageId::CtxInspVolatileWorkingSet,
    MessageId::CtxInspFirstLine,
    MessageId::CtxInspTotal,
    MessageId::CtxInspTextPromptLayers,
    MessageId::CtxInspSingleTextBlob,
    MessageId::CtxInspBlocks,
    MessageId::CtxInspBlock,
    MessageId::CtxInspTokens,
    MessageId::CtxInspLayers,
    MessageId::CtxInspNone,
    MessageId::CtxInspEmpty,
    MessageId::CtxInspCacheFriendly,
    MessageId::CtxInspChangesByTurn,
    MessageId::CtxInspStablePrefixOnly,
    MessageId::CtxInspCacheTip,
    MessageId::CmdVoiceDescription,
    MessageId::CmdVoiceSendDescription,
    MessageId::CmdVoiceControlDescription,
    MessageId::VoiceEnabled,
    MessageId::VoiceDisabled,
    MessageId::VoiceSendEnabled,
    MessageId::VoiceSendDisabled,
    MessageId::VoiceControlEnabled,
    MessageId::VoiceControlDisabled,
    MessageId::VoiceErrNoAuth,
    MessageId::VoiceErrNoRecorder,
    MessageId::VoiceErrNetwork,
    MessageId::VoiceErrEmptySend,
    MessageId::VoiceErrUnknownAgent,
    MessageId::VoiceRecording,
    MessageId::VoiceProcessing,
    MessageId::VoiceTranscribed,
];

pub fn tr(locale: Locale, id: MessageId) -> &'static str {
    fallback_translation(translation(locale, id), id)
}

pub fn thinking_translation_placeholder(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Thinking; translating when complete...",
        Locale::Ja => "思考中です。完了後に日本語へ翻訳します...",
        Locale::ZhHans => "正在思考，完成后翻译为简体中文...",
        Locale::ZhHant => "正在思考，完成後翻譯為繁體中文...",
        Locale::PtBr => "Pensando; traduzindo ao concluir...",
        Locale::Es419 => "Pensando; traduciendo al finalizar...",
        Locale::Vi => "Đang suy nghĩ; sẽ dịch sau khi hoàn thành...",
    }
}

pub fn thinking_translation_in_progress(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Translating thinking content...",
        Locale::Ja => "思考内容を翻訳中...",
        Locale::ZhHans => "正在翻译思考内容...",
        Locale::ZhHant => "正在翻譯思考內容...",
        Locale::PtBr => "Traduzindo o conteúdo de raciocínio...",
        Locale::Es419 => "Traduciendo el contenido de razonamiento...",
        Locale::Vi => "Đang dịch nội dung suy nghĩ...",
    }
}

pub fn thinking_translation_complete(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Thinking translation complete",
        Locale::Ja => "思考内容の翻訳が完了しました",
        Locale::ZhHans => "思考内容翻译完成",
        Locale::ZhHant => "思考內容翻譯完成",
        Locale::PtBr => "Tradução do raciocínio concluída",
        Locale::Es419 => "Traducción del razonamiento completada",
        Locale::Vi => "Đã dịch xong nội dung suy nghĩ",
    }
}

pub fn thinking_translation_failed(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Thinking translation failed",
        Locale::Ja => "思考内容の翻訳に失敗しました",
        Locale::ZhHans => "思考内容翻译失败",
        Locale::ZhHant => "思考內容翻譯失敗",
        Locale::PtBr => "Falha ao traduzir o raciocínio",
        Locale::Es419 => "Falló la traducción del razonamiento",
        Locale::Vi => "Dịch nội dung suy nghĩ thất bại",
    }
}

pub fn hidden_translation_failed(locale: Locale) -> &'static str {
    match locale {
        Locale::En => "Translation failed; original text is hidden.",
        Locale::Ja => "翻訳に失敗しました。原文は非表示です。",
        Locale::ZhHans => "翻译失败，原文已隐藏。",
        Locale::ZhHant => "翻譯失敗，原文已隱藏。",
        Locale::PtBr => "A tradução falhou; o texto original está oculto.",
        Locale::Es419 => "La traducción falló; el texto original está oculto.",
        Locale::Vi => "Dịch thất bại; văn bản gốc đã bị ẩn.",
    }
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
            return Some(Locale::ZhHant);
        }
        return Some(Locale::ZhHans);
    }
    if value.starts_with("pt") || value == "br" {
        return Some(Locale::PtBr);
    }
    if value.starts_with("es") {
        return Some(Locale::Es419);
    }
    if value.starts_with("vi") {
        return Some(Locale::Vi);
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
        MessageId::StatusPickerTitle => " Status line ",
        MessageId::StatusPickerInstruction => "Pick the chips you want in the footer:",
        MessageId::StatusPickerActionToggle => "toggle ",
        MessageId::StatusPickerActionAll => "all ",
        MessageId::StatusPickerActionNone => "none ",
        MessageId::StatusPickerActionSave => "save ",
        MessageId::StatusPickerActionCancel => "cancel ",
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
        MessageId::CmdChangeDescription => "Show the latest changelog entry",
        MessageId::CmdChangeHeader => "Latest Changelog",
        MessageId::CmdChangeTranslationQueued => {
            "English release notes are shown below. A translated version will be requested next; if the provider is unavailable, this English text is the fallback."
        }
        MessageId::CmdChangeTranslationUnavailable => {
            "English release notes are shown below. Translation is unavailable because the current session has no API key or is offline."
        }
        MessageId::CmdChangePreviousVersion => {
            "Previous version: {version} — run `/change {version}` to view it"
        }
        MessageId::CmdBalanceDescription => "Check the active provider account balance",
        MessageId::CmdClearDescription => "Clear conversation history",
        MessageId::CmdCompactDescription => "Trigger context compaction to free up space",
        MessageId::CmdPurgeDescription => {
            "Let the agent surgically prune conversation history to free context space"
        }
        MessageId::CmdConfigDescription => "Open interactive configuration editor",
        MessageId::CmdContextDescription => "Open compact session context inspector",
        MessageId::CmdCostDescription => "Show session cost breakdown",
        MessageId::CmdDiffDescription => "Show file changes since session start",
        MessageId::CmdEditDescription => "Revise and resubmit the last message",
        MessageId::CmdExitDescription => "Exit the application",
        MessageId::CmdExportDescription => "Export conversation to markdown",
        MessageId::CmdFeedbackDescription => "Generate a GitHub feedback URL",
        MessageId::CmdHfDescription => "Inspect Hugging Face MCP setup and concepts",
        MessageId::CmdHelpDescription => "Show help information",
        MessageId::CmdHomeDescription => "Show home dashboard with stats and quick actions",
        MessageId::CmdHooksDescription => "List configured lifecycle hooks (read-only)",
        MessageId::CmdAgentDescription => {
            "Open a persistent sub-agent session: /agent [0-3] <task>"
        }
        MessageId::CmdGoalDescription => "Set a session goal with optional token budget",
        MessageId::CmdInitDescription => "Generate AGENTS.md for project",
        MessageId::CmdLspDescription => "Toggle LSP diagnostics on or off",
        MessageId::CmdShareDescription => "Export current session as a shareable web URL",
        MessageId::CmdJobsDescription => "Inspect and control background commands",
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
        MessageId::CmdThemeDescription => "Switch theme or open the theme picker",
        MessageId::CmdProviderDescription => "Switch the active provider and/or model",
        MessageId::CmdQueueDescription => "View or edit queued messages",
        MessageId::CmdQueueUsage => "Usage: /queue [list|edit <n>|drop <n>|clear]",
        MessageId::CmdQueueDraftHeader => "Editing queued message:",
        MessageId::CmdQueueNoMessages => "No queued messages",
        MessageId::CmdQueueListHeader => "Queued messages ({count}):",
        MessageId::CmdQueueTip => "Tip: /queue edit <n> to edit, /queue drop <n> to remove",
        MessageId::CmdQueueAlreadyEditing => {
            "Already editing a queued message. Send it or /queue clear to discard."
        }
        MessageId::CmdQueueNotFound => "Queued message not found",
        MessageId::CmdQueueEditingStatus => "Editing queued message {index}",
        MessageId::CmdQueueEditingMessage => {
            "Editing queued message {index} (press Enter to re-queue/send)"
        }
        MessageId::CmdQueueDropped => "Dropped queued message {index}",
        MessageId::CmdQueueAlreadyEmpty => "Queue already empty",
        MessageId::CmdQueueCleared => "Queue cleared",
        MessageId::CmdQueueMissingIndex => {
            "Missing index. Usage: /queue edit <n> or /queue drop <n>"
        }
        MessageId::CmdQueueIndexPositive => "Index must be a positive number",
        MessageId::CmdQueueIndexMin => "Index must be >= 1",
        MessageId::CmdRelayDescription => "Create a session relay (接力) for a fresh thread",
        MessageId::CmdRenameDescription => "Rename the current session",
        MessageId::CmdRestoreDescription => {
            "Roll back the workspace to a prior pre/post-turn snapshot. With no arg, lists recent snapshots."
        }
        MessageId::CmdRetryDescription => "Retry the last request",
        MessageId::CmdReviewDescription => "Run a structured code review on a file, diff, or PR",
        MessageId::CmdRlmDescription => "Open a persistent RLM context: /rlm [0-3] <file_or_text>",
        MessageId::CmdSaveDescription => "Save session to file",
        MessageId::CmdForkDescription => "Fork the active conversation into a sibling session",
        MessageId::CmdNewDescription => "Start a fresh saved session",
        MessageId::CmdSessionsDescription => "Open session history picker",
        MessageId::CmdSettingsDescription => "Show persistent settings",
        MessageId::CmdSidebarDescription => "Toggle or focus the right sidebar",
        MessageId::CmdSkillDescription => {
            "Activate a skill, or install/update/uninstall/trust a community skill"
        }
        MessageId::CmdSkillsDescription => {
            "List local skills (filter by `/skills <prefix>`; --remote browses the curated registry)"
        }
        MessageId::CmdSlopDescription => "Inspect or export the SlopLedger",
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
        MessageId::CmdTranslateDescription => {
            "Toggle output translation to the current system language on/off"
        }
        MessageId::CmdTranslateOff => "Output translation disabled (original model output shown)",
        MessageId::CmdTranslateOn => {
            "Output translation enabled: model responses will be shown in your system language"
        }
        MessageId::TranslationInProgress => "Translating assistant output...",
        MessageId::TranslationComplete => "Translation complete",
        MessageId::TranslationFailed => "Translation failed",
        MessageId::CmdTrustDescription => {
            "Manage workspace trust and per-path allowlist (`/trust add <path>`, `/trust list`, `/trust on|off`)"
        }
        MessageId::CmdWorkspaceDescription => "Show or switch the current workspace",
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
        MessageId::FooterBalancePrefix => "balance",
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
        MessageId::KbBrowseHistory => "Browse conversation history",
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
        MessageId::KbThinkingPager => "Open Activity Detail",
        MessageId::KbLiveTranscript => "Open live transcript overlay (sticky-tail auto-scroll)",
        MessageId::KbBacktrackMessage => {
            "Backtrack to a previous user message (Left/Right step, Enter to rewind)"
        }
        MessageId::KbCompleteCycleModes => {
            "Complete /command, queue running-turn follow-up, cycle modes; Shift+Tab cycles reasoning effort"
        }
        MessageId::KbJumpPlanAgentYolo => "Jump directly to Plan / Agent / YOLO mode",
        MessageId::KbAltJumpPlanAgentYolo => "Alternative jump to Plan / Agent / YOLO mode",
        MessageId::KbFocusSidebar => {
            "Focus Work / Tasks / Agents / Context / Auto sidebar; Ctrl+Alt+0 hides it"
        }
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
        MessageId::HomeDashboardTitle => "codewhale Home Dashboard",
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
        MessageId::HomeGoalModeTip => "Goal tracking - Set /goal <objective> to pursue objectives",
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "Choose your language",
        MessageId::OnboardLanguageBlurb => {
            "Pick the UI language. You can change it any time with `/settings set locale <tag>`."
        }
        MessageId::OnboardLanguageFooter => {
            "Press 1-7 to choose, or Enter to keep the current setting"
        }
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "Connect your DeepSeek API key",
        MessageId::OnboardApiKeyStep1 => {
            "Step 1.  Open https://platform.deepseek.com/api_keys and create a key."
        }
        MessageId::OnboardApiKeyStep2 => "Step 2.  Paste it below and press Enter.",
        MessageId::OnboardApiKeySavedHint => {
            "Saved to ~/.codewhale/config.toml so it works from any folder."
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
        // Context menu.
        MessageId::CtxMenuTitle => " Right click ",
        MessageId::CtxMenuCopySelection => "Copy selection",
        MessageId::CtxMenuCopySelectionDesc => "write selected transcript text",
        MessageId::CtxMenuOpenSelection => "Open selection",
        MessageId::CtxMenuOpenSelectionDesc => "show selected text in pager",
        MessageId::CtxMenuClearSelection => "Clear selection",
        MessageId::CtxMenuOpenDetails => "Open details",
        MessageId::CtxMenuCopyMessage => "Copy message",
        MessageId::CtxMenuCopyMessageDesc => "write clicked transcript cell",
        MessageId::CtxMenuOpenInEditor => "Open in editor",
        MessageId::CtxMenuOpenInEditorDesc => "open file:line in $EDITOR",
        MessageId::CtxMenuShowCell => "Show cell",
        MessageId::CtxMenuShowCellDesc => "unhide this transcript cell",
        MessageId::CtxMenuHideCell => "Hide cell",
        MessageId::CtxMenuHideCellDesc => "collapse this transcript cell",
        MessageId::CtxMenuShowHidden => "Show hidden",
        MessageId::CtxMenuShowHiddenDesc => "unhide all collapsed cells",
        MessageId::CtxMenuPaste => "Paste",
        MessageId::CtxMenuPasteDesc => "insert clipboard into composer",
        MessageId::CtxMenuCmdPalette => "Command palette",
        MessageId::CtxMenuCmdPaletteDesc => "commands, skills, and tools",
        MessageId::CtxMenuContextInspector => "Context inspector",
        MessageId::CtxMenuContextInspectorDesc => "active context and cache hints",
        MessageId::CtxMenuHelp => "Help",
        MessageId::CtxMenuHelpDesc => "keybindings and commands",
        MessageId::FanoutCounts => {
            "{done} done · {running} running · {failed} failed · {pending} pending"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "REVIEW",
        MessageId::ApprovalRiskDestructive => "DESTRUCTIVE",
        MessageId::ApprovalCategorySafe => "Safe",
        MessageId::ApprovalCategoryFileWrite => "File Write",
        MessageId::ApprovalCategoryShell => "Shell Command",
        MessageId::ApprovalCategoryNetwork => "Network",
        MessageId::ApprovalCategoryMcpRead => "MCP Read",
        MessageId::ApprovalCategoryMcpAction => "MCP Action",
        MessageId::ApprovalCategoryUnknown => "Unknown",
        MessageId::ApprovalFieldType => "Type: ",
        MessageId::ApprovalFieldAbout => "About:  ",
        MessageId::ApprovalFieldImpact => "Impact: ",
        MessageId::ApprovalFieldParams => "Params: ",
        MessageId::ApprovalOptionApproveOnce => "Approve once",
        MessageId::ApprovalOptionApproveAlways => "Approve always for this kind",
        MessageId::ApprovalOptionDeny => "Deny this call",
        MessageId::ApprovalOptionAbortTurn => "Abort the turn",
        MessageId::ApprovalBlockTitle => "approval",
        MessageId::ApprovalControlsHint => "  ·  v: full params  ·  Esc: abort",
        MessageId::ApprovalChooseHint => "Choose: ",
        MessageId::ApprovalChooseAction => "Enter selected option, or press y/a/d directly",
        MessageId::ApprovalIntentLabel => "Intent: ",
        MessageId::ApprovalMoreLines => "  … (+{count} lines)",

        MessageId::CtxInspTitle => "Context inspector",
        MessageId::CtxInspSessionContext => "Session Context",
        MessageId::CtxInspSystemPrompt => "System Prompt Structure",
        MessageId::CtxInspReferences => "References",
        MessageId::CtxInspRecentTools => "Recent Tools",
        MessageId::CtxInspModel => "Model",
        MessageId::CtxInspWorkspace => "Workspace",
        MessageId::CtxInspSession => "Session",
        MessageId::CtxInspContext => "Context",
        MessageId::CtxInspTranscript => "Transcript",
        MessageId::CtxInspWorkspaceStatus => "Workspace status",
        MessageId::CtxInspNotSampledYet => "not sampled yet",
        MessageId::CtxInspOk => "ok",
        MessageId::CtxInspHigh => "high",
        MessageId::CtxInspCritical => "critical",
        MessageId::CtxInspIncluded => "included",
        MessageId::CtxInspAttached => "attached",
        MessageId::CtxInspNotIncluded => "not included",
        MessageId::CtxInspOutputCaptured => "output captured",
        MessageId::CtxInspNoOutputYet => "no output yet",
        MessageId::CtxInspNoSystemPrompt => "No system prompt set.",
        MessageId::CtxInspNoReferences => "No file, directory, or media references recorded yet.",
        MessageId::CtxInspNoToolActivity => "No tool activity recorded yet.",
        MessageId::CtxInspAltVHint => "Open the matching card and press Alt+V for full details.",
        MessageId::CtxInspCells => "cells",
        MessageId::CtxInspApiMessages => "API messages",
        MessageId::CtxInspActive => "active",
        MessageId::CtxInspCell => "cell",
        MessageId::CtxInspMoreReferences => "more reference(s)",
        MessageId::CtxInspStablePrefix => "Stable prefix",
        MessageId::CtxInspVolatileWorkingSet => "Volatile working set",
        MessageId::CtxInspFirstLine => "First line",
        MessageId::CtxInspTotal => "Total",
        MessageId::CtxInspTextPromptLayers => "Text prompt layers",
        MessageId::CtxInspSingleTextBlob => "Single text blob",
        MessageId::CtxInspBlocks => "block(s)",
        MessageId::CtxInspBlock => "block",
        MessageId::CtxInspTokens => "tokens",
        MessageId::CtxInspLayers => "layer(s)",
        MessageId::CtxInspNone => "none",
        MessageId::CtxInspEmpty => "(empty)",
        MessageId::CtxInspCacheFriendly => "cache-friendly",
        MessageId::CtxInspChangesByTurn => "changes by session/turn",
        MessageId::CtxInspStablePrefixOnly => "stable prefix only",
        MessageId::CtxInspCacheTip => {
            "Tip: Stable prefix blocks are DeepSeek V4 prefix-cache eligible. \
            Volatile working-set changes break the cache only for the tail."
        }
        // Voice command
        MessageId::CmdVoiceDescription => {
            "Toggle voice input: record speech and transcribe to text"
        }
        MessageId::CmdVoiceSendDescription => {
            "Toggle auto-send: submit after transcription when ending with send/send it"
        }
        MessageId::CmdVoiceControlDescription => {
            "Toggle voice control: AI-powered edit/send/agent actions from speech"
        }
        MessageId::VoiceEnabled => "Voice input enabled. Speak to record.",
        MessageId::VoiceDisabled => "Voice input disabled.",
        MessageId::VoiceSendEnabled => "Voice auto-send enabled.",
        MessageId::VoiceSendDisabled => "Voice auto-send disabled.",
        MessageId::VoiceControlEnabled => "Voice control enabled.",
        MessageId::VoiceControlDisabled => "Voice control disabled.",
        MessageId::VoiceErrNoAuth => "Voice: no API key configured for the active provider",
        MessageId::VoiceErrNoRecorder => {
            "Voice: no recording tool found. Install sox, arecord, or rec."
        }
        MessageId::VoiceErrNetwork => "Voice: network error during transcription",
        MessageId::VoiceErrEmptySend => "Voice: nothing to send",
        MessageId::VoiceErrUnknownAgent => "Voice: unknown agent",
        MessageId::VoiceRecording => "🎙 Recording... speak now",
        MessageId::VoiceProcessing => "🎙 Transcribing...",
        MessageId::VoiceTranscribed => "🎙 Transcribed",
    }
}

fn translation(locale: Locale, id: MessageId) -> Option<&'static str> {
    match locale {
        Locale::En => Some(english(id)),
        Locale::Ja => japanese(id),
        Locale::ZhHans => chinese_simplified(id),
        Locale::ZhHant => traditional_chinese(id),
        Locale::PtBr => portuguese_brazil(id),
        Locale::Es419 => spanish_latin_america(id),
        Locale::Vi => vietnamese(id),
    }
}

fn vietnamese(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ComposerPlaceholder => "Nhập nhiệm vụ hoặc sử dụng /.",
        MessageId::HistorySearchPlaceholder => "Tìm kiếm lịch sử câu lệnh...",
        MessageId::HistorySearchTitle => "Tìm kiếm lịch sử",
        MessageId::HistoryHintMove => "Lên/Xuống để di chuyển",
        MessageId::HistoryHintAccept => "Enter để chấp nhận",
        MessageId::HistoryHintRestore => "Esc để khôi phục",
        MessageId::HistoryNoMatches => "  Không tìm thấy kết quả",
        MessageId::StatusPickerTitle => " Dòng trạng thái ",
        MessageId::StatusPickerInstruction => {
            "Chọn các thành phần bạn muốn hiển thị ở cuối màn hình:"
        }
        MessageId::StatusPickerActionToggle => "bật/tắt ",
        MessageId::StatusPickerActionAll => "tất cả ",
        MessageId::StatusPickerActionNone => "không ",
        MessageId::StatusPickerActionSave => "lưu ",
        MessageId::StatusPickerActionCancel => "huỷ ",
        MessageId::ConfigTitle => "Cấu hình phiên làm việc",
        MessageId::ConfigModalTitle => " Cấu hình ",
        MessageId::ConfigSearchPlaceholder => "Nhập để lọc kết quả",
        MessageId::ConfigNoSettings => "  Không có cài đặt nào khả dụng.",
        MessageId::ConfigNoMatchesPrefix => "  Không có cài đặt nào khớp với ",
        MessageId::ConfigFilteredSettings => "  Cài đặt đã lọc",
        MessageId::ConfigShowing => "  Đang hiển thị",
        MessageId::ConfigFooterDefault => " gõ=lọc, Lên/Xuống=chọn, Enter/e=sửa, Esc/q=đóng ",
        MessageId::ConfigFooterScrollable => {
            " gõ=lọc, Lên/Xuống=chọn, Enter/e=sửa, PgUp/PgDn=cuộn, Esc/q=đóng "
        }
        MessageId::ConfigFooterFiltered => {
            " gõ=lọc, Backspace=xóa, Ctrl+U/Esc=xóa sạch, Enter=sửa "
        }
        MessageId::HelpTitle => "Trợ giúp",
        MessageId::HelpFilterPlaceholder => "Nhập để lọc",
        MessageId::HelpFilterPrefix => "Bộ lọc: ",
        MessageId::HelpNoMatches => "  Không tìm thấy kết quả.",
        MessageId::HelpSlashCommands => "Các lệnh bắt đầu bằng dấu gạch chéo (/)",
        MessageId::HelpKeybindings => "Phím tắt",
        MessageId::HelpFooterTypeFilter => " nhập để lọc ",
        MessageId::HelpFooterMove => "  Lên/Xuống để di chuyển ",
        MessageId::HelpFooterJump => " PgUp/PgDn để nhảy trang ",
        MessageId::HelpFooterClose => " Esc để đóng ",
        MessageId::CmdAnchorDescription => {
            "Ghim một dữ kiện không bị ảnh hưởng khi nén (tự động đưa vào ngữ cảnh)"
        }
        MessageId::CmdAttachDescription => {
            "Đính kèm hình ảnh/video; sử dụng @path cho tệp văn bản hoặc thư mục"
        }
        MessageId::CmdCacheDescription => {
            "Hiển thị thống kê hit/miss của bộ nhớ đệm tiền tố DeepSeek trong N lượt gần nhất"
        }
        MessageId::CmdChangeDescription => "Hiển thị thông tin nhật ký thay đổi mới nhất",
        MessageId::CmdChangeHeader => "Nhật Ký Thay Đổi Mới Nhất",
        MessageId::CmdChangeTranslationQueued => {
            "Ghi chú phát hành bằng tiếng Anh hiển thị bên dưới. Bản dịch sẽ được yêu cầu tiếp theo; nếu nhà cung cấp không khả dụng, văn bản tiếng Anh này sẽ được dùng làm dự phòng."
        }
        MessageId::CmdChangeTranslationUnavailable => {
            "Ghi chú phát hành bằng tiếng Anh hiển thị bên dưới. Bản dịch không khả dụng vì phiên hiện tại không có mã khóa API hoặc đang ngoại tuyến."
        }
        MessageId::CmdChangePreviousVersion => {
            "Phiên bản trước: {version} — chạy `/change {version}` để xem"
        }
        MessageId::CmdBalanceDescription => {
            "Kiểm tra số dư tài khoản của nhà cung cấp dịch vụ đang hoạt động"
        }
        MessageId::CmdClearDescription => "Xóa lịch sử trò chuyện",
        MessageId::CmdCompactDescription => "Kích hoạt nén ngữ cảnh để giải phóng không gian",
        MessageId::CmdPurgeDescription => {
            "Cho agent cắt gọn lịch sử trò chuyện để giải phóng ngữ cảnh"
        }
        MessageId::CmdConfigDescription => "Mở trình chỉnh sửa cấu hình tương tác",
        MessageId::CmdContextDescription => "Mở trình kiểm tra ngữ cảnh phiên thu gọn",
        MessageId::CmdCostDescription => "Hiển thị chi tiết chi phí của phiên làm việc",
        MessageId::CmdDiffDescription => "Hiển thị các thay đổi của tệp kể từ khi bắt đầu phiên",
        MessageId::CmdEditDescription => "Chỉnh sửa và gửi lại tin nhắn gần nhất",
        MessageId::CmdExitDescription => "Thoát ứng dụng",
        MessageId::CmdExportDescription => "Xuất cuộc trò chuyện sang định dạng Markdown",
        MessageId::CmdFeedbackDescription => "Tạo một URL để gửi phản hồi trên GitHub",
        MessageId::CmdHfDescription => "Kiểm tra thiết lập và khái niệm Hugging Face MCP",
        MessageId::CmdHelpDescription => "Hiển thị thông tin trợ giúp",
        MessageId::CmdHomeDescription => {
            "Hiển thị bảng điều khiển trang chủ với số liệu thống kê và hành động nhanh"
        }
        MessageId::CmdHooksDescription => "Liệt kê các lifecycle hook đã cấu hình (chỉ đọc)",
        MessageId::CmdAgentDescription => "Mở một phiên sub-agent nền: /agent [0-3] <nhiệm_vụ>",
        MessageId::CmdGoalDescription => "Đặt mục tiêu cho phiên với giới hạn token tùy chọn",
        MessageId::CmdInitDescription => "Tạo tệp AGENTS.md cho dự án",
        MessageId::CmdLspDescription => "Bật hoặc tắt tính năng chẩn đoán LSP",
        MessageId::CmdShareDescription => {
            "Xuất phiên hiện tại thành một liên kết web có thể chia sẻ"
        }
        MessageId::CmdJobsDescription => "Kiểm tra và kiểm soát các lệnh chạy ngầm",
        MessageId::CmdLinksDescription => {
            "Hiển thị các liên kết đến bảng điều khiển và tài liệu của DeepSeek"
        }
        MessageId::CmdLoadDescription => "Tải phiên làm việc từ tệp",
        MessageId::CmdLogoutDescription => "Xóa khóa API và quay lại bước thiết lập",
        MessageId::CmdMcpDescription => "Mở hoặc quản lý các máy chủ MCP",
        MessageId::CmdMemoryDescription => "Kiểm tra hoặc quản lý tệp bộ nhớ người dùng liên tục",
        MessageId::CmdModeDescription => {
            "Chuyển đổi chế độ hoặc mở bảng chọn: /mode [agent|plan|yolo|1|2|3]"
        }
        MessageId::CmdModelDescription => "Chuyển đổi hoặc xem mô hình AI hiện tại",
        MessageId::CmdModelsDescription => "Liệt kê các mô hình khả dụng từ API",
        MessageId::CmdNetworkDescription => "Quản lý các quy tắc cho phép và từ chối mạng",
        MessageId::CmdNoteDescription => {
            "Thêm, liệt kê, sửa hoặc xóa ghi chú trong không gian làm việc"
        }
        MessageId::CmdThemeDescription => "Chuyển đổi giao diện hoặc mở bảng chọn giao diện",
        MessageId::CmdProviderDescription => {
            "Chuyển đổi hoặc xem backend LLM đang hoạt động (deepseek | nvidia-nim | ollama)"
        }
        MessageId::CmdQueueDescription => "Xem hoặc chỉnh sửa các tin nhắn đang chờ xử lý",
        MessageId::CmdQueueUsage => "Cách dùng: /queue [list|edit <n>|drop <n>|clear]",
        MessageId::CmdQueueDraftHeader => "Đang chỉnh sửa tin nhắn đang chờ:",
        MessageId::CmdQueueNoMessages => "Không có tin nhắn đang chờ",
        MessageId::CmdQueueListHeader => "Tin nhắn đang chờ ({count}):",
        MessageId::CmdQueueTip => "Mẹo: /queue edit <n> để sửa, /queue drop <n> để xóa",
        MessageId::CmdQueueAlreadyEditing => {
            "Đã đang chỉnh sửa một tin nhắn đang chờ. Hãy gửi nó hoặc dùng /queue clear để hủy."
        }
        MessageId::CmdQueueNotFound => "Không tìm thấy tin nhắn đang chờ",
        MessageId::CmdQueueEditingStatus => "Đang chỉnh sửa tin nhắn đang chờ {index}",
        MessageId::CmdQueueEditingMessage => {
            "Đang chỉnh sửa tin nhắn đang chờ {index} (nhấn Enter để xếp lại hàng/gửi)"
        }
        MessageId::CmdQueueDropped => "Đã xóa tin nhắn đang chờ {index}",
        MessageId::CmdQueueAlreadyEmpty => "Hàng đợi đã trống",
        MessageId::CmdQueueCleared => "Đã xóa hàng đợi",
        MessageId::CmdQueueMissingIndex => {
            "Thiếu chỉ mục. Cách dùng: /queue edit <n> hoặc /queue drop <n>"
        }
        MessageId::CmdQueueIndexPositive => "Chỉ mục phải là số dương",
        MessageId::CmdQueueIndexMin => "Chỉ mục phải >= 1",
        MessageId::CmdRelayDescription => "Tạo một phiên tiếp sức cho một luồng mới",
        MessageId::CmdRenameDescription => "Đổi tên phiên làm việc hiện tại",
        MessageId::CmdRestoreDescription => {
            "Khôi phục không gian làm việc về bản chụp trước/sau lượt. Nếu không có đối số, hiển thị các bản chụp gần đây."
        }
        MessageId::CmdRetryDescription => "Thử lại yêu cầu gần nhất",
        MessageId::CmdReviewDescription => {
            "Chạy một quy trình xem xét mã nguồn có cấu trúc trên tệp, diff hoặc PR"
        }
        MessageId::CmdRlmDescription => {
            "Mở một ngữ cảnh RLM liên tục: /rlm [0-3] <tệp_hoặc_văn_bản>"
        }
        MessageId::CmdSaveDescription => "Lưu phiên làm việc vào tệp",
        MessageId::CmdForkDescription => {
            "Rẽ nhánh (fork) cuộc hội thoại hiện tại thành một phiên song song"
        }
        MessageId::CmdNewDescription => "Bắt đầu một phiên lưu mới",
        MessageId::CmdSessionsDescription => "Mở bảng chọn lịch sử phiên làm việc",
        MessageId::CmdSettingsDescription => "Hiển thị các cài đặt liên tục",
        MessageId::CmdSidebarDescription => "Toggle or focus the right sidebar",
        MessageId::CmdSkillDescription => {
            "Kích hoạt một kỹ năng, hoặc cài đặt/cập nhật/gỡ bỏ/tin cậy một kỹ năng cộng đồng"
        }
        MessageId::CmdSkillsDescription => {
            "Liệt kê các kỹ năng cục bộ (lọc bằng `/skills <tiền_tố>`; --remote để duyệt kho lưu trữ được kiểm duyệt)"
        }
        MessageId::CmdSlopDescription => "Kiểm tra hoặc xuất SlopLedger",
        MessageId::CmdStashDescription => {
            "Tạm cất hoặc khôi phục bản nháp (Ctrl+S để cất, /stash list/pop để xem/lấy ra)"
        }
        MessageId::CmdStatusDescription => "Hiển thị trạng thái thời gian chạy của phiên",
        MessageId::CmdStatuslineDescription => {
            "Cấu hình các mục hiển thị ở thanh trạng thái dưới cùng"
        }
        MessageId::CmdSubagentsDescription => "Liệt kê trạng thái của các sub-agent",
        MessageId::CmdSwarmDescription => {
            "Khởi chạy chế độ đa agent (sequential | mixture | distill | deliberate)"
        }
        MessageId::CmdSystemDescription => "Hiển thị prompt hệ thống hiện tại",
        MessageId::CmdTaskDescription => "Quản lý các nhiệm vụ chạy ngầm",
        MessageId::CmdTokensDescription => "Hiển thị lượng token đã sử dụng cho phiên",
        MessageId::CmdTranslateDescription => {
            "Bật/Tắt chế độ dịch đầu ra sang ngôn ngữ hệ thống hiện tại"
        }
        MessageId::CmdTranslateOff => {
            "Đã tắt chế độ dịch đầu ra (hiển thị câu trả lời gốc của mô hình)"
        }
        MessageId::CmdTranslateOn => {
            "Đã bật chế độ dịch đầu ra: câu trả lời của mô hình sẽ được hiển thị bằng tiếng Việt"
        }
        MessageId::TranslationInProgress => "Đang dịch câu trả lời của trợ lý...",
        MessageId::TranslationComplete => "Đã dịch xong",
        MessageId::TranslationFailed => "Dịch thất bại",
        MessageId::CmdTrustDescription => {
            "Quản lý quyền tin cậy không gian làm việc và danh sách trắng theo đường dẫn (`/trust add <path>`, `/trust list`, `/trust on|off`)"
        }
        MessageId::CmdWorkspaceDescription => {
            "Hiển thị hoặc chuyển đổi không gian làm việc hiện tại"
        }
        MessageId::CmdUndoDescription => "Xóa cặp tin nhắn gần nhất",
        MessageId::CmdVerboseDescription => {
            "Bật/Tắt chế độ hiển thị đầy đủ quá trình suy nghĩ trực tiếp"
        }
        MessageId::CmdCacheAdvice => {
            "Tỷ lệ hit/miss trên ~70% sau lượt thứ ba cho thấy tiền tố bộ nhớ đệm ổn định; \nthấp hơn mức đó trong các phiên dài cho thấy có sự biến động tiền tố cần kiểm tra (#263)."
        }
        MessageId::CmdCacheFootnote => {
            "* miss được suy ra từ đầu vào − hit khi nhà cung cấp không báo cáo rõ ràng.\n"
        }
        MessageId::CmdCacheHeader => {
            "Thông tin cache — {count} lượt gần nhất trong tổng số {total} lượt (mô hình: {model})\n"
        }
        MessageId::CmdCacheNoData => {
            "Lịch sử bộ nhớ đệm: chưa có lượt nào được ghi nhận.\n\n\
             DeepSeek cung cấp `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` \
             trên mỗi lượt API mà mô hình hỗ trợ (dòng V4). Hãy chạy một lượt \
             và thử lại lệnh /cache."
        }
        MessageId::CmdCacheTotals => {
            "Σ vào: {sum_in}   Σ hit: {sum_hit}   Σ miss: {sum_miss}   tỷ lệ hit trung bình: {avg}\n"
        }
        MessageId::CmdCostReport => {
            "Chi Phí Phiên Làm Việc:\n\
             ─────────────────────────────\n\
             Tổng chi tiêu ước tính: {cost}\n\n\
             Các ước tính chi phí mang tính xấp xỉ và sử dụng dữ liệu viễn trắc từ nhà cung cấp nếu có.\n\n\
             Bảng Giá API DeepSeek:\n\
             ─────────────────────────────\n\
             Thông tin chi tiết về giá chưa được cấu hình trong CLI này."
        }
        MessageId::CmdTokensCacheBoth => "{hit} hit / {miss} miss",
        MessageId::CmdTokensCacheHitOnly => "{hit} hit / không báo cáo miss",
        MessageId::CmdTokensCacheMissOnly => "không báo cáo hit / {miss} miss",
        MessageId::CmdTokensContextUnknownWindow => "~{estimated} / không rõ cửa sổ ngữ cảnh",
        MessageId::CmdTokensContextWithWindow => "~{used} / {window} ({percent}%)",
        MessageId::FooterAgentSingular => "1 tác nhân",
        MessageId::FooterAgentsPlural => "{count} tác nhân",
        MessageId::FooterPressCtrlCAgain => "Nhấn Ctrl+C một lần nữa để thoát",
        MessageId::FooterWorking => "đang xử lý",
        MessageId::FooterBalancePrefix => "số dư",
        MessageId::HelpSectionActions => "Hành động",
        MessageId::HelpSectionClipboard => "Bộ nhớ tạm",
        MessageId::HelpSectionEditing => "Chỉnh sửa đầu vào",
        MessageId::HelpSectionHelp => "Trợ giúp",
        MessageId::HelpSectionModes => "Chế độ",
        MessageId::HelpSectionNavigation => "Điều hướng",
        MessageId::HelpSectionSessions => "Phiên",
        MessageId::CmdTokensNotReported => "không được báo cáo",
        MessageId::CmdTokensReport => {
            "Lượng Token Sử Dụng:\n\
             ─────────────────────────────\n\
             Ngữ cảnh hoạt động:        {active}\n\
             Đầu vào API gần nhất:       {input} (viễn trắc theo lượt; có thể đếm lặp lại tiền tố qua các vòng công cụ)\n\
             Đầu ra API gần nhất:       {output}\n\
             Hit/miss bộ nhớ đệm:        {cache} (chỉ dành cho viễn trắc/chi phí)\n\
             Token tích lũy:             {total} (dữ liệu viễn trắc sử dụng của phiên)\n\
             Chi phí phiên xấp xỉ:       {cost}\n\
             Tin nhắn API:               {api_messages}\n\
             Tin nhắn trò chuyện:        {chat_messages}\n\
             Mô hình:                    {model}"
        }
        MessageId::KbScrollTranscript => {
            "Cuộn bản ghi trò chuyện, điều hướng lịch sử nhập hoặc chọn tệp đính kèm"
        }
        MessageId::KbNavigateHistory => "Điều hướng lịch sử nhập",
        MessageId::KbBrowseHistory => "Duyệt lịch sử cuộc trò chuyện",
        MessageId::KbScrollTranscriptAlt => "Cuộn bản ghi trò chuyện",
        MessageId::KbScrollPage => "Cuộn bản ghi trò chuyện theo trang",
        MessageId::KbJumpTopBottom => "Nhảy lên đầu / xuống cuối bản ghi trò chuyện",
        MessageId::KbJumpTopBottomEmpty => "Nhảy lên đầu / xuống cuối (khi khung nhập trống)",
        MessageId::KbJumpToolBlocks => "Nhảy giữa các khối đầu ra của công cụ",
        MessageId::KbMoveCursor => "Di chuyển con trỏ trong khung soạn thảo",
        MessageId::KbJumpLineStartEnd => "Nhảy về đầu / cuối dòng",
        MessageId::KbDeleteChar => "Xóa ký tự trước / sau con trỏ, hoặc xóa tệp đính kèm đã chọn",
        MessageId::KbClearDraft => "Xóa bản nháp hiện tại",
        MessageId::KbStashDraft => "Tạm cất bản nháp hiện tại (dùng `/stash pop` để khôi phục)",
        MessageId::KbSearchHistory => "Tìm kiếm lịch sử câu lệnh và khôi phục các bản nháp cục bộ",
        MessageId::KbInsertNewline => "Chèn một dòng mới trong khung soạn thảo",
        MessageId::KbSendDraft => "Gửi bản nháp hiện tại",
        MessageId::KbCloseMenu => "Đóng menu, hủy yêu cầu, hủy bản nháp hoặc xóa sạch đầu vào",
        MessageId::KbCancelOrExit => "Hủy yêu cầu, hoặc thoát khi rảnh",
        MessageId::KbShellControls => "Mở các điều khiển shell cho một lệnh đang chạy ở tiền cảnh",
        MessageId::KbExitEmpty => "Thoát khi khung nhập trống",
        MessageId::KbCommandPalette => "Mở bảng lệnh (command palette)",
        MessageId::KbFuzzyFilePicker => {
            "Mở trình tìm file nhanh (fuzzy) (chèn @path khi nhấn Enter)"
        }
        MessageId::KbCompactInspector => "Mở trình kiểm tra ngữ cảnh phiên thu gọn",
        MessageId::KbLastMessagePager => {
            "Mở trang xem cho tin nhắn cuối cùng (khi khung nhập trống)"
        }
        MessageId::KbSelectedDetails => {
            "Mở chi tiết cho công cụ hoặc tin nhắn được chọn (khi khung nhập trống)"
        }
        MessageId::KbToolDetailsPager => "Mở trang xem chi tiết công cụ",
        MessageId::KbThinkingPager => "Mở Chi Tiết Hoạt Động (Activity Detail)",
        MessageId::KbLiveTranscript => "Mở lớp phủ bản ghi trực tiếp (tự động cuộn theo đuôi)",
        MessageId::KbBacktrackMessage => {
            "Quay lại tin nhắn trước đó của người dùng (nhấn Trái/Phải để chuyển bước, Enter để lùi lại)"
        }
        MessageId::KbCompleteCycleModes => {
            "Hoàn thành /command, xếp hàng theo dõi lượt đang chạy, chuyển đổi chế độ; Shift+Tab để chuyển đổi mức độ suy luận"
        }
        MessageId::KbJumpPlanAgentYolo => "Nhảy trực tiếp sang chế độ Plan / Agent / YOLO",
        MessageId::KbAltJumpPlanAgentYolo => {
            "Phím tắt thay thế để nhảy sang chế độ Plan / Agent / YOLO"
        }
        MessageId::KbFocusSidebar => {
            "Focus vào thanh bên Work / Tasks / Agents / Context / Auto; Ctrl+Alt+0 để ẩn"
        }
        MessageId::KbTogglePlanAgent => "Chuyển đổi giữa chế độ Plan và Agent",
        MessageId::KbSessionPicker => "Mở bảng chọn phiên làm việc",
        MessageId::KbPasteAttach => "Dán văn bản hoặc đính kèm hình ảnh từ bộ nhớ tạm",
        MessageId::KbCopySelection => "Sao chép vùng chọn hiện tại (Cmd+C trên macOS)",
        MessageId::KbContextMenu => {
            "Mở các hành động ngữ cảnh cho dán, vùng chọn, chi tiết tin nhắn, ngữ cảnh và trợ giúp"
        }
        MessageId::KbAttachPath => "Thêm một tệp văn bản cục bộ hoặc thư mục vào ngữ cảnh",
        MessageId::KbHelpOverlay => "Mở lớp phủ trợ giúp này (khi khung nhập trống)",
        MessageId::KbToggleHelp => "Bật/Tắt lớp phủ trợ giúp",
        MessageId::KbToggleHelpSlash => "Bật/Tắt lớp phủ trợ giúp",
        MessageId::HelpUsageLabel => "Sử dụng:",
        MessageId::HelpAliasesLabel => "Bí danh:",
        MessageId::SettingsTitle => "Cài đặt:",
        MessageId::SettingsConfigFile => "Tệp cấu hình:",
        MessageId::ClearConversation => "Đã xóa cuộc trò chuyện",
        MessageId::ClearConversationBusy => {
            "Đã xóa cuộc trò chuyện (trạng thái plan đang bận; chạy lại /clear nếu cần)"
        }
        MessageId::ModelChanged => "Đã thay đổi mô hình: {old} \u{2192} {new}",
        MessageId::LinksTitle => "Liên kết DeepSeek:",
        MessageId::LinksDashboard => "Bảng điều khiển:",
        MessageId::LinksDocs => "Tài liệu:",
        MessageId::LinksTip => "Mẹo: Mã khóa API có sẵn trong bảng điều khiển console.",
        MessageId::SubagentsFetching => "Đang lấy trạng thái của các sub-agent...",
        MessageId::HelpUnknownCommand => "Lệnh không xác định: {topic}",
        MessageId::HomeDashboardTitle => "Bảng Điều Khiển Trang Chủ codewhale",
        MessageId::HomeModel => "Mô hình:",
        MessageId::HomeMode => "Chế độ:",
        MessageId::HomeWorkspace => "Không gian làm việc:",
        MessageId::HomeHistory => "Lịch sử:",
        MessageId::HomeTokens => "Token:",
        MessageId::HomeQueued => "Trong hàng đợi:",
        MessageId::HomeSubagents => "Sub-agent:",
        MessageId::HomeSkill => "Kỹ năng:",
        MessageId::HomeQuickActions => "Hành động nhanh",
        MessageId::HomeQuickLinks => "/links      - Các liên kết đến Dashboard & API",
        MessageId::HomeQuickSkills => "/skills     - Liệt kê các kỹ năng khả dụng",
        MessageId::HomeQuickConfig => "/config     - Mở trình chỉnh sửa cấu hình tương tác",
        MessageId::HomeQuickSettings => "/settings    - Hiển thị các cài đặt liên tục",
        MessageId::HomeQuickModel => "/model       - Xem hoặc chuyển đổi mô hình",
        MessageId::HomeQuickSubagents => "/subagents   - Liệt kê trạng thái sub-agent",
        MessageId::HomeQuickTaskList => "/task list   - Hiển thị hàng đợi nhiệm vụ ngầm",
        MessageId::HomeQuickHelp => "/help        - Hiển thị trợ giúp",
        MessageId::HomeModeTips => "Mẹo về Chế độ",
        MessageId::HomeAgentModeTip => "Chế độ Agent - Sử dụng công cụ cho các nhiệm vụ tự chủ",
        MessageId::HomeAgentModeReviewTip => {
            "  Sử dụng Ctrl+X để xem xét ở chế độ Plan trước khi thực thi"
        }
        MessageId::HomeAgentModeYoloTip => "  Nhập /mode yolo để bật toàn quyền truy cập công cụ",
        MessageId::HomeYoloModeTip => {
            "Chế độ YOLO - Toàn quyền truy cập công cụ, không cần phê duyệt"
        }
        MessageId::HomeYoloModeCaution => "  Hãy cẩn thận với các thao tác mang tính phá hủy!",
        MessageId::HomePlanModeTip => "Chế độ Plan - Thiết kế trước khi triển khai",
        MessageId::HomePlanModeChecklistTip => {
            "  Sử dụng /mode plan để tạo danh sách kiểm tra có cấu trúc"
        }
        MessageId::HomeGoalModeTip => {
            "Theo dõi mục tiêu - Dùng /goal <mục_tiêu> để đặt mục tiêu làm việc"
        }
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "Chọn ngôn ngữ của bạn",
        MessageId::OnboardLanguageBlurb => {
            "Chọn ngôn ngữ hiển thị. Bạn có thể thay đổi bất kỳ lúc nào bằng lệnh `/settings set locale <tag>`."
        }
        MessageId::OnboardLanguageFooter => {
            "Nhấn phím từ 1-7 để chọn, hoặc Enter để giữ cài đặt hiện tại"
        }
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "Kết nối khóa API DeepSeek của bạn",
        MessageId::OnboardApiKeyStep1 => {
            "Bước 1. Truy cập https://platform.deepseek.com/api_keys và tạo một khóa."
        }
        MessageId::OnboardApiKeyStep2 => "Bước 2. Dán khóa vào bên dưới và nhấn Enter.",
        MessageId::OnboardApiKeySavedHint => {
            "Được lưu vào ~/.codewhale/config.toml để có thể hoạt động từ mọi thư mục."
        }
        MessageId::OnboardApiKeyFormatHint => {
            "Dán chính xác toàn bộ khóa (không chứa khoảng trắng hoặc xuống dòng)."
        }
        MessageId::OnboardApiKeyPlaceholder => "(dán khóa vào đây)",
        MessageId::OnboardApiKeyLabel => "Khóa: ",
        MessageId::OnboardApiKeyFooter => "Nhấn Enter để lưu, Esc để quay lại.",
        // Onboarding — workspace trust.
        MessageId::OnboardTrustTitle => "Tin cậy không gian làm việc",
        MessageId::OnboardTrustQuestion => "Bạn có tin cậy nội dung của thư mục này không?",
        MessageId::OnboardTrustLocationPrefix => "Bạn đang ở ",
        MessageId::OnboardTrustRiskHint => {
            "Làm việc với các nội dung không tin cậy sẽ tăng nguy cơ bị tấn công prompt injection."
        }
        MessageId::OnboardTrustEffectHint => {
            "Tin cậy thư mục này sẽ lưu lại vào cấu hình toàn cục và bật chế độ không gian làm việc tin cậy."
        }
        MessageId::OnboardTrustFooterPrefix => "Nhấn ",
        MessageId::OnboardTrustFooterMiddle => " để tin cậy và tiếp tục, ",
        MessageId::OnboardTrustFooterSuffix => " để thoát",
        // Onboarding — final tips.
        MessageId::OnboardTipsTitle => "Bắt đầu đơn giản",
        MessageId::OnboardTipsLine1 => {
            "Viết nhiệm vụ bằng ngôn ngữ tự nhiên. Sử dụng /help hoặc Ctrl+K khi bạn muốn dùng lệnh."
        }
        MessageId::OnboardTipsLine2 => {
            "Khung nhập văn bản bên dưới hỗ trợ viết nhiều dòng: Enter để gửi, Alt+Enter hoặc Ctrl+J để xuống dòng."
        }
        MessageId::OnboardTipsLine3 => {
            "Chỉ chuyển đổi chế độ khi tính chất công việc thay đổi: Plan để lập kế hoạch trước khi làm, Agent để tự động thực hiện, YOLO khi bạn muốn tự động phê duyệt."
        }
        MessageId::OnboardTipsLine4 => {
            "Ctrl+R để khôi phục lại các phiên làm việc trước đó, và Esc để thoát khỏi bản nháp hoặc lớp phủ hiện tại."
        }
        MessageId::OnboardTipsFooterEnter => "Nhấn Enter",
        MessageId::OnboardTipsFooterAction => " để mở không gian làm việc",
        // Context menu.
        MessageId::CtxMenuTitle => " Nhấp chuột phải ",
        MessageId::CtxMenuCopySelection => "Sao chép vùng chọn",
        MessageId::CtxMenuCopySelectionDesc => "ghi văn bản transcript đã chọn",
        MessageId::CtxMenuOpenSelection => "Mở vùng chọn",
        MessageId::CtxMenuOpenSelectionDesc => "hiển thị văn bản đã chọn trong trình xem",
        MessageId::CtxMenuClearSelection => "Xóa vùng chọn",
        MessageId::CtxMenuOpenDetails => "Mở chi tiết",
        MessageId::CtxMenuCopyMessage => "Sao chép tin nhắn",
        MessageId::CtxMenuCopyMessageDesc => "ghi ô transcript đã bấm",
        MessageId::CtxMenuOpenInEditor => "Mở trong trình soạn thảo",
        MessageId::CtxMenuOpenInEditorDesc => "mở file:line trong $EDITOR",
        MessageId::CtxMenuShowCell => "Hiển thị ô",
        MessageId::CtxMenuShowCellDesc => "hiển thị lại ô transcript này",
        MessageId::CtxMenuHideCell => "Ẩn ô",
        MessageId::CtxMenuHideCellDesc => "thu gọn ô transcript này",
        MessageId::CtxMenuShowHidden => "Hiển thị mục ẩn",
        MessageId::CtxMenuShowHiddenDesc => "hiển thị lại tất cả ô đã thu gọn",
        MessageId::CtxMenuPaste => "Dán",
        MessageId::CtxMenuPasteDesc => "chèn clipboard vào khung nhập",
        MessageId::CtxMenuCmdPalette => "Bảng lệnh",
        MessageId::CtxMenuCmdPaletteDesc => "lệnh, kỹ năng và công cụ",
        MessageId::CtxMenuContextInspector => "Trình kiểm tra ngữ cảnh",
        MessageId::CtxMenuContextInspectorDesc => "ngữ cảnh đang hoạt động và gợi ý bộ nhớ đệm",
        MessageId::CtxMenuHelp => "Trợ giúp",
        MessageId::CtxMenuHelpDesc => "phím tắt và lệnh",
        MessageId::FanoutCounts => {
            "{done} hoàn thành · {running} đang chạy · {failed} thất bại · {pending} chờ"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "XEM XÉT",
        MessageId::ApprovalRiskDestructive => "NGUY HẠI",
        MessageId::ApprovalCategorySafe => "An toàn",
        MessageId::ApprovalCategoryFileWrite => "Ghi Tệp",
        MessageId::ApprovalCategoryShell => "Lệnh Shell",
        MessageId::ApprovalCategoryNetwork => "Mạng",
        MessageId::ApprovalCategoryMcpRead => "Đọc MCP",
        MessageId::ApprovalCategoryMcpAction => "Hành động MCP",
        MessageId::ApprovalCategoryUnknown => "Không xác định",
        MessageId::ApprovalFieldType => "Loại:",
        MessageId::ApprovalFieldAbout => "Mô tả:",
        MessageId::ApprovalFieldImpact => "Tác động:",
        MessageId::ApprovalFieldParams => "Tham số:",
        MessageId::ApprovalOptionApproveOnce => "Phê duyệt một lần",
        MessageId::ApprovalOptionApproveAlways => "Phê duyệt mọi lần cho loại này",
        MessageId::ApprovalOptionDeny => "Từ chối lần gọi này",
        MessageId::ApprovalOptionAbortTurn => "Hủy bỏ lượt",
        MessageId::ApprovalBlockTitle => "phê duyệt",
        MessageId::ApprovalControlsHint => "  ·  v: tham số  ·  Esc: hủy bỏ",
        MessageId::ApprovalChooseHint => "Chọn: ",
        MessageId::ApprovalChooseAction => "Enter để chọn, hoặc nhấn y/a/d trực tiếp",
        MessageId::ApprovalIntentLabel => "Ý định: ",
        MessageId::ApprovalMoreLines => "  … (+{count} dòng)",

        MessageId::CtxInspTitle => "Trình kiểm tra ngữ cảnh",
        MessageId::CtxInspSessionContext => "Ngữ cảnh phiên",
        MessageId::CtxInspSystemPrompt => "Cấu trúc lời nhắc hệ thống",
        MessageId::CtxInspReferences => "Tham chiếu",
        MessageId::CtxInspRecentTools => "Công cụ gần đây",
        MessageId::CtxInspModel => "Mô hình",
        MessageId::CtxInspWorkspace => "Không gian làm việc",
        MessageId::CtxInspSession => "Phiên",
        MessageId::CtxInspContext => "Ngữ cảnh",
        MessageId::CtxInspTranscript => "Bảng ghi",
        MessageId::CtxInspWorkspaceStatus => "Trạng thái không gian làm việc",
        MessageId::CtxInspNotSampledYet => "chưa lấy mẫu",
        MessageId::CtxInspOk => "ổn",
        MessageId::CtxInspHigh => "cao",
        MessageId::CtxInspCritical => "nghiêm trọng",
        MessageId::CtxInspIncluded => "đã bao gồm",
        MessageId::CtxInspAttached => "đã đính kèm",
        MessageId::CtxInspNotIncluded => "không bao gồm",
        MessageId::CtxInspOutputCaptured => "đã thu được đầu ra",
        MessageId::CtxInspNoOutputYet => "chưa có đầu ra",
        MessageId::CtxInspNoSystemPrompt => "Chưa có lời nhắc hệ thống.",
        MessageId::CtxInspNoReferences => "Chưa có tham chiếu tệp, thư mục hoặc phương tiện nào.",
        MessageId::CtxInspNoToolActivity => "Chưa có hoạt động công cụ nào.",
        MessageId::CtxInspAltVHint => "Mở thẻ phù hợp và nhấn Alt+V để biết chi tiết.",
        MessageId::CtxInspCells => "ô",
        MessageId::CtxInspApiMessages => "tin nhắn API",
        MessageId::CtxInspActive => "đang hoạt động",
        MessageId::CtxInspCell => "ô",
        MessageId::CtxInspMoreReferences => "các tham chiếu khác",
        MessageId::CtxInspStablePrefix => "Khối ổn định",
        MessageId::CtxInspVolatileWorkingSet => "Vùng làm việc thay đổi",
        MessageId::CtxInspFirstLine => "Dòng đầu",
        MessageId::CtxInspTotal => "Tổng",
        MessageId::CtxInspTextPromptLayers => "Lớp văn bản gợi ý",
        MessageId::CtxInspSingleTextBlob => "Văn bản khối đơn",
        MessageId::CtxInspBlocks => "khối",
        MessageId::CtxInspBlock => "khối",
        MessageId::CtxInspTokens => "token",
        MessageId::CtxInspLayers => "lớp",
        MessageId::CtxInspNone => "không",
        MessageId::CtxInspEmpty => "(trống)",
        MessageId::CtxInspCacheFriendly => "thân thiện với bộ nhớ đệm",
        MessageId::CtxInspChangesByTurn => "thay đổi theo phiên/lượt",
        MessageId::CtxInspStablePrefixOnly => "chỉ có tiền tố ổn định",
        MessageId::CtxInspCacheTip => {
            "Gợi ý: Các khối ổn định đủ điều kiện cho bộ nhớ đệm tiền tố DeepSeek V4. Thay đổi vùng làm việc chỉ phá vỡ bộ nhớ đệm ở phần cuối."
        }
        _ => english(id),
    })
}

fn traditional_chinese(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::CmdRelayDescription => "為新執行緒建立會話接力摘要",
        MessageId::CmdTranslateDescription => "切換輸出翻譯為目前系統語言的開關狀態",
        MessageId::CmdTranslateOff => "輸出翻譯已關閉（顯示原始模型輸出）",
        MessageId::CmdTranslateOn => "輸出翻譯已開啟：模型回覆將以繁體中文顯示",
        MessageId::TranslationInProgress => "正在翻譯助理輸出...",
        MessageId::TranslationComplete => "翻譯完成",
        MessageId::TranslationFailed => "翻譯失敗",
        MessageId::FooterBalancePrefix => "餘額",
        MessageId::FanoutCounts => {
            "{done} 已完成 · {running} 運行中 · {failed} 失敗 · {pending} 等待中"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "審查",
        MessageId::ApprovalRiskDestructive => "破壞性",
        MessageId::ApprovalCategorySafe => "安全",
        MessageId::ApprovalCategoryFileWrite => "檔案寫入",
        MessageId::ApprovalCategoryShell => "Shell 命令",
        MessageId::ApprovalCategoryNetwork => "網路",
        MessageId::ApprovalCategoryMcpRead => "MCP 讀取",
        MessageId::ApprovalCategoryMcpAction => "MCP 操作",
        MessageId::ApprovalCategoryUnknown => "未分類",
        MessageId::ApprovalFieldType => "類型：",
        MessageId::ApprovalFieldAbout => "說明：",
        MessageId::ApprovalFieldImpact => "影響：",
        MessageId::ApprovalFieldParams => "參數：",
        MessageId::ApprovalOptionApproveOnce => "僅批准一次",
        MessageId::ApprovalOptionApproveAlways => "本會話同類自動批准",
        MessageId::ApprovalOptionDeny => "拒絕本次調用",
        MessageId::ApprovalOptionAbortTurn => "終止本輪",
        MessageId::ApprovalBlockTitle => "審批",
        MessageId::ApprovalControlsHint => "  ·  v：完整參數  ·  Esc：終止",
        MessageId::ApprovalChooseHint => "選擇：",
        MessageId::ApprovalChooseAction => "Enter 執行選中項，或直接按 y/a/d",
        MessageId::ApprovalIntentLabel => "意圖：",
        MessageId::ApprovalMoreLines => "  … (還有 {count} 行)",

        MessageId::CtxInspTitle => "上下文檢查器",
        MessageId::CtxInspSessionContext => "會話上下文",
        MessageId::CtxInspSystemPrompt => "系統提示結構",
        MessageId::CtxInspReferences => "引用",
        MessageId::CtxInspRecentTools => "最近使用的工具",
        MessageId::CtxInspModel => "模型",
        MessageId::CtxInspWorkspace => "工作區",
        MessageId::CtxInspSession => "會話",
        MessageId::CtxInspContext => "上下文",
        MessageId::CtxInspTranscript => "記錄",
        MessageId::CtxInspWorkspaceStatus => "工作區狀態",
        MessageId::CtxInspNotSampledYet => "尚未取樣",
        MessageId::CtxInspOk => "正常",
        MessageId::CtxInspHigh => "較高",
        MessageId::CtxInspCritical => "嚴重",
        MessageId::CtxInspIncluded => "已包含",
        MessageId::CtxInspAttached => "已附加",
        MessageId::CtxInspNotIncluded => "未包含",
        MessageId::CtxInspOutputCaptured => "已捕獲輸出",
        MessageId::CtxInspNoOutputYet => "尚無輸出",
        MessageId::CtxInspNoSystemPrompt => "未設定系統提示。",
        MessageId::CtxInspNoReferences => "尚未記錄任何檔案、目錄或媒體引用。",
        MessageId::CtxInspNoToolActivity => "尚未記錄任何工具活動。",
        MessageId::CtxInspAltVHint => "開啟對應的卡片並按 Alt+V 檢視詳細資訊。",
        MessageId::CtxInspCells => "儲存格",
        MessageId::CtxInspApiMessages => "API 訊息",
        MessageId::CtxInspActive => "作用中",
        MessageId::CtxInspCell => "儲存格",
        MessageId::CtxInspMoreReferences => "其他引用",
        MessageId::CtxInspStablePrefix => "穩定前綴",
        MessageId::CtxInspVolatileWorkingSet => "易變工作集",
        MessageId::CtxInspFirstLine => "第一行",
        MessageId::CtxInspTotal => "總計",
        MessageId::CtxInspTextPromptLayers => "文字提示層",
        MessageId::CtxInspSingleTextBlob => "單一文字塊",
        MessageId::CtxInspBlocks => "個區塊",
        MessageId::CtxInspBlock => "個區塊",
        MessageId::CtxInspTokens => "個 token",
        MessageId::CtxInspLayers => "個層",
        MessageId::CtxInspNone => "無",
        MessageId::CtxInspEmpty => "(空)",
        MessageId::CtxInspCacheFriendly => "快取友好",
        MessageId::CtxInspChangesByTurn => "按會話/輪次變化",
        MessageId::CtxInspStablePrefixOnly => "僅穩定前綴",
        MessageId::CtxInspCacheTip => {
            "提示：穩定前綴區塊符合 DeepSeek V4 前綴快取條件。易變工作集的更改僅會破壞快取尾部。"
        }
        MessageId::StatusPickerTitle => " 狀態列 ",
        MessageId::StatusPickerInstruction => "選擇要在底部顯示的項目:",
        MessageId::StatusPickerActionToggle => "切換 ",
        MessageId::StatusPickerActionAll => "全部 ",
        MessageId::StatusPickerActionNone => "無 ",
        MessageId::StatusPickerActionSave => "儲存 ",
        MessageId::StatusPickerActionCancel => "取消 ",
        other => chinese_simplified(other)?,
    })
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
        MessageId::StatusPickerTitle => " ステータス行 ",
        MessageId::StatusPickerInstruction => "フッターに表示する項目を選択:",
        MessageId::StatusPickerActionToggle => "切替 ",
        MessageId::StatusPickerActionAll => "すべて ",
        MessageId::StatusPickerActionNone => "なし ",
        MessageId::StatusPickerActionSave => "保存 ",
        MessageId::StatusPickerActionCancel => "キャンセル ",
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
        MessageId::CmdChangeDescription => "最新の更新履歴を表示",
        MessageId::CmdChangeHeader => "最新の更新履歴",
        MessageId::CmdChangeTranslationQueued => {
            "英語のリリースノートを以下に表示します。次に翻訳を依頼します。プロバイダーを利用できない場合は、この英語版がフォールバックです。"
        }
        MessageId::CmdChangeTranslationUnavailable => {
            "英語のリリースノートを以下に表示します。現在のセッションに API キーがないかオフラインのため、翻訳は利用できません。"
        }
        MessageId::CmdChangePreviousVersion => {
            "前のバージョン: {version} — `/change {version}` で表示"
        }
        MessageId::CmdBalanceDescription => "アクティブなプロバイダーのアカウント残高を確認",
        MessageId::CmdClearDescription => "会話履歴をクリア",
        MessageId::CmdCompactDescription => "コンテキスト圧縮で容量を確保",
        MessageId::CmdPurgeDescription => {
            "エージェントに会話履歴を分析させ、不要なメッセージを削除・要約"
        }
        MessageId::CmdConfigDescription => "インタラクティブな設定エディタを開く",
        MessageId::CmdContextDescription => "コンパクトなセッションコンテキスト検査ツールを開く",
        MessageId::CmdCostDescription => "セッションのコスト内訳を表示",
        MessageId::CmdDiffDescription => "セッション開始以降のファイル変更を表示",
        MessageId::CmdEditDescription => "最後のメッセージを編集して再送信",
        MessageId::CmdExitDescription => "アプリを終了",
        MessageId::CmdExportDescription => "会話を Markdown にエクスポート",
        MessageId::CmdFeedbackDescription => "GitHub フィードバック URL を生成",
        MessageId::CmdHfDescription => "Hugging Face MCP の設定と概念を確認",
        MessageId::CmdHelpDescription => "ヘルプを表示",
        MessageId::CmdHomeDescription => "統計とクイックアクション付きのホームダッシュボードを表示",
        MessageId::CmdHooksDescription => {
            "設定済みのライフサイクルフックを一覧表示（読み取り専用）"
        }
        MessageId::CmdAgentDescription => {
            "永続サブエージェントセッションを開く: /agent [0-3] <task>"
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
        MessageId::CmdThemeDescription => {
            "テーマを切り替え（ダーク/ライト/グレースケール/システム）"
        }
        MessageId::CmdProviderDescription => {
            "現在の LLM バックエンドを切り替え・確認（deepseek | nvidia-nim | ollama）"
        }
        MessageId::CmdQueueDescription => "キューされたメッセージを確認・編集",
        MessageId::CmdQueueUsage => "使用方法: /queue [list|edit <n>|drop <n>|clear]",
        MessageId::CmdQueueDraftHeader => "キューされたメッセージを編集中:",
        MessageId::CmdQueueNoMessages => "キューされたメッセージはありません",
        MessageId::CmdQueueListHeader => "キューされたメッセージ ({count}):",
        MessageId::CmdQueueTip => "ヒント: /queue edit <n> で編集、/queue drop <n> で削除",
        MessageId::CmdQueueAlreadyEditing => {
            "すでにキューされたメッセージを編集中です。送信するか /queue clear で破棄してください。"
        }
        MessageId::CmdQueueNotFound => "キューされたメッセージが見つかりません",
        MessageId::CmdQueueEditingStatus => "キューされたメッセージ {index} を編集中",
        MessageId::CmdQueueEditingMessage => {
            "キューされたメッセージ {index} を編集中（Enter で再キュー/送信）"
        }
        MessageId::CmdQueueDropped => "キューされたメッセージ {index} を削除しました",
        MessageId::CmdQueueAlreadyEmpty => "キューはすでに空です",
        MessageId::CmdQueueCleared => "キューをクリアしました",
        MessageId::CmdQueueMissingIndex => {
            "インデックスが指定されていません。使用方法: /queue edit <n> または /queue drop <n>"
        }
        MessageId::CmdQueueIndexPositive => "インデックスは正の数値である必要があります",
        MessageId::CmdQueueIndexMin => "インデックスは 1 以上である必要があります",
        MessageId::CmdRelayDescription => "新しいスレッド用のセッションリレー（接力）を作成",
        MessageId::CmdRenameDescription => "現在のセッションの名前を変更",
        MessageId::CmdRestoreDescription => {
            "ワークスペースを以前のターン前/後スナップショットへロールバック。引数なしで最近のスナップショットを一覧表示。"
        }
        MessageId::CmdRetryDescription => "直前のリクエストを再試行",
        MessageId::CmdReviewDescription => "ファイル・diff・PR に対して構造化コードレビューを実行",
        MessageId::CmdRlmDescription => "永続 RLM コンテキストを開く: /rlm [0-3] <file_or_text>",
        MessageId::CmdSaveDescription => "セッションをファイルに保存",
        MessageId::CmdForkDescription => "現在の会話を兄弟セッションに fork",
        MessageId::CmdNewDescription => "新しい保存済みセッションを開始",
        MessageId::CmdSessionsDescription => "セッション履歴ピッカーを開く",
        MessageId::CmdSettingsDescription => "永続化された設定を表示",
        MessageId::CmdSidebarDescription => "Toggle or focus the right sidebar",
        MessageId::CmdSkillDescription => {
            "スキルを有効化、またはコミュニティスキルをインストール／更新／アンインストール／信頼"
        }
        MessageId::CmdSkillsDescription => {
            "ローカルスキルを一覧表示（`/skills <prefix>` で絞り込み、--remote で精選レジストリを参照）"
        }
        MessageId::CmdSlopDescription => "Inspect or export the SlopLedger",
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
        MessageId::CmdTranslateDescription => "出力翻訳を現在のシステム言語に切り替え",
        MessageId::CmdTranslateOff => "出力翻訳が無効になりました（元のモデル出力を表示）",
        MessageId::CmdTranslateOn => {
            "出力翻訳が有効になりました：モデル応答は現在のシステム言語で表示されます"
        }
        MessageId::TranslationInProgress => "アシスタント出力を翻訳中...",
        MessageId::TranslationComplete => "翻訳が完了しました",
        MessageId::TranslationFailed => "翻訳に失敗しました",
        MessageId::CmdTrustDescription => {
            "ワークスペースの信頼設定とパス別許可リストを管理（`/trust add <path>`、`/trust list`、`/trust on|off`）"
        }
        MessageId::CmdWorkspaceDescription => "現在のワークスペースを表示または切り替え",
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
        MessageId::FooterBalancePrefix => "残高",
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
        MessageId::KbBrowseHistory => "会話履歴を閲覧",
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
        MessageId::KbThinkingPager => "Activity Detail を開く",
        MessageId::KbLiveTranscript => "ライブ会話履歴オーバーレイを開く（自動追尾スクロール）",
        MessageId::KbBacktrackMessage => {
            "前のユーザーメッセージに戻る（左右でステップ、Enter で巻き戻し）"
        }
        MessageId::KbCompleteCycleModes => {
            "/command を補完、実行中ターンのフォローアップをキュー、モードを切り替え；Shift+Tab で推論強度を切り替え"
        }
        MessageId::KbJumpPlanAgentYolo => "Plan / Agent / YOLO モードに直接ジャンプ",
        MessageId::KbAltJumpPlanAgentYolo => "Plan / Agent / YOLO モードへの代替ジャンプ",
        MessageId::KbFocusSidebar => {
            "Work / Tasks / Agents / Context / Auto / Hidden サイドバーにフォーカス"
        }
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
        MessageId::HomeDashboardTitle => "codewhale ホームダッシュボード",
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
        MessageId::HomeGoalModeTip => "Goal 追跡 - /goal <目標> で持続的な目標を追跡",
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "言語を選択",
        MessageId::OnboardLanguageBlurb => {
            "UI 言語を選んでください。`/settings set locale <tag>` でいつでも変更できます。"
        }
        MessageId::OnboardLanguageFooter => "1〜7 で選択、または Enter で現在の設定を維持",
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "DeepSeek API キーを設定",
        MessageId::OnboardApiKeyStep1 => {
            "ステップ 1. https://platform.deepseek.com/api_keys を開いてキーを作成。"
        }
        MessageId::OnboardApiKeyStep2 => "ステップ 2. 下に貼り付けて Enter を押してください。",
        MessageId::OnboardApiKeySavedHint => {
            "~/.codewhale/config.toml に保存されるので、どのフォルダからでも有効になります。"
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
        // Context menu.
        MessageId::CtxMenuTitle => " 右クリック ",
        MessageId::CtxMenuCopySelection => "選択をコピー",
        MessageId::CtxMenuCopySelectionDesc => "選択したトランスクリプトのテキストを書き込む",
        MessageId::CtxMenuOpenSelection => "選択を開く",
        MessageId::CtxMenuOpenSelectionDesc => "選択したテキストをページャで表示",
        MessageId::CtxMenuClearSelection => "選択を解除",
        MessageId::CtxMenuOpenDetails => "詳細を開く",
        MessageId::CtxMenuCopyMessage => "メッセージをコピー",
        MessageId::CtxMenuCopyMessageDesc => "クリックしたトランスクリプトセルを書き込む",
        MessageId::CtxMenuOpenInEditor => "エディタで開く",
        MessageId::CtxMenuOpenInEditorDesc => "$EDITOR で file:line を開く",
        MessageId::CtxMenuShowCell => "セルを表示",
        MessageId::CtxMenuShowCellDesc => "このトランスクリプトセルを再表示",
        MessageId::CtxMenuHideCell => "セルを隠す",
        MessageId::CtxMenuHideCellDesc => "このトランスクリプトセルを折りたたむ",
        MessageId::CtxMenuShowHidden => "非表示を表示",
        MessageId::CtxMenuShowHiddenDesc => "すべての折りたたまれたセルを再表示",
        MessageId::CtxMenuPaste => "貼り付け",
        MessageId::CtxMenuPasteDesc => "クリップボードをコンポーザに挿入",
        MessageId::CtxMenuCmdPalette => "コマンドパレット",
        MessageId::CtxMenuCmdPaletteDesc => "コマンド、スキル、ツール",
        MessageId::CtxMenuContextInspector => "コンテキストインスペクタ",
        MessageId::CtxMenuContextInspectorDesc => "アクティブなコンテキストとキャッシュヒント",
        MessageId::CtxMenuHelp => "ヘルプ",
        MessageId::CtxMenuHelpDesc => "キー操作とコマンド",
        MessageId::FanoutCounts => {
            "{done} 完了 · {running} 実行中 · {failed} 失敗 · {pending} 保留"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "確認",
        MessageId::ApprovalRiskDestructive => "破壊的操作",
        MessageId::ApprovalCategorySafe => "安全",
        MessageId::ApprovalCategoryFileWrite => "ファイル書き込み",
        MessageId::ApprovalCategoryShell => "シェルコマンド",
        MessageId::ApprovalCategoryNetwork => "ネットワーク",
        MessageId::ApprovalCategoryMcpRead => "MCP読み取り",
        MessageId::ApprovalCategoryMcpAction => "MCPアクション",
        MessageId::ApprovalCategoryUnknown => "未分類",
        MessageId::ApprovalFieldType => "種類：",
        MessageId::ApprovalFieldAbout => "詳細：",
        MessageId::ApprovalFieldImpact => "影響：",
        MessageId::ApprovalFieldParams => "パラメータ：",
        MessageId::ApprovalOptionApproveOnce => "1回だけ承認",
        MessageId::ApprovalOptionApproveAlways => "常に承認（この種類）",
        MessageId::ApprovalOptionDeny => "拒否",
        MessageId::ApprovalOptionAbortTurn => "中断",
        MessageId::ApprovalBlockTitle => "承認",
        MessageId::ApprovalControlsHint => "  ·  v: パラメータ表示  ·  Esc: 中止",
        MessageId::ApprovalChooseHint => "選択：",
        MessageId::ApprovalChooseAction => "Enterで選択、または y/a/d を直接入力",
        MessageId::ApprovalIntentLabel => "意図：",
        MessageId::ApprovalMoreLines => "  … (+{count} 行)",

        MessageId::CtxInspTitle => "コンテキストインスペクタ",
        MessageId::CtxInspSessionContext => "セッションコンテキスト",
        MessageId::CtxInspSystemPrompt => "システムプロンプト構造",
        MessageId::CtxInspReferences => "参照",
        MessageId::CtxInspRecentTools => "最近のツール",
        MessageId::CtxInspModel => "モデル",
        MessageId::CtxInspWorkspace => "ワークスペース",
        MessageId::CtxInspSession => "セッション",
        MessageId::CtxInspContext => "コンテキスト",
        MessageId::CtxInspTranscript => "トランスクリプト",
        MessageId::CtxInspWorkspaceStatus => "ワークスペース状態",
        MessageId::CtxInspNotSampledYet => "未サンプリング",
        MessageId::CtxInspOk => "良好",
        MessageId::CtxInspHigh => "高い",
        MessageId::CtxInspCritical => "深刻",
        MessageId::CtxInspIncluded => "含まれている",
        MessageId::CtxInspAttached => "添付済み",
        MessageId::CtxInspNotIncluded => "含まれていない",
        MessageId::CtxInspOutputCaptured => "出力取得済み",
        MessageId::CtxInspNoOutputYet => "未出力",
        MessageId::CtxInspNoSystemPrompt => "システムプロンプトが設定されていません。",
        MessageId::CtxInspNoReferences => {
            "ファイル、ディレクトリ、メディアの参照はまだ記録されていません。"
        }
        MessageId::CtxInspNoToolActivity => "ツールアクティビティはまだ記録されていません。",
        MessageId::CtxInspAltVHint => "該当するカードを開き、Alt+V を押すと詳細が表示されます。",
        MessageId::CtxInspCells => "セル",
        MessageId::CtxInspApiMessages => "API メッセージ",
        MessageId::CtxInspActive => "アクティブ",
        MessageId::CtxInspCell => "セル",
        MessageId::CtxInspMoreReferences => "その他の参照",
        MessageId::CtxInspStablePrefix => "安定プレフィックス",
        MessageId::CtxInspVolatileWorkingSet => "揮発性ワーキングセット",
        MessageId::CtxInspFirstLine => "最初の行",
        MessageId::CtxInspTotal => "合計",
        MessageId::CtxInspTextPromptLayers => "テキストプロンプトレイヤー",
        MessageId::CtxInspSingleTextBlob => "単一テキストブロブ",
        MessageId::CtxInspBlocks => "ブロック",
        MessageId::CtxInspBlock => "ブロック",
        MessageId::CtxInspTokens => "トークン",
        MessageId::CtxInspLayers => "レイヤー",
        MessageId::CtxInspNone => "なし",
        MessageId::CtxInspEmpty => "(空)",
        MessageId::CtxInspCacheFriendly => "キャッシュフレンドリー",
        MessageId::CtxInspChangesByTurn => "セッション/ターンごとに変更",
        MessageId::CtxInspStablePrefixOnly => "安定プレフィックスのみ",
        MessageId::CtxInspCacheTip => {
            "ヒント：安定プレフィックスブロックはDeepSeek V4プレフィックスキャッシュの対象です。揮発性ワーキングセットの変更は末尾のキャッシュのみを破壊します。"
        }
        _ => english(id),
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
        MessageId::StatusPickerTitle => " 状态行 ",
        MessageId::StatusPickerInstruction => "选择要在底部显示的项目:",
        MessageId::StatusPickerActionToggle => "切换 ",
        MessageId::StatusPickerActionAll => "全部 ",
        MessageId::StatusPickerActionNone => "无 ",
        MessageId::StatusPickerActionSave => "保存 ",
        MessageId::StatusPickerActionCancel => "取消 ",
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
        MessageId::CmdChangeDescription => "显示最新的更新日志",
        MessageId::CmdChangeHeader => "最新更新日志",
        MessageId::CmdChangeTranslationQueued => {
            "下面显示英文发布说明。接下来会请求模型翻译；如果当前提供商不可用，这段英文内容就是备用结果。"
        }
        MessageId::CmdChangeTranslationUnavailable => {
            "下面显示英文发布说明。当前会话没有 API Key 或处于离线状态，无法翻译。"
        }
        MessageId::CmdChangePreviousVersion => {
            "上一个版本: {version} —— 输入 `/change {version}` 查看"
        }
        MessageId::CmdBalanceDescription => "查看当前提供商账户余额",
        MessageId::CmdClearDescription => "清除对话历史",
        MessageId::CmdCompactDescription => "触发上下文压缩以释放空间",
        MessageId::CmdPurgeDescription => "让 Agent 分析对话历史，精确保留有用信息并移除冗余内容",
        MessageId::CmdConfigDescription => "打开交互式配置编辑器",
        MessageId::CmdContextDescription => "打开紧凑会话上下文检查器",
        MessageId::CmdCostDescription => "显示本次会话的费用明细",
        MessageId::CmdDiffDescription => "显示会话开始以来的文件变更",
        MessageId::CmdEditDescription => "修改并重新提交最后一条消息",
        MessageId::CmdExitDescription => "退出应用",
        MessageId::CmdExportDescription => "将对话导出为 Markdown",
        MessageId::CmdFeedbackDescription => "生成 GitHub 反馈链接",
        MessageId::CmdHfDescription => "检查 Hugging Face MCP 设置和概念",
        MessageId::CmdHelpDescription => "显示帮助信息",
        MessageId::CmdHomeDescription => "显示主页面板，含统计与快捷操作",
        MessageId::CmdHooksDescription => "列出已配置的生命周期钩子（只读）",
        MessageId::CmdAgentDescription => "打开持久子代理会话：/agent [0-3] <task>",
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
        MessageId::CmdThemeDescription => "切换主题：深色、浅色、灰度或系统",
        MessageId::CmdProviderDescription => {
            "切换或查看当前 LLM 后端（deepseek | nvidia-nim | ollama）"
        }
        MessageId::CmdQueueDescription => "查看或编辑已排队的消息",
        MessageId::CmdQueueUsage => "用法: /queue [list|edit <n>|drop <n>|clear]",
        MessageId::CmdQueueDraftHeader => "正在编辑已排队的消息:",
        MessageId::CmdQueueNoMessages => "没有已排队的消息",
        MessageId::CmdQueueListHeader => "已排队的消息 ({count}):",
        MessageId::CmdQueueTip => "提示: /queue edit <n> 编辑, /queue drop <n> 删除",
        MessageId::CmdQueueAlreadyEditing => {
            "已在编辑一条已排队的消息。请先发送或使用 /queue clear 放弃。"
        }
        MessageId::CmdQueueNotFound => "未找到已排队的消息",
        MessageId::CmdQueueEditingStatus => "正在编辑已排队的消息 {index}",
        MessageId::CmdQueueEditingMessage => {
            "正在编辑已排队的消息 {index}（按 Enter 重新排队/发送）"
        }
        MessageId::CmdQueueDropped => "已删除已排队的消息 {index}",
        MessageId::CmdQueueAlreadyEmpty => "队列已空",
        MessageId::CmdQueueCleared => "队列已清空",
        MessageId::CmdQueueMissingIndex => "缺少索引。用法: /queue edit <n> 或 /queue drop <n>",
        MessageId::CmdQueueIndexPositive => "索引必须为正数",
        MessageId::CmdQueueIndexMin => "索引必须 >= 1",
        MessageId::CmdRelayDescription => "为新线程创建会话接力摘要",
        MessageId::CmdRenameDescription => "重命名当前会话",
        MessageId::CmdRestoreDescription => {
            "将工作区回滚到此前的轮次前/后快照。不带参数时列出最近的快照。"
        }
        MessageId::CmdRetryDescription => "重试上一次请求",
        MessageId::CmdReviewDescription => "对文件、diff 或 PR 进行结构化代码审查",
        MessageId::CmdRlmDescription => "打开持久 RLM 上下文：/rlm [0-3] <file_or_text>",
        MessageId::CmdSaveDescription => "将会话保存到文件",
        MessageId::CmdForkDescription => "将当前对话分叉为兄弟会话",
        MessageId::CmdNewDescription => "开始一个新的已保存会话",
        MessageId::CmdSessionsDescription => "打开会话历史选择器",
        MessageId::CmdSettingsDescription => "显示持久化设置",
        MessageId::CmdSkillDescription => "激活技能，或安装/更新/卸载/信任社区技能",
        MessageId::CmdSidebarDescription => "Toggle or focus the right sidebar",
        MessageId::CmdSkillsDescription => {
            "列出本地技能（用 `/skills <prefix>` 按名称前缀过滤，--remote 浏览精选注册表）"
        }
        MessageId::CmdSlopDescription => "Inspect or export the SlopLedger",
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
        MessageId::CmdTranslateDescription => "切换输出翻译为当前系统语言的开/关状态",
        MessageId::CmdTranslateOff => "输出翻译已关闭（显示原始模型输出）",
        MessageId::CmdTranslateOn => "输出翻译已开启：模型回复将以当前系统语言显示",
        MessageId::TranslationInProgress => "正在翻译助手输出...",
        MessageId::TranslationComplete => "翻译完成",
        MessageId::TranslationFailed => "翻译失败",
        MessageId::CmdTrustDescription => {
            "管理工作区信任与按路径的白名单（`/trust add <path>`、`/trust list`、`/trust on|off`）"
        }
        MessageId::CmdWorkspaceDescription => "显示或切换当前工作空间",
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
        MessageId::FooterBalancePrefix => "余额",
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
        MessageId::KbBrowseHistory => "浏览对话历史",
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
        MessageId::KbThinkingPager => "打开 Activity Detail",
        MessageId::KbLiveTranscript => "打开实时对话覆盖层（自动滚动尾随）",
        MessageId::KbBacktrackMessage => "回退到之前的用户消息（左右键步进，Enter 回退）",
        MessageId::KbCompleteCycleModes => {
            "补全 /command、排队运行轮次跟进、切换模式；Shift+Tab 切换推理强度"
        }
        MessageId::KbJumpPlanAgentYolo => "直接跳转到 Plan / Agent / YOLO 模式",
        MessageId::KbAltJumpPlanAgentYolo => "替代快捷键跳转到 Plan / Agent / YOLO 模式",
        MessageId::KbFocusSidebar => "聚焦 Work / 任务 / 代理 / Context / 自动 / 隐藏侧边栏",
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
        MessageId::HomeDashboardTitle => "codewhale 主面板",
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
        MessageId::HomeGoalModeTip => "Goal 跟踪 - 设置 /goal <目标> 以跟踪持久目标",
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "选择语言",
        MessageId::OnboardLanguageBlurb => {
            "选择界面语言。可随时使用 `/settings set locale <tag>` 修改。"
        }
        MessageId::OnboardLanguageFooter => "按 1-7 选择，或按 Enter 保留当前设置",
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "连接你的 DeepSeek API 密钥",
        MessageId::OnboardApiKeyStep1 => {
            "步骤 1.  打开 https://platform.deepseek.com/api_keys 创建一个密钥。"
        }
        MessageId::OnboardApiKeyStep2 => "步骤 2.  把密钥粘贴到下方并按 Enter。",
        MessageId::OnboardApiKeySavedHint => {
            "保存到 ~/.codewhale/config.toml，因此在任何目录下都生效。"
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
        // Context menu.
        MessageId::CtxMenuTitle => " 右键菜单 ",
        MessageId::CtxMenuCopySelection => "复制所选",
        MessageId::CtxMenuCopySelectionDesc => "将选中的记录区域文本写入剪贴板",
        MessageId::CtxMenuOpenSelection => "打开所选",
        MessageId::CtxMenuOpenSelectionDesc => "在翻阅器中查看选中文本",
        MessageId::CtxMenuClearSelection => "清除选择",
        MessageId::CtxMenuOpenDetails => "打开详情",
        MessageId::CtxMenuCopyMessage => "复制消息",
        MessageId::CtxMenuCopyMessageDesc => "将点击的记录条目写入剪贴板",
        MessageId::CtxMenuOpenInEditor => "在编辑器中打开",
        MessageId::CtxMenuOpenInEditorDesc => "在 $EDITOR 中打开 file:line",
        MessageId::CtxMenuShowCell => "显示条目",
        MessageId::CtxMenuShowCellDesc => "取消隐藏此记录条目",
        MessageId::CtxMenuHideCell => "隐藏条目",
        MessageId::CtxMenuHideCellDesc => "折叠此记录条目",
        MessageId::CtxMenuShowHidden => "显示已隐藏",
        MessageId::CtxMenuShowHiddenDesc => "取消隐藏所有已折叠条目",
        MessageId::CtxMenuPaste => "粘贴",
        MessageId::CtxMenuPasteDesc => "将剪贴板插入输入框",
        MessageId::CtxMenuCmdPalette => "命令面板",
        MessageId::CtxMenuCmdPaletteDesc => "命令、技能和工具",
        MessageId::CtxMenuContextInspector => "上下文检查器",
        MessageId::CtxMenuContextInspectorDesc => "活动上下文和缓存提示",
        MessageId::CtxMenuHelp => "帮助",
        MessageId::CtxMenuHelpDesc => "快捷键和命令",
        MessageId::FanoutCounts => {
            "{done} 已完成 · {running} 运行中 · {failed} 失败 · {pending} 等待中"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "审查",
        MessageId::ApprovalRiskDestructive => "破坏性",
        MessageId::ApprovalCategorySafe => "安全",
        MessageId::ApprovalCategoryFileWrite => "文件写入",
        MessageId::ApprovalCategoryShell => "Shell 命令",
        MessageId::ApprovalCategoryNetwork => "网络",
        MessageId::ApprovalCategoryMcpRead => "MCP 读取",
        MessageId::ApprovalCategoryMcpAction => "MCP 操作",
        MessageId::ApprovalCategoryUnknown => "未知",
        MessageId::ApprovalFieldType => "类型：",
        MessageId::ApprovalFieldAbout => "说明：",
        MessageId::ApprovalFieldImpact => "影响：",
        MessageId::ApprovalFieldParams => "参数：",
        MessageId::ApprovalOptionApproveOnce => "仅本次批准",
        MessageId::ApprovalOptionApproveAlways => "本会话同类自动批准",
        MessageId::ApprovalOptionDeny => "拒绝本次调用",
        MessageId::ApprovalOptionAbortTurn => "终止本轮",
        MessageId::ApprovalBlockTitle => "审批",
        MessageId::ApprovalControlsHint => "  ·  v：完整参数  ·  Esc：终止",
        MessageId::ApprovalChooseHint => "选择：",
        MessageId::ApprovalChooseAction => "Enter 执行选中项，或直接按 y/a/d",
        MessageId::ApprovalIntentLabel => "意图：",
        MessageId::ApprovalMoreLines => "  … (还有 {count} 行)",

        MessageId::CtxInspTitle => "上下文检查器",
        MessageId::CtxInspSessionContext => "会话上下文",
        MessageId::CtxInspSystemPrompt => "系统提示结构",
        MessageId::CtxInspReferences => "引用",
        MessageId::CtxInspRecentTools => "最近使用的工具",
        MessageId::CtxInspModel => "模型",
        MessageId::CtxInspWorkspace => "工作区",
        MessageId::CtxInspSession => "会话",
        MessageId::CtxInspContext => "上下文",
        MessageId::CtxInspTranscript => "记录",
        MessageId::CtxInspWorkspaceStatus => "工作区状态",
        MessageId::CtxInspNotSampledYet => "尚未采样",
        MessageId::CtxInspOk => "正常",
        MessageId::CtxInspHigh => "较高",
        MessageId::CtxInspCritical => "严重",
        MessageId::CtxInspIncluded => "已包含",
        MessageId::CtxInspAttached => "已附加",
        MessageId::CtxInspNotIncluded => "未包含",
        MessageId::CtxInspOutputCaptured => "已捕获输出",
        MessageId::CtxInspNoOutputYet => "尚无输出",
        MessageId::CtxInspNoSystemPrompt => "未设置系统提示。",
        MessageId::CtxInspNoReferences => "尚未记录任何文件、目录或媒体引用。",
        MessageId::CtxInspNoToolActivity => "尚未记录任何工具活动。",
        MessageId::CtxInspAltVHint => "打开对应的卡片并按 Alt+V 查看详细信息。",
        MessageId::CtxInspCells => "单元格",
        MessageId::CtxInspApiMessages => "API 消息",
        MessageId::CtxInspActive => "活动中",
        MessageId::CtxInspCell => "单元格",
        MessageId::CtxInspMoreReferences => "更多引用",
        MessageId::CtxInspStablePrefix => "稳定前缀",
        MessageId::CtxInspVolatileWorkingSet => "易变工作集",
        MessageId::CtxInspFirstLine => "第一行",
        MessageId::CtxInspTotal => "总计",
        MessageId::CtxInspTextPromptLayers => "文本提示层",
        MessageId::CtxInspSingleTextBlob => "单一文本块",
        MessageId::CtxInspBlocks => "个区块",
        MessageId::CtxInspBlock => "个区块",
        MessageId::CtxInspTokens => "个 token",
        MessageId::CtxInspLayers => "个层",
        MessageId::CtxInspNone => "无",
        MessageId::CtxInspEmpty => "(空)",
        MessageId::CtxInspCacheFriendly => "缓存友好",
        MessageId::CtxInspChangesByTurn => "按会话/轮次变化",
        MessageId::CtxInspStablePrefixOnly => "仅稳定前缀",
        MessageId::CtxInspCacheTip => {
            "提示：稳定前缀区块符合 DeepSeek V4 前缀缓存条件。易变工作集的更改仅会破坏缓存尾部。"
        }
        // Voice command
        MessageId::CmdVoiceDescription => "切换语音输入：录制语音并转录为文字",
        MessageId::CmdVoiceSendDescription => "切换自动发送：转录后若以「发送」结尾则自动提交",
        MessageId::CmdVoiceControlDescription => {
            "切换语音控制：AI 驱动的语音编辑/发送/切换代理操作"
        }
        MessageId::VoiceEnabled => "语音输入已开启，开始说话即可录制",
        MessageId::VoiceDisabled => "语音输入已关闭",
        MessageId::VoiceSendEnabled => "语音自动发送已开启",
        MessageId::VoiceSendDisabled => "语音自动发送已关闭",
        MessageId::VoiceControlEnabled => "语音控制已开启",
        MessageId::VoiceControlDisabled => "语音控制已关闭",
        MessageId::VoiceErrNoAuth => "语音：当前提供商未配置 API 密钥",
        MessageId::VoiceErrNoRecorder => "语音：未找到录音工具，请安装 sox、arecord 或 rec",
        MessageId::VoiceErrNetwork => "语音：转录网络请求失败",
        MessageId::VoiceErrEmptySend => "语音：没有可发送的内容",
        MessageId::VoiceErrUnknownAgent => "语音：未知代理",
        MessageId::VoiceRecording => "🎙 正在录音...请说话",
        MessageId::VoiceProcessing => "🎙 正在转录...",
        MessageId::VoiceTranscribed => "🎙 转录完成",
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
        MessageId::StatusPickerTitle => " Linha de status ",
        MessageId::StatusPickerInstruction => "Escolha os itens que deseja no rodapé:",
        MessageId::StatusPickerActionToggle => "alternar ",
        MessageId::StatusPickerActionAll => "todos ",
        MessageId::StatusPickerActionNone => "nenhum ",
        MessageId::StatusPickerActionSave => "salvar ",
        MessageId::StatusPickerActionCancel => "cancelar ",
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
        MessageId::CmdChangeDescription => "Mostrar a entrada mais recente do changelog",
        MessageId::CmdChangeHeader => "Changelog Mais Recente",
        MessageId::CmdChangeTranslationQueued => {
            "As notas de versao em ingles aparecem abaixo. Uma versao traduzida sera solicitada em seguida; se o provedor estiver indisponivel, este texto em ingles sera o fallback."
        }
        MessageId::CmdChangeTranslationUnavailable => {
            "As notas de versao em ingles aparecem abaixo. A traducao esta indisponivel porque a sessao atual nao tem chave de API ou esta offline."
        }
        MessageId::CmdChangePreviousVersion => {
            "Versão anterior: {version} — execute `/change {version}` para visualizar"
        }
        MessageId::CmdBalanceDescription => "Verificar o saldo da conta do provedor ativo",
        MessageId::CmdClearDescription => "Limpar o histórico da conversa",
        MessageId::CmdCompactDescription => "Compactar o contexto para liberar espaço",
        MessageId::CmdPurgeDescription => {
            "Deixe o agente podar cirurgicamente o histórico para liberar espaço de contexto"
        }
        MessageId::CmdConfigDescription => "Abrir o editor interativo de configuração",
        MessageId::CmdContextDescription => "Abrir o inspetor compacto de contexto da sessão",
        MessageId::CmdCostDescription => "Exibir o detalhamento de custo da sessão",
        MessageId::CmdDiffDescription => "Mostrar alterações em arquivos desde o início da sessão",
        MessageId::CmdEditDescription => "Revisar e reenviar a última mensagem",
        MessageId::CmdExitDescription => "Sair do aplicativo",
        MessageId::CmdExportDescription => "Exportar a conversa para markdown",
        MessageId::CmdFeedbackDescription => "Gerar uma URL de feedback no GitHub",
        MessageId::CmdHfDescription => "Inspecionar configuracao e conceitos do Hugging Face MCP",
        MessageId::CmdHelpDescription => "Exibir informações de ajuda",
        MessageId::CmdHomeDescription => "Exibir o painel inicial com estatísticas e ações rápidas",
        MessageId::CmdHooksDescription => {
            "Listar hooks de ciclo de vida configurados (somente leitura)"
        }
        MessageId::CmdAgentDescription => {
            "Abrir uma sessão persistente de sub-agente: /agent [0-3] <task>"
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
        MessageId::CmdThemeDescription => "Alternar tema: escuro, claro, tons de cinza ou sistema",
        MessageId::CmdProviderDescription => {
            "Trocar ou exibir o backend LLM ativo (deepseek | nvidia-nim | ollama)"
        }
        MessageId::CmdQueueDescription => "Ver ou editar mensagens enfileiradas",
        MessageId::CmdQueueUsage => "Uso: /queue [list|edit <n>|drop <n>|clear]",
        MessageId::CmdQueueDraftHeader => "Editando mensagem enfileirada:",
        MessageId::CmdQueueNoMessages => "Nenhuma mensagem enfileirada",
        MessageId::CmdQueueListHeader => "Mensagens enfileiradas ({count}):",
        MessageId::CmdQueueTip => "Dica: /queue edit <n> para editar, /queue drop <n> para remover",
        MessageId::CmdQueueAlreadyEditing => {
            "Já está editando uma mensagem enfileirada. Envie-a ou use /queue clear para descartar."
        }
        MessageId::CmdQueueNotFound => "Mensagem enfileirada não encontrada",
        MessageId::CmdQueueEditingStatus => "Editando mensagem enfileirada {index}",
        MessageId::CmdQueueEditingMessage => {
            "Editando mensagem enfileirada {index} (pressione Enter para re-enfileirar/enviar)"
        }
        MessageId::CmdQueueDropped => "Mensagem enfileirada {index} removida",
        MessageId::CmdQueueAlreadyEmpty => "Fila já está vazia",
        MessageId::CmdQueueCleared => "Fila limpa",
        MessageId::CmdQueueMissingIndex => {
            "Índice ausente. Uso: /queue edit <n> ou /queue drop <n>"
        }
        MessageId::CmdQueueIndexPositive => "O índice deve ser um número positivo",
        MessageId::CmdQueueIndexMin => "O índice deve ser >= 1",
        MessageId::CmdRelayDescription => "Criar um relay da sessão para um novo thread",
        MessageId::CmdRenameDescription => "Renomear a sessão atual",
        MessageId::CmdRestoreDescription => {
            "Reverter o workspace a um snapshot pré/pós-turno anterior. Sem argumento, lista os snapshots recentes."
        }
        MessageId::CmdRetryDescription => "Repetir a última requisição",
        MessageId::CmdReviewDescription => {
            "Executar uma revisão de código estruturada em um arquivo, diff ou PR"
        }
        MessageId::CmdRlmDescription => {
            "Abrir um contexto RLM persistente: /rlm [0-3] <file_or_text>"
        }
        MessageId::CmdSaveDescription => "Salvar a sessão em arquivo",
        MessageId::CmdForkDescription => "Bifurcar a conversa ativa para uma sessão irmã",
        MessageId::CmdNewDescription => "Iniciar uma nova sessão salva",
        MessageId::CmdSessionsDescription => "Abrir seletor de histórico de sessões",
        MessageId::CmdSettingsDescription => "Exibir as configurações persistidas",
        MessageId::CmdSidebarDescription => "Toggle or focus the right sidebar",
        MessageId::CmdSkillDescription => {
            "Ativar uma skill, ou instalar/atualizar/desinstalar/confiar em uma skill da comunidade"
        }
        MessageId::CmdSkillsDescription => {
            "Listar skills locais (filtre com `/skills <prefixo>`; --remote navega pelo registro curado)"
        }
        MessageId::CmdSlopDescription => "Inspect or export the SlopLedger",
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
        MessageId::CmdTranslateDescription => {
            "Alternar tradução de saída para o idioma atual do sistema"
        }
        MessageId::CmdTranslateOff => {
            "Tradução de saída desativada (saída original do modelo exibida)"
        }
        MessageId::CmdTranslateOn => {
            "Tradução de saída ativada: as respostas serão exibidas no idioma do sistema"
        }
        MessageId::TranslationInProgress => "Traduzindo saída do assistente...",
        MessageId::TranslationComplete => "Tradução concluída",
        MessageId::TranslationFailed => "Falha na tradução",
        MessageId::CmdTrustDescription => {
            "Gerenciar a confiança do workspace e a allowlist por caminho (`/trust add <path>`, `/trust list`, `/trust on|off`)"
        }
        MessageId::CmdWorkspaceDescription => "Mostrar ou trocar o workspace atual",
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
        MessageId::FooterBalancePrefix => "saldo",
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
        MessageId::KbBrowseHistory => "Navegar histórico da conversa",
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
        MessageId::KbThinkingPager => "Abrir Activity Detail",
        MessageId::KbLiveTranscript => "Abrir sobreposição de transcrição ao vivo (auto-scroll)",
        MessageId::KbBacktrackMessage => {
            "Retroceder para mensagem anterior do usuário (esquerda/direita, Enter para rebobinar)"
        }
        MessageId::KbCompleteCycleModes => {
            "Completar /command, enfileirar follow-up, ciclar modos; Shift+Tab cicla esforço de raciocínio"
        }
        MessageId::KbJumpPlanAgentYolo => "Pular direto para modo Plan / Agent / YOLO",
        MessageId::KbAltJumpPlanAgentYolo => "Salto alternativo para modo Plan / Agent / YOLO",
        MessageId::KbFocusSidebar => {
            "Focar barra lateral Work / Tasks / Agents / Context / Auto / Ocultar"
        }
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
        MessageId::HomeDashboardTitle => "Painel Inicial do codewhale",
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
        MessageId::HomeGoalModeTip => {
            "Rastreamento de Goal - Use /goal <objetivo> para rastrear um objetivo persistente"
        }
        // Onboarding — language picker.
        MessageId::OnboardLanguageTitle => "Escolha o idioma",
        MessageId::OnboardLanguageBlurb => {
            "Escolha o idioma da interface. Você pode mudá-lo a qualquer momento com `/settings set locale <tag>`."
        }
        MessageId::OnboardLanguageFooter => {
            "Pressione 1-7 para escolher, ou Enter para manter a configuração atual"
        }
        // Onboarding — API key entry.
        MessageId::OnboardApiKeyTitle => "Conecte sua chave de API DeepSeek",
        MessageId::OnboardApiKeyStep1 => {
            "Passo 1.  Abra https://platform.deepseek.com/api_keys e crie uma chave."
        }
        MessageId::OnboardApiKeyStep2 => "Passo 2.  Cole abaixo e pressione Enter.",
        MessageId::OnboardApiKeySavedHint => {
            "Salvo em ~/.codewhale/config.toml para funcionar em qualquer pasta."
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
        // Context menu.
        MessageId::CtxMenuTitle => " Clique direito ",
        MessageId::CtxMenuCopySelection => "Copiar seleção",
        MessageId::CtxMenuCopySelectionDesc => "copiar texto selecionado da transcrição",
        MessageId::CtxMenuOpenSelection => "Abrir seleção",
        MessageId::CtxMenuOpenSelectionDesc => "mostrar texto selecionado no visualizador",
        MessageId::CtxMenuClearSelection => "Limpar seleção",
        MessageId::CtxMenuOpenDetails => "Abrir detalhes",
        MessageId::CtxMenuCopyMessage => "Copiar mensagem",
        MessageId::CtxMenuCopyMessageDesc => "copiar célula da transcrição clicada",
        MessageId::CtxMenuOpenInEditor => "Abrir no editor",
        MessageId::CtxMenuOpenInEditorDesc => "abrir file:line no $EDITOR",
        MessageId::CtxMenuShowCell => "Mostrar célula",
        MessageId::CtxMenuShowCellDesc => "reexibir esta célula da transcrição",
        MessageId::CtxMenuHideCell => "Ocultar célula",
        MessageId::CtxMenuHideCellDesc => "recolher esta célula da transcrição",
        MessageId::CtxMenuShowHidden => "Mostrar ocultas",
        MessageId::CtxMenuShowHiddenDesc => "reexibir todas as células recolhidas",
        MessageId::CtxMenuPaste => "Colar",
        MessageId::CtxMenuPasteDesc => "inserir área de transferência no compositor",
        MessageId::CtxMenuCmdPalette => "Paleta de comandos",
        MessageId::CtxMenuCmdPaletteDesc => "comandos, habilidades e ferramentas",
        MessageId::CtxMenuContextInspector => "Inspetor de contexto",
        MessageId::CtxMenuContextInspectorDesc => "contexto ativo e dicas de cache",
        MessageId::CtxMenuHelp => "Ajuda",
        MessageId::CtxMenuHelpDesc => "atalhos de teclado e comandos",
        MessageId::FanoutCounts => {
            "{done} concluído · {running} em execução · {failed} falhou · {pending} pendente"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "REVISÃO",
        MessageId::ApprovalRiskDestructive => "DESTRUTIVO",
        MessageId::ApprovalCategorySafe => "Seguro",
        MessageId::ApprovalCategoryFileWrite => "Escrita de Arquivo",
        MessageId::ApprovalCategoryShell => "Comando Shell",
        MessageId::ApprovalCategoryNetwork => "Rede",
        MessageId::ApprovalCategoryMcpRead => "Leitura MCP",
        MessageId::ApprovalCategoryMcpAction => "Ação MCP",
        MessageId::ApprovalCategoryUnknown => "Desconhecido",
        MessageId::ApprovalFieldType => "Tipo:",
        MessageId::ApprovalFieldAbout => "Sobre:",
        MessageId::ApprovalFieldImpact => "Impacto:",
        MessageId::ApprovalFieldParams => "Parâmetros:",
        MessageId::ApprovalOptionApproveOnce => "Aprovar uma vez",
        MessageId::ApprovalOptionApproveAlways => "Aprovar sempre para este tipo",
        MessageId::ApprovalOptionDeny => "Negar esta chamada",
        MessageId::ApprovalOptionAbortTurn => "Abortar turno",
        MessageId::ApprovalBlockTitle => "aprovação",
        MessageId::ApprovalControlsHint => "  ·  v: parâmetros  ·  Esc: abortar",
        MessageId::ApprovalChooseHint => "Escolha: ",
        MessageId::ApprovalChooseAction => "Enter para selecionar, ou pressione y/a/d diretamente",
        MessageId::ApprovalIntentLabel => "Intenção: ",
        MessageId::ApprovalMoreLines => "  … (+{count} linhas)",

        MessageId::CtxInspTitle => "Inspetor de contexto",
        MessageId::CtxInspSessionContext => "Contexto da sessão",
        MessageId::CtxInspSystemPrompt => "Estrutura do prompt do sistema",
        MessageId::CtxInspReferences => "Referências",
        MessageId::CtxInspRecentTools => "Ferramentas recentes",
        MessageId::CtxInspModel => "Modelo",
        MessageId::CtxInspWorkspace => "Espaço de trabalho",
        MessageId::CtxInspSession => "Sessão",
        MessageId::CtxInspContext => "Contexto",
        MessageId::CtxInspTranscript => "Transcrição",
        MessageId::CtxInspWorkspaceStatus => "Status do espaço de trabalho",
        MessageId::CtxInspNotSampledYet => "ainda não amostrado",
        MessageId::CtxInspOk => "ok",
        MessageId::CtxInspHigh => "alto",
        MessageId::CtxInspCritical => "crítico",
        MessageId::CtxInspIncluded => "incluído",
        MessageId::CtxInspAttached => "anexado",
        MessageId::CtxInspNotIncluded => "não incluído",
        MessageId::CtxInspOutputCaptured => "saída capturada",
        MessageId::CtxInspNoOutputYet => "nenhuma saída ainda",
        MessageId::CtxInspNoSystemPrompt => "Nenhum prompt de sistema definido.",
        MessageId::CtxInspNoReferences => {
            "Nenhuma referência de arquivo, diretório ou mídia registrada ainda."
        }
        MessageId::CtxInspNoToolActivity => "Nenhuma atividade de ferramenta registrada ainda.",
        MessageId::CtxInspAltVHint => {
            "Abra o cartão correspondente e pressione Alt+V para detalhes completos."
        }
        MessageId::CtxInspCells => "células",
        MessageId::CtxInspApiMessages => "mensagens da API",
        MessageId::CtxInspActive => "ativo",
        MessageId::CtxInspCell => "célula",
        MessageId::CtxInspMoreReferences => "mais referência(s)",
        MessageId::CtxInspStablePrefix => "Prefixo estável",
        MessageId::CtxInspVolatileWorkingSet => "Conjunto de trabalho volátil",
        MessageId::CtxInspFirstLine => "Primeira linha",
        MessageId::CtxInspTotal => "Total",
        MessageId::CtxInspTextPromptLayers => "Camadas de prompt de texto",
        MessageId::CtxInspSingleTextBlob => "Bloco de texto único",
        MessageId::CtxInspBlocks => "bloco(s)",
        MessageId::CtxInspBlock => "bloco",
        MessageId::CtxInspTokens => "token(s)",
        MessageId::CtxInspLayers => "camada(s)",
        MessageId::CtxInspNone => "nenhum",
        MessageId::CtxInspEmpty => "(vazio)",
        MessageId::CtxInspCacheFriendly => "amigável ao cache",
        MessageId::CtxInspChangesByTurn => "muda por sessão/turno",
        MessageId::CtxInspStablePrefixOnly => "apenas prefixo estável",
        MessageId::CtxInspCacheTip => {
            "Dica: Blocos de prefixo estável são elegíveis para cache de prefixo DeepSeek V4. Alterações no conjunto de trabalho volátil quebram o cache apenas no final."
        }
        _ => english(id),
    })
}

fn spanish_latin_america(id: MessageId) -> Option<&'static str> {
    Some(match id {
        MessageId::ComposerPlaceholder => "Escribe una tarea o usa /.",
        MessageId::HistorySearchPlaceholder => "Buscar en el historial de prompts...",
        MessageId::HistorySearchTitle => "Búsqueda en el historial",
        MessageId::HistoryHintMove => "Arriba/Abajo mover",
        MessageId::HistoryHintAccept => "Enter aceptar",
        MessageId::HistoryHintRestore => "Esc restaurar",
        MessageId::HistoryNoMatches => "  Sin resultados",
        MessageId::StatusPickerTitle => " Línea de estado ",
        MessageId::StatusPickerInstruction => "Elige los elementos que quieres en el pie:",
        MessageId::StatusPickerActionToggle => "alternar ",
        MessageId::StatusPickerActionAll => "todos ",
        MessageId::StatusPickerActionNone => "ninguno ",
        MessageId::StatusPickerActionSave => "guardar ",
        MessageId::StatusPickerActionCancel => "cancelar ",
        MessageId::ConfigTitle => "Configuración de la sesión",
        MessageId::ConfigModalTitle => " Config ",
        MessageId::ConfigSearchPlaceholder => "escribe para filtrar",
        MessageId::ConfigNoSettings => "  No hay configuraciones disponibles.",
        MessageId::ConfigNoMatchesPrefix => "  Ninguna configuración coincide con ",
        MessageId::ConfigFilteredSettings => "  Configuraciones filtradas",
        MessageId::ConfigShowing => "  Mostrando",
        MessageId::ConfigFooterDefault => {
            " escribir=filtrar, Arriba/Abajo=seleccionar, Enter/e=editar, Esc/q=cerrar "
        }
        MessageId::ConfigFooterScrollable => {
            " escribir=filtrar, Arriba/Abajo=seleccionar, Enter/e=editar, PgUp/PgDn=desplazar, Esc/q=cerrar "
        }
        MessageId::ConfigFooterFiltered => {
            " escribir=filtrar, Backspace=borrar, Ctrl+U/Esc=limpiar, Enter=editar "
        }
        MessageId::HelpTitle => "Ayuda",
        MessageId::HelpFilterPlaceholder => "Escribe para filtrar",
        MessageId::HelpFilterPrefix => "Filtro: ",
        MessageId::HelpNoMatches => "  Sin resultados.",
        MessageId::HelpSlashCommands => "Comandos con barra",
        MessageId::HelpKeybindings => "Atajos de teclado",
        MessageId::HelpFooterTypeFilter => " escribir para filtrar ",
        MessageId::HelpFooterMove => "  Arriba/Abajo mover ",
        MessageId::HelpFooterJump => " PgUp/PgDn saltar ",
        MessageId::HelpFooterClose => " Esc cerrar ",
        MessageId::CmdAnchorDescription => {
            "Fijar un dato que sobrevive a la compactación (inyectado automáticamente en el contexto)"
        }
        MessageId::CmdAttachDescription => {
            "Adjuntar imagen o video; usa @ruta para archivos de texto o directorios"
        }
        MessageId::CmdCacheDescription => {
            "Mostrar estadísticas de hit/miss del caché de prefijo DeepSeek en las últimas N rondas"
        }
        MessageId::CmdChangeDescription => "Mostrar la entrada más reciente del changelog",
        MessageId::CmdChangeHeader => "Changelog más reciente",
        MessageId::CmdChangeTranslationQueued => {
            "Las notas de la versión en inglés se muestran abajo. Se solicitará una versión traducida a continuación; si el proveedor no está disponible, este texto en inglés será el fallback."
        }
        MessageId::CmdChangeTranslationUnavailable => {
            "Las notas de la versión en inglés se muestran abajo. La traducción no está disponible porque la sesión actual no tiene clave de API o está offline."
        }
        MessageId::CmdChangePreviousVersion => {
            "Versión anterior: {version} — ejecuta `/change {version}` para verla"
        }
        MessageId::CmdBalanceDescription => "Consultar el saldo de la cuenta del proveedor activo",
        MessageId::CmdClearDescription => "Limpiar el historial de la conversación",
        MessageId::CmdCompactDescription => "Compactar el contexto para liberar espacio",
        MessageId::CmdPurgeDescription => {
            "Permite al agente eliminar quirúrgicamente historial innecesario para liberar espacio de contexto"
        }
        MessageId::CmdConfigDescription => "Abrir el editor interactivo de configuración",
        MessageId::CmdContextDescription => "Abrir el inspector compacto de contexto de la sesión",
        MessageId::CmdCostDescription => "Mostrar el desglose de costo de la sesión",
        MessageId::CmdDiffDescription => "Mostrar cambios en archivos desde el inicio de la sesión",
        MessageId::CmdEditDescription => "Revisar y reenviar el último mensaje",
        MessageId::CmdExitDescription => "Salir de la aplicación",
        MessageId::CmdExportDescription => "Exportar la conversación a markdown",
        MessageId::CmdFeedbackDescription => "Generar una URL de feedback en GitHub",
        MessageId::CmdHfDescription => "Inspeccionar configuracion y conceptos de Hugging Face MCP",
        MessageId::CmdHelpDescription => "Mostrar información de ayuda",
        MessageId::CmdHomeDescription => {
            "Mostrar el panel inicial con estadísticas y acciones rápidas"
        }
        MessageId::CmdHooksDescription => {
            "Listar hooks de ciclo de vida configurados (solo lectura)"
        }
        MessageId::CmdAgentDescription => {
            "Abrir una sesión persistente de sub-agente: /agent [0-3] <tarea>"
        }
        MessageId::CmdGoalDescription => {
            "Definir una meta de sesión con presupuesto de tokens opcional"
        }
        MessageId::CmdInitDescription => "Generar AGENTS.md para el proyecto",
        MessageId::CmdLspDescription => "Alternar diagnóstico LSP encendido o apagado",
        MessageId::CmdShareDescription => "Exportar la sesión actual como una URL web compartible",
        MessageId::CmdJobsDescription => {
            "Inspeccionar y controlar trabajos de shell en segundo plano"
        }
        MessageId::CmdLinksDescription => "Mostrar enlaces del panel y documentación de DeepSeek",
        MessageId::CmdLoadDescription => "Cargar la sesión desde un archivo",
        MessageId::CmdLogoutDescription => "Limpiar la clave de API y volver a la configuración",
        MessageId::CmdMcpDescription => "Abrir o gestionar servidores MCP",
        MessageId::CmdMemoryDescription => {
            "Inspeccionar o gestionar el archivo persistente de memoria del usuario"
        }
        MessageId::CmdModeDescription => {
            "Alternar modo o abrir selector: /mode [agent|plan|yolo|1|2|3]"
        }
        MessageId::CmdModelDescription => "Cambiar o mostrar el modelo actual",
        MessageId::CmdModelsDescription => "Listar los modelos disponibles por la API",
        MessageId::CmdNetworkDescription => "Gestionar reglas de red permitidas y bloqueadas",
        MessageId::CmdNoteDescription => {
            "Agregar nota al archivo persistente (.codewhale/notes.md)"
        }
        MessageId::CmdThemeDescription => "Alternar entre tema claro y oscuro",
        MessageId::CmdProviderDescription => {
            "Cambiar o mostrar el backend LLM activo (deepseek | nvidia-nim | ollama)"
        }
        MessageId::CmdQueueDescription => "Ver o editar mensajes en cola",
        MessageId::CmdQueueUsage => "Uso: /queue [list|edit <n>|drop <n>|clear]",
        MessageId::CmdQueueDraftHeader => "Editando mensaje en cola:",
        MessageId::CmdQueueNoMessages => "No hay mensajes en cola",
        MessageId::CmdQueueListHeader => "Mensajes en cola ({count}):",
        MessageId::CmdQueueTip => {
            "Consejo: /queue edit <n> para editar, /queue drop <n> para eliminar"
        }
        MessageId::CmdQueueAlreadyEditing => {
            "Ya estás editando un mensaje en cola. Envíalo o usa /queue clear para descartarlo."
        }
        MessageId::CmdQueueNotFound => "Mensaje en cola no encontrado",
        MessageId::CmdQueueEditingStatus => "Editando mensaje en cola {index}",
        MessageId::CmdQueueEditingMessage => {
            "Editando mensaje en cola {index} (presiona Enter para re-encolar/enviar)"
        }
        MessageId::CmdQueueDropped => "Mensaje en cola {index} eliminado",
        MessageId::CmdQueueAlreadyEmpty => "La cola ya está vacía",
        MessageId::CmdQueueCleared => "Cola limpiada",
        MessageId::CmdQueueMissingIndex => {
            "Índice faltante. Uso: /queue edit <n> o /queue drop <n>"
        }
        MessageId::CmdQueueIndexPositive => "El índice debe ser un número positivo",
        MessageId::CmdQueueIndexMin => "El índice debe ser >= 1",
        MessageId::CmdRelayDescription => "Crear un relay de sesión (接力) para un hilo nuevo",
        MessageId::CmdRenameDescription => "Renombrar la sesión actual",
        MessageId::CmdRestoreDescription => {
            "Revertir el workspace a un snapshot pre/post-turno anterior. Sin argumento, lista los snapshots recientes."
        }
        MessageId::CmdRetryDescription => "Repetir la última solicitud",
        MessageId::CmdReviewDescription => {
            "Ejecutar una revisión de código estructurada en un archivo, diff o PR"
        }
        MessageId::CmdRlmDescription => {
            "Turno del Recursive Language Model (RLM) — guarda el prompt en un REPL Python y deja que el modelo escriba el código que lo procesa; usa `llm_query()` / `sub_rlm()` para llamadas a sub-LLMs."
        }
        MessageId::CmdSaveDescription => "Guardar la sesión en archivo",
        MessageId::CmdForkDescription => "Bifurcar la conversación activa a una sesión hermana",
        MessageId::CmdNewDescription => "Iniciar una nueva sesión guardada",
        MessageId::CmdSessionsDescription => "Abrir el selector de sesiones",
        MessageId::CmdSettingsDescription => "Mostrar las configuraciones persistidas",
        MessageId::CmdSidebarDescription => "Toggle or focus the right sidebar",
        MessageId::CmdSkillDescription => {
            "Activar una skill, o instalar/actualizar/desinstalar/confiar en una skill de la comunidad"
        }
        MessageId::CmdSkillsDescription => {
            "Listar skills locales (filtra con `/skills <prefijo>`; --remote navega el registro curado)"
        }
        MessageId::CmdSlopDescription => "Inspect or export the SlopLedger",
        MessageId::CmdStashDescription => {
            "Estacionar o restaurar borrador del compositor (Ctrl+S estaciona, /stash list|pop)"
        }
        MessageId::CmdStatusDescription => "Mostrar el estado de la sesión en ejecución",
        MessageId::CmdStatuslineDescription => {
            "Configurar qué elementos aparecen en el pie de página"
        }
        MessageId::CmdSubagentsDescription => "Listar el estado de los sub-agentes",
        MessageId::CmdSwarmDescription => {
            "Ejecutar turno fanout multi-agente (sequential | mixture | distill | deliberate)"
        }
        MessageId::CmdSystemDescription => "Mostrar el prompt de sistema actual",
        MessageId::CmdTaskDescription => "Gestionar tareas en segundo plano",
        MessageId::CmdTokensDescription => "Mostrar el uso de tokens de la sesión",
        MessageId::CmdTranslateDescription => {
            "Activar o desactivar la traducción de salida al idioma actual del sistema"
        }
        MessageId::CmdTranslateOff => {
            "Traducción de salida desactivada (se muestra la salida original del modelo)"
        }
        MessageId::CmdTranslateOn => {
            "Traducción de salida activada: las respuestas del modelo se mostrarán en el idioma del sistema"
        }
        MessageId::TranslationInProgress => "Traduciendo la salida del asistente...",
        MessageId::TranslationComplete => "Traducción completada",
        MessageId::TranslationFailed => "Traducción fallida",
        MessageId::CmdTrustDescription => {
            "Gestionar la confianza del workspace y la lista de paths permitidos (`/trust add <ruta>`, `/trust list`, `/trust on|off`)"
        }
        MessageId::CmdWorkspaceDescription => "Mostrar o cambiar el workspace actual",
        MessageId::CmdUndoDescription => "Eliminar el último par de mensajes",
        MessageId::CmdVerboseDescription => {
            "Alternar pensamiento en vivo completo en la transcripción"
        }
        MessageId::CmdCacheAdvice => {
            "Tasas de hit/miss arriba del ~70% a partir del tercer turno indican un prefijo de caché estable;\n\
             valores menores en sesiones largas sugieren inestabilidad en el prefijo, vale investigar (#263)."
        }
        MessageId::CmdCacheFootnote => {
            "* miss inferido a partir de entrada − hit cuando el proveedor no lo reporta por separado.\n"
        }
        MessageId::CmdCacheHeader => {
            "Telemetría del caché — últimos {count} de {total} turno(s) (modelo: {model})\n"
        }
        MessageId::CmdCacheNoData => {
            "Historial del caché: ningún turno registrado todavía.\n\n\
             DeepSeek expone `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` en cada turno \
             de la API donde el modelo lo soporta (familia V4). Ejecuta un turno y prueba /cache de nuevo."
        }
        MessageId::CmdCacheTotals => {
            "Σ entrada: {sum_in}   Σ hit: {sum_hit}   Σ miss: {sum_miss}   tasa promedio de hit: {avg}\n"
        }
        MessageId::CmdCostReport => {
            "Costo de la sesión:\n\
             ─────────────────────────────\n\
             Total aproximado: {cost}\n\n\
             Las estimaciones de costo son aproximadas y usan la telemetría de uso del proveedor cuando está disponible.\n\n\
             Precios de la API DeepSeek:\n\
             ─────────────────────────────\n\
             Los detalles de precio no están configurados en esta CLI."
        }
        MessageId::CmdTokensCacheBoth => "{hit} hit / {miss} miss",
        MessageId::CmdTokensCacheHitOnly => "{hit} hit / miss no reportado",
        MessageId::CmdTokensCacheMissOnly => "hit no reportado / {miss} miss",
        MessageId::CmdTokensContextUnknownWindow => "~{estimated} / ventana desconocida",
        MessageId::CmdTokensContextWithWindow => "~{used} / {window} ({percent}%)",
        MessageId::FooterAgentSingular => "1 sub-agente",
        MessageId::FooterAgentsPlural => "{count} sub-agentes",
        MessageId::FooterPressCtrlCAgain => "Presiona Ctrl+C de nuevo para salir",
        MessageId::FooterWorking => "trabajando",
        MessageId::FooterBalancePrefix => "saldo",
        MessageId::HelpSectionActions => "Acciones",
        MessageId::HelpSectionClipboard => "Portapapeles",
        MessageId::HelpSectionEditing => "Edición de entrada",
        MessageId::HelpSectionHelp => "Ayuda",
        MessageId::HelpSectionModes => "Modos",
        MessageId::HelpSectionNavigation => "Navegación",
        MessageId::HelpSectionSessions => "Sesiones",
        MessageId::CmdTokensNotReported => "no reportado",
        MessageId::CmdTokensReport => {
            "Uso de tokens:\n\
             ─────────────────────────────\n\
             Contexto activo:           {active}\n\
             Última entrada de API:     {input} (telemetría por turno; puede contar el mismo prefijo varias veces en rondas con herramientas)\n\
             Última salida de API:      {output}\n\
             Hit/miss del caché:        {cache} (solo para telemetría/costo)\n\
             Tokens acumulados:         {total} (telemetría de uso de la sesión)\n\
             Costo aproximado:          {cost}\n\
             Mensajes de API:           {api_messages}\n\
             Mensajes del chat:         {chat_messages}\n\
             Modelo:                    {model}"
        }
        MessageId::KbScrollTranscript => {
            "Desplazar transcripción, navegar historial de entrada o seleccionar adjuntos del compositor"
        }
        MessageId::KbNavigateHistory => "Navegar historial de entrada",
        MessageId::KbBrowseHistory => "Explorar historial de conversación",
        MessageId::KbScrollTranscriptAlt => "Desplazar transcripción",
        MessageId::KbScrollPage => "Desplazar transcripción por página",
        MessageId::KbJumpTopBottom => "Saltar al inicio / fin de la transcripción",
        MessageId::KbJumpTopBottomEmpty => "Saltar al inicio / fin (cuando la entrada está vacía)",
        MessageId::KbJumpToolBlocks => "Saltar entre bloques de salida de herramientas",
        MessageId::KbMoveCursor => "Mover cursor en el compositor",
        MessageId::KbJumpLineStartEnd => "Saltar al inicio / fin de la línea",
        MessageId::KbDeleteChar => {
            "Eliminar carácter antes / después del cursor, o quitar adjunto seleccionado"
        }
        MessageId::KbClearDraft => "Limpiar borrador actual",
        MessageId::KbStashDraft => "Estacionar borrador actual (`/stash pop` restaura)",
        MessageId::KbSearchHistory => "Buscar historial de prompts y recuperar borradores locales",
        MessageId::KbInsertNewline => "Insertar nueva línea en el compositor",
        MessageId::KbSendDraft => "Enviar borrador actual",
        MessageId::KbCloseMenu => {
            "Cerrar menú, cancelar solicitud, descartar borrador o limpiar entrada"
        }
        MessageId::KbCancelOrExit => "Cancelar solicitud o salir cuando está inactivo",
        MessageId::KbShellControls => "Abrir controles de shell para comando en primer plano",
        MessageId::KbExitEmpty => "Salir cuando la entrada está vacía",
        MessageId::KbCommandPalette => "Abrir paleta de comandos",
        MessageId::KbFuzzyFilePicker => {
            "Abrir selector de archivo fuzzy (inserta @ruta al presionar Enter)"
        }
        MessageId::KbCompactInspector => "Abrir inspector compacto de contexto de la sesión",
        MessageId::KbLastMessagePager => {
            "Abrir paginador para el último mensaje (cuando la entrada está vacía)"
        }
        MessageId::KbSelectedDetails => {
            "Abrir detalles de la herramienta o mensaje seleccionado (cuando la entrada está vacía)"
        }
        MessageId::KbToolDetailsPager => "Abrir paginador de detalles de la herramienta",
        MessageId::KbThinkingPager => "Abrir paginador de razonamiento",
        MessageId::KbLiveTranscript => "Abrir superposición de transcripción en vivo (auto-scroll)",
        MessageId::KbBacktrackMessage => {
            "Retroceder al mensaje anterior del usuario (izquierda/derecha, Enter para rebobinar)"
        }
        MessageId::KbCompleteCycleModes => {
            "Completar /command, encolar follow-up, ciclar modos; Shift+Tab cicla esfuerzo de razonamiento"
        }
        MessageId::KbJumpPlanAgentYolo => "Saltar directo a modo Plan / Agent / YOLO",
        MessageId::KbAltJumpPlanAgentYolo => "Salto alternativo a modo Plan / Agent / YOLO",
        MessageId::KbFocusSidebar => {
            "Enfocar barra lateral Work / Tasks / Agents / Context / Auto / Ocultar"
        }
        MessageId::KbTogglePlanAgent => "Alternar entre modos Plan y Agent",
        MessageId::KbSessionPicker => "Abrir selector de sesiones",
        MessageId::KbPasteAttach => "Pegar texto o adjuntar imagen del portapapeles",
        MessageId::KbCopySelection => "Copiar selección actual (Cmd+C en macOS)",
        MessageId::KbContextMenu => {
            "Abrir acciones de contexto para pegar, selección, detalles, contexto y ayuda"
        }
        MessageId::KbAttachPath => "Agregar archivo o directorio local al contexto",
        MessageId::KbHelpOverlay => {
            "Abrir esta superposición de ayuda (cuando la entrada está vacía)"
        }
        MessageId::KbToggleHelp => "Alternar superposición de ayuda",
        MessageId::KbToggleHelpSlash => "Alternar superposición de ayuda",
        MessageId::HelpUsageLabel => "Uso:",
        MessageId::HelpAliasesLabel => "Alias:",
        MessageId::SettingsTitle => "Configuraciones:",
        MessageId::SettingsConfigFile => "Archivo de configuración:",
        MessageId::ClearConversation => "Conversación limpia",
        MessageId::ClearConversationBusy => {
            "Conversación limpia (estado del plan ocupado; ejecuta /clear de nuevo si es necesario)"
        }
        MessageId::ModelChanged => "Modelo cambiado: {old} \u{2192} {new}",
        MessageId::LinksTitle => "Enlaces de DeepSeek:",
        MessageId::LinksDashboard => "Panel:",
        MessageId::LinksDocs => "Documentación:",
        MessageId::LinksTip => "Tip: las claves de API están disponibles en la consola del panel.",
        MessageId::SubagentsFetching => "Obteniendo estado de los sub-agentes...",
        MessageId::HelpUnknownCommand => "Comando desconocido: {topic}",
        MessageId::HomeDashboardTitle => "Panel Inicial de codewhale",
        MessageId::HomeModel => "Modelo:",
        MessageId::HomeMode => "Modo:",
        MessageId::HomeWorkspace => "Workspace:",
        MessageId::HomeHistory => "Historial:",
        MessageId::HomeTokens => "Tokens:",
        MessageId::HomeQueued => "En cola:",
        MessageId::HomeSubagents => "Sub-agentes:",
        MessageId::HomeSkill => "Skill:",
        MessageId::HomeQuickActions => "Acciones Rápidas",
        MessageId::HomeQuickLinks => "/links      - Enlaces del panel y API",
        MessageId::HomeQuickSkills => "/skills      - Listar skills disponibles",
        MessageId::HomeQuickConfig => "/config      - Abrir editor interactivo de configuración",
        MessageId::HomeQuickSettings => "/settings    - Mostrar configuraciones persistentes",
        MessageId::HomeQuickModel => "/model       - Alternar o visualizar modelo",
        MessageId::HomeQuickSubagents => "/subagents   - Listar estado de los sub-agentes",
        MessageId::HomeQuickTaskList => "/task list   - Mostrar fila de tareas en segundo plano",
        MessageId::HomeQuickHelp => "/help        - Mostrar ayuda",
        MessageId::HomeModeTips => "Tips de Modo",
        MessageId::HomeAgentModeTip => "Modo Agent - Usar herramientas para tareas autónomas",
        MessageId::HomeAgentModeReviewTip => {
            "  Usa Ctrl+X para revisar en modo Plan antes de ejecutar"
        }
        MessageId::HomeAgentModeYoloTip => {
            "  Escribe /mode yolo para habilitar acceso total a las herramientas"
        }
        MessageId::HomeYoloModeTip => "Modo YOLO - Acceso total a herramientas, sin aprobaciones",
        MessageId::HomeYoloModeCaution => "  ¡Ten cuidado con operaciones destructivas!",
        MessageId::HomePlanModeTip => "Modo Plan - Planear antes de implementar",
        MessageId::HomePlanModeChecklistTip => {
            "  Usa /mode plan para crear checklists estructurados"
        }
        MessageId::HomeGoalModeTip => {
            "Seguimiento de Goal - Usa /goal <objetivo> para seguir un objetivo persistente"
        }
        MessageId::OnboardLanguageTitle => "Elige el idioma",
        MessageId::OnboardLanguageBlurb => {
            "Elige el idioma de la interfaz. Puedes cambiarlo en cualquier momento con `/settings set locale <etiqueta>`."
        }
        MessageId::OnboardLanguageFooter => {
            "Presiona 1-7 para elegir, o Enter para mantener la configuración actual"
        }
        MessageId::OnboardApiKeyTitle => "Conecta tu clave de API DeepSeek",
        MessageId::OnboardApiKeyStep1 => {
            "Paso 1.  Abre https://platform.deepseek.com/api_keys y crea una clave."
        }
        MessageId::OnboardApiKeyStep2 => "Paso 2.  Pégala abajo y presiona Enter.",
        MessageId::OnboardApiKeySavedHint => {
            "Guardada en ~/.codewhale/config.toml para funcionar en cualquier carpeta."
        }
        MessageId::OnboardApiKeyFormatHint => {
            "Pega la clave completa tal como fue emitida (sin espacios ni saltos de línea)."
        }
        MessageId::OnboardApiKeyPlaceholder => "(pega la clave acá)",
        MessageId::OnboardApiKeyLabel => "Clave: ",
        MessageId::OnboardApiKeyFooter => "Enter para guardar, Esc para volver.",
        MessageId::OnboardTrustTitle => "Confiar en el directorio",
        MessageId::OnboardTrustQuestion => "¿Confías en el contenido de este directorio?",
        MessageId::OnboardTrustLocationPrefix => "Estás en ",
        MessageId::OnboardTrustRiskHint => {
            "Trabajar con contenido no confiable aumenta el riesgo de inyección de prompt."
        }
        MessageId::OnboardTrustEffectHint => {
            "Confiar en este directorio lo registra en la configuración global y habilita el modo workspace confiable."
        }
        MessageId::OnboardTrustFooterPrefix => "Presiona ",
        MessageId::OnboardTrustFooterMiddle => " para confiar y continuar, ",
        MessageId::OnboardTrustFooterSuffix => " para salir",
        MessageId::OnboardTipsTitle => "Empieza simple",
        MessageId::OnboardTipsLine1 => {
            "Escribe la tarea en lenguaje natural. Usa /help o Ctrl+K para comandos."
        }
        MessageId::OnboardTipsLine2 => {
            "El composer inferior es multilínea: Enter envía, Alt+Enter o Ctrl+J agrega una nueva línea."
        }
        MessageId::OnboardTipsLine3 => {
            "Cambia de modo solo cuando el trabajo cambie: Plan para revisar antes, Agent para ejecución, YOLO para auto-aprobación."
        }
        MessageId::OnboardTipsLine4 => {
            "Ctrl+R retoma sesiones anteriores, y Esc cancela el borrador o superposición actual."
        }
        MessageId::OnboardTipsFooterEnter => "Presiona Enter",
        MessageId::OnboardTipsFooterAction => " para abrir el workspace",
        // Context menu.
        MessageId::CtxMenuTitle => " Clic derecho ",
        MessageId::CtxMenuCopySelection => "Copiar selección",
        MessageId::CtxMenuCopySelectionDesc => "copiar texto seleccionado de la transcripción",
        MessageId::CtxMenuOpenSelection => "Abrir selección",
        MessageId::CtxMenuOpenSelectionDesc => "mostrar texto seleccionado en el visor",
        MessageId::CtxMenuClearSelection => "Limpiar selección",
        MessageId::CtxMenuOpenDetails => "Abrir detalles",
        MessageId::CtxMenuCopyMessage => "Copiar mensaje",
        MessageId::CtxMenuCopyMessageDesc => "copiar celda de transcripción seleccionada",
        MessageId::CtxMenuOpenInEditor => "Abrir en editor",
        MessageId::CtxMenuOpenInEditorDesc => "abrir file:line en $EDITOR",
        MessageId::CtxMenuShowCell => "Mostrar celda",
        MessageId::CtxMenuShowCellDesc => "volver a mostrar esta celda de transcripción",
        MessageId::CtxMenuHideCell => "Ocultar celda",
        MessageId::CtxMenuHideCellDesc => "colapsar esta celda de transcripción",
        MessageId::CtxMenuShowHidden => "Mostrar ocultas",
        MessageId::CtxMenuShowHiddenDesc => "volver a mostrar todas las celdas colapsadas",
        MessageId::CtxMenuPaste => "Pegar",
        MessageId::CtxMenuPasteDesc => "insertar portapapeles en el compositor",
        MessageId::CtxMenuCmdPalette => "Paleta de comandos",
        MessageId::CtxMenuCmdPaletteDesc => "comandos, habilidades y herramientas",
        MessageId::CtxMenuContextInspector => "Inspector de contexto",
        MessageId::CtxMenuContextInspectorDesc => "contexto activo y sugerencias de caché",
        MessageId::CtxMenuHelp => "Ayuda",
        MessageId::CtxMenuHelpDesc => "atajos de teclado y comandos",
        MessageId::FanoutCounts => {
            "{done} completado · {running} ejecutando · {failed} falló · {pending} pendiente"
        }

        // Approval dialog.
        MessageId::ApprovalRiskReview => "REVISAR",
        MessageId::ApprovalRiskDestructive => "DESTRUCTIVO",
        MessageId::ApprovalCategorySafe => "Seguro",
        MessageId::ApprovalCategoryFileWrite => "Escritura de Archivo",
        MessageId::ApprovalCategoryShell => "Comando Shell",
        MessageId::ApprovalCategoryNetwork => "Red",
        MessageId::ApprovalCategoryMcpRead => "Lectura MCP",
        MessageId::ApprovalCategoryMcpAction => "Acción MCP",
        MessageId::ApprovalCategoryUnknown => "Desconocido",
        MessageId::ApprovalFieldType => "Tipo:",
        MessageId::ApprovalFieldAbout => "Acerca de:",
        MessageId::ApprovalFieldImpact => "Impacto:",
        MessageId::ApprovalFieldParams => "Parámetros:",
        MessageId::ApprovalOptionApproveOnce => "Aprobar una vez",
        MessageId::ApprovalOptionApproveAlways => "Aprobar siempre para este tipo",
        MessageId::ApprovalOptionDeny => "Denegar esta llamada",
        MessageId::ApprovalOptionAbortTurn => "Abortar turno",
        MessageId::ApprovalBlockTitle => "aprobación",
        MessageId::ApprovalControlsHint => "  ·  v: parámetros  ·  Esc: abortar",
        MessageId::ApprovalChooseHint => "Elegir: ",
        MessageId::ApprovalChooseAction => "Enter para seleccionar, o presione y/a/d directamente",
        MessageId::ApprovalIntentLabel => "Intención: ",
        MessageId::ApprovalMoreLines => "  … (+{count} líneas)",

        MessageId::CtxInspTitle => "Inspector de contexto",
        MessageId::CtxInspSessionContext => "Contexto de la sesión",
        MessageId::CtxInspSystemPrompt => "Estructura del prompt del sistema",
        MessageId::CtxInspReferences => "Referencias",
        MessageId::CtxInspRecentTools => "Herramientas recientes",
        MessageId::CtxInspModel => "Modelo",
        MessageId::CtxInspWorkspace => "Espacio de trabajo",
        MessageId::CtxInspSession => "Sesión",
        MessageId::CtxInspContext => "Contexto",
        MessageId::CtxInspTranscript => "Transcripción",
        MessageId::CtxInspWorkspaceStatus => "Estado del espacio de trabajo",
        MessageId::CtxInspNotSampledYet => "aún no muestreado",
        MessageId::CtxInspOk => "bien",
        MessageId::CtxInspHigh => "alto",
        MessageId::CtxInspCritical => "crítico",
        MessageId::CtxInspIncluded => "incluido",
        MessageId::CtxInspAttached => "adjunto",
        MessageId::CtxInspNotIncluded => "no incluido",
        MessageId::CtxInspOutputCaptured => "salida capturada",
        MessageId::CtxInspNoOutputYet => "sin salida aún",
        MessageId::CtxInspNoSystemPrompt => "No hay prompt de sistema establecido.",
        MessageId::CtxInspNoReferences => {
            "Aún no se han registrado referencias de archivos, directorios o medios."
        }
        MessageId::CtxInspNoToolActivity => "Aún no se ha registrado actividad de herramientas.",
        MessageId::CtxInspAltVHint => {
            "Abra la tarjeta correspondiente y presione Alt+V para ver los detalles completos."
        }
        MessageId::CtxInspCells => "celdas",
        MessageId::CtxInspApiMessages => "mensajes de API",
        MessageId::CtxInspActive => "activo",
        MessageId::CtxInspCell => "celda",
        MessageId::CtxInspMoreReferences => "más referencia(s)",
        MessageId::CtxInspStablePrefix => "Prefijo estable",
        MessageId::CtxInspVolatileWorkingSet => "Conjunto de trabajo volátil",
        MessageId::CtxInspFirstLine => "Primera línea",
        MessageId::CtxInspTotal => "Total",
        MessageId::CtxInspTextPromptLayers => "Capas de prompt de texto",
        MessageId::CtxInspSingleTextBlob => "Bloque de texto único",
        MessageId::CtxInspBlocks => "bloque(s)",
        MessageId::CtxInspBlock => "bloque",
        MessageId::CtxInspTokens => "token(es)",
        MessageId::CtxInspLayers => "capa(s)",
        MessageId::CtxInspNone => "ninguno",
        MessageId::CtxInspEmpty => "(vacío)",
        MessageId::CtxInspCacheFriendly => "amigable con caché",
        MessageId::CtxInspChangesByTurn => "cambia por sesión/turno",
        MessageId::CtxInspStablePrefixOnly => "solo prefijo estable",
        MessageId::CtxInspCacheTip => {
            "Consejo: Los bloques de prefijo estable son elegibles para caché de prefijo DeepSeek V4. Los cambios en el conjunto de trabajo volátil solo rompen la caché al final."
        }
        _ => english(id),
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
        assert_eq!(normalize_configured_locale("zh-TW"), Some("zh-Hant"));
        assert_eq!(normalize_configured_locale("zh_HK.UTF-8"), Some("zh-Hant"));
        assert_eq!(normalize_configured_locale("pt"), Some("pt-BR"));
        assert_eq!(normalize_configured_locale("pt-PT"), Some("pt-BR"));
        assert_eq!(normalize_configured_locale("es"), Some("es-419"));
        assert_eq!(normalize_configured_locale("es-MX"), Some("es-419"));
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
        assert_eq!(
            resolve_locale_with_env("auto", |key| {
                (key == "LANG").then(|| "zh_TW.UTF-8".to_string())
            }),
            Locale::ZhHant
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
    fn provider_description_is_present_for_all_locales() {
        for locale in Locale::shipped() {
            let description = tr(*locale, MessageId::CmdProviderDescription);
            assert!(
                !description.is_empty(),
                "{} provider description should not be empty",
                locale.tag()
            );
            assert!(
                !description.contains("codewhale |"),
                "{} provider description should not name codewhale as a backend: {description}",
                locale.tag()
            );
        }
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
