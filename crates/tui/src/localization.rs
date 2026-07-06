//! Lightweight localization registry for high-visibility TUI strings.
//!
//! This intentionally covers UI chrome only. It does not change model prompts,
//! model output language, provider behavior, or media payload semantics.
use std::borrow::Cow;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    ConfigSectionProvider,
    ConfigSectionModel,
    ConfigSectionPermissions,
    ConfigSectionNetwork,
    ConfigSectionDisplay,
    ConfigSectionComposer,
    ConfigSectionSidebar,
    ConfigSectionHistory,
    ConfigSectionMcp,
    ConfigSectionFleet,
    ConfigSectionExperimental,
    ConfigScopeSession,
    ConfigScopeSaved,
    ConfigEditCancelled,
    ConfigEditTitlePrefix,
    ConfigEditScopeLabel,
    ConfigEditCurrentLabel,
    ConfigEditHintLabel,
    ConfigEditNewLabel,
    ConfigEditFooter,
    ConfigRowEffective,
    ConfigDefaultValue,
    ConfigDefaultReasoning,
    ConfigUnavailable,
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
    CmdConstitutionDescription,
    CmdContextDescription,
    CmdCostDescription,
    CmdDiffDescription,
    CmdEditDescription,
    CmdExitDescription,
    CmdExportDescription,
    CmdFeedbackDescription,
    CmdHfDescription,
    CmdHelpDescription,
    CmdProfileDescription,
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
    CmdPluginDescription,
    CmdPluginNoneFound,
    CmdPluginNotFound,
    CmdPluginListHeader,
    CmdPluginDetailDescription,
    CmdPluginDetailSchema,
    CmdPluginDetailApproval,
    CmdPluginDetailPath,
    CmdModeDescription,
    CmdModelDescription,
    CmdModelsDescription,
    CmdModelDbDescription,
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
    CmdFleetDescription,
    CmdHotbarDescription,
    CmdSetupDescription,
    CmdSubagentsDescription,
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
    KbCancelBackgroundShellJobs,
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
    // Onboarding screens — welcome.
    OnboardWelcomeVersion,
    OnboardWelcomeLead,
    OnboardWelcomeSetupBlurb,
    OnboardWelcomeSteps,
    OnboardWelcomeStepLanguage,
    OnboardWelcomeStepApiKey,
    OnboardWelcomeStepTrust,
    OnboardWelcomeStepTips,
    OnboardWelcomeDefaults,
    OnboardWelcomeEnter,
    OnboardWelcomeExit,
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
    // Constitution-first setup wizard.
    SetupWizardTitle,
    SetupWizardWhy,
    SetupWizardProgress,
    SetupActionBack,
    SetupActionContinue,
    SetupActionSkip,
    SetupActionRetry,
    SetupActionGuided,
    SetupActionTuneGuided,
    SetupActionModelDraft,
    SetupActionKeepExisting,
    SetupActionProvider,
    SetupActionModel,
    SetupActionMode,
    SetupActionConfig,
    SetupActionRuntimePreset,
    SetupActionApplyRuntimePreset,
    SetupActionUseBundled,
    SetupActionDefer,
    SetupActionCancel,
    SetupStatusNotStarted,
    SetupStatusRecommended,
    SetupStatusOptional,
    SetupStatusDeferred,
    SetupStatusInProgress,
    SetupStatusNeedsAction,
    SetupStatusVerified,
    SetupStatusSkipped,
    SetupStatusFailed,
    SetupStepLanguageTitle,
    SetupStepLanguageWhy,
    SetupStepProviderModelTitle,
    SetupStepProviderModelWhy,
    SetupStepTrustSandboxTitle,
    SetupStepTrustSandboxWhy,
    SetupStepToolsMcpTitle,
    SetupStepToolsMcpWhy,
    SetupStepHotbarTitle,
    SetupStepHotbarWhy,
    SetupStepRemoteRuntimeTitle,
    SetupStepRemoteRuntimeWhy,
    SetupStepConstitutionTitle,
    SetupStepConstitutionWhy,
    SetupStepVerificationTitle,
    SetupStepVerificationWhy,
    SetupCheckpointLayerOrder,
    SetupCheckpointDoneBundled,
    SetupCheckpointDoneGuided,
    SetupCheckpointDoneKept,
    SetupCheckpointDeferred,
    SetupStepSkipped,
    SetupStepRetryRecorded,
    SetupConstitutionChoiceLabel,
    SetupConstitutionSourceLabel,
    SetupConstitutionValidityLabel,
    SetupConstitutionPreviewLabel,
    SetupConstitutionExistingLabel,
    SetupConstitutionGuidedHint,
    SetupConstitutionGuidedAnswersHint,
    SetupConstitutionPurposeLabel,
    SetupConstitutionAutonomyLabel,
    SetupConstitutionEvidenceLabel,
    SetupConstitutionCommunicationLabel,
    SetupConstitutionPrivacyLabel,
    SetupConstitutionPrinciplesLabel,
    SetupCardRouteLabel,
    SetupCardModelLabel,
    SetupCardAuthLabel,
    SetupCardHealthLabel,
    SetupCardIntentLabel,
    SetupCardApprovalLabel,
    SetupCardShellLabel,
    SetupCardTrustLabel,
    SetupCardSandboxLabel,
    SetupCardNetworkLabel,
    SetupProviderModelReadyHint,
    SetupProviderModelNeedsActionHint,
    SetupProviderModelReviewed,
    SetupProviderModelNeedsActionSaved,
    SetupRuntimePostureBoundary,
    SetupRuntimePostureReviewHint,
    SetupRuntimePostureReviewed,
    SetupRuntimePresetSelectedLabel,
    SetupRuntimePresetDiffLabel,
    SetupRuntimePresetAskFirstTitle,
    SetupRuntimePresetAskFirstDescription,
    SetupRuntimePresetNormalAgentTitle,
    SetupRuntimePresetNormalAgentDescription,
    SetupRuntimePresetHighTrustTitle,
    SetupRuntimePresetHighTrustDescription,
    SetupRuntimePresetPreviewTitle,
    SetupRuntimePresetSafetyFloor,
    SetupRuntimePresetApplyHint,
    SetupRuntimePresetApplied,
    SetupRuntimeProjectOverrideLabel,
    SetupRuntimeProjectOverrideNone,
    SetupReportFirstRunLabel,
    SetupReportUpdateLabel,
    SetupReportSourceLabel,
    SetupReportAutonomyLabel,
    SetupReportRuntimePostureLabel,
    SetupReportPersisted,
    SetupReportInherited,
    SetupReportReady,
    SetupReportRequired,
    SetupReportOptional,
    SetupReportRowsLabel,
    SetupReportNextActionLabel,
    SetupReportNextActionNone,
    SetupReportNextActionConstitution,
    SetupReportNextActionProvider,
    SetupReportNextActionRuntime,
    SetupReportNextActionRequired,
    SetupReportRecorded,
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

    // App mode picker (prompt, names, hints) and composer vim indicator.
    ModePickerPrompt,
    AppModeAgent,
    AppModeAuto,
    AppModeYolo,
    AppModePlan,
    AppModeAgentHint,
    AppModeAutoHint,
    AppModePlanHint,
    AppModeYoloHint,
    VimModeNormal,
    VimModeInsert,
    VimModeVisual,

    // Approval dialog — risk badges, category labels, field labels, options.
    ApprovalRiskReview,
    ApprovalRiskElevated,
    ApprovalRiskDestructive,
    ApprovalCategorySafe,
    ApprovalCategoryFileWrite,
    ApprovalCategoryShell,
    ApprovalCategoryNetwork,
    ApprovalCategoryMcpRead,
    ApprovalCategoryMcpAction,
    ApprovalCategoryAgent,
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
    // Sandbox elevation dialog.
    ElevationTitleSandboxDenied,
    ElevationTitleRequired,
    ElevationFieldTool,
    ElevationFieldCmd,
    ElevationFieldReason,
    ElevationImpactHeader,
    ElevationImpactNetwork,
    ElevationImpactWrite,
    ElevationImpactFullAccess,
    ElevationPromptProceed,
    ElevationOptionNetwork,
    ElevationOptionWrite,
    ElevationOptionFullAccess,
    ElevationOptionAbort,
    ElevationOptionNetworkDesc,
    ElevationOptionWriteDesc,
    ElevationOptionFullAccessDesc,
    ElevationOptionAbortDesc,

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
    CtxInspVHint,
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
    // Tool family labels (card headers, sidebar, footer).
    ToolFamilyRead,
    ToolFamilyPatch,
    ToolFamilyRun,
    ToolFamilyFind,
    ToolFamilyDelegate,
    ToolFamilyFanout,
    ToolFamilyRlm,
    ToolFamilyVerify,
    ToolFamilyThink,
    ToolFamilyGeneric,
    // Voice commands (/voice, /voice-send, /voice-control)
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
    VoiceErrTooShort,
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
    MessageId::ConfigSectionProvider,
    MessageId::ConfigSectionModel,
    MessageId::ConfigSectionPermissions,
    MessageId::ConfigSectionNetwork,
    MessageId::ConfigSectionDisplay,
    MessageId::ConfigSectionComposer,
    MessageId::ConfigSectionSidebar,
    MessageId::ConfigSectionHistory,
    MessageId::ConfigSectionMcp,
    MessageId::ConfigSectionFleet,
    MessageId::ConfigSectionExperimental,
    MessageId::ConfigScopeSession,
    MessageId::ConfigScopeSaved,
    MessageId::ConfigEditCancelled,
    MessageId::ConfigEditTitlePrefix,
    MessageId::ConfigEditScopeLabel,
    MessageId::ConfigEditCurrentLabel,
    MessageId::ConfigEditHintLabel,
    MessageId::ConfigEditNewLabel,
    MessageId::ConfigEditFooter,
    MessageId::ConfigRowEffective,
    MessageId::ConfigDefaultValue,
    MessageId::ConfigDefaultReasoning,
    MessageId::ConfigUnavailable,
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
    MessageId::CmdConstitutionDescription,
    MessageId::CmdContextDescription,
    MessageId::CmdCostDescription,
    MessageId::CmdDiffDescription,
    MessageId::CmdEditDescription,
    MessageId::CmdExitDescription,
    MessageId::CmdExportDescription,
    MessageId::CmdFeedbackDescription,
    MessageId::CmdHfDescription,
    MessageId::CmdHelpDescription,
    MessageId::CmdProfileDescription,
    MessageId::CmdHomeDescription,
    MessageId::CmdHooksDescription,
    MessageId::CmdAgentDescription,
    MessageId::CmdInitDescription,
    MessageId::CmdJobsDescription,
    MessageId::CmdLinksDescription,
    MessageId::CmdLoadDescription,
    MessageId::CmdLogoutDescription,
    MessageId::CmdMcpDescription,
    MessageId::CmdPluginDescription,
    MessageId::CmdPluginNoneFound,
    MessageId::CmdPluginNotFound,
    MessageId::CmdPluginListHeader,
    MessageId::CmdPluginDetailDescription,
    MessageId::CmdPluginDetailSchema,
    MessageId::CmdPluginDetailApproval,
    MessageId::CmdPluginDetailPath,
    MessageId::CmdMemoryDescription,
    MessageId::CmdModeDescription,
    MessageId::CmdModelDescription,
    MessageId::CmdModelsDescription,
    MessageId::CmdModelDbDescription,
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
    MessageId::CmdFleetDescription,
    MessageId::CmdHotbarDescription,
    MessageId::CmdSetupDescription,
    MessageId::CmdSubagentsDescription,
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
    MessageId::KbCancelBackgroundShellJobs,
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
    MessageId::OnboardWelcomeVersion,
    MessageId::OnboardWelcomeLead,
    MessageId::OnboardWelcomeSetupBlurb,
    MessageId::OnboardWelcomeSteps,
    MessageId::OnboardWelcomeStepLanguage,
    MessageId::OnboardWelcomeStepApiKey,
    MessageId::OnboardWelcomeStepTrust,
    MessageId::OnboardWelcomeStepTips,
    MessageId::OnboardWelcomeDefaults,
    MessageId::OnboardWelcomeEnter,
    MessageId::OnboardWelcomeExit,
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
    MessageId::SetupWizardTitle,
    MessageId::SetupWizardWhy,
    MessageId::SetupWizardProgress,
    MessageId::SetupActionBack,
    MessageId::SetupActionContinue,
    MessageId::SetupActionSkip,
    MessageId::SetupActionRetry,
    MessageId::SetupActionGuided,
    MessageId::SetupActionTuneGuided,
    MessageId::SetupActionModelDraft,
    MessageId::SetupActionKeepExisting,
    MessageId::SetupActionProvider,
    MessageId::SetupActionModel,
    MessageId::SetupActionMode,
    MessageId::SetupActionConfig,
    MessageId::SetupActionRuntimePreset,
    MessageId::SetupActionApplyRuntimePreset,
    MessageId::SetupActionUseBundled,
    MessageId::SetupActionDefer,
    MessageId::SetupActionCancel,
    MessageId::SetupStatusNotStarted,
    MessageId::SetupStatusRecommended,
    MessageId::SetupStatusOptional,
    MessageId::SetupStatusDeferred,
    MessageId::SetupStatusInProgress,
    MessageId::SetupStatusNeedsAction,
    MessageId::SetupStatusVerified,
    MessageId::SetupStatusSkipped,
    MessageId::SetupStatusFailed,
    MessageId::SetupStepLanguageTitle,
    MessageId::SetupStepLanguageWhy,
    MessageId::SetupStepProviderModelTitle,
    MessageId::SetupStepProviderModelWhy,
    MessageId::SetupStepTrustSandboxTitle,
    MessageId::SetupStepTrustSandboxWhy,
    MessageId::SetupStepToolsMcpTitle,
    MessageId::SetupStepToolsMcpWhy,
    MessageId::SetupStepHotbarTitle,
    MessageId::SetupStepHotbarWhy,
    MessageId::SetupStepRemoteRuntimeTitle,
    MessageId::SetupStepRemoteRuntimeWhy,
    MessageId::SetupStepConstitutionTitle,
    MessageId::SetupStepConstitutionWhy,
    MessageId::SetupStepVerificationTitle,
    MessageId::SetupStepVerificationWhy,
    MessageId::SetupCheckpointLayerOrder,
    MessageId::SetupCheckpointDoneBundled,
    MessageId::SetupCheckpointDoneGuided,
    MessageId::SetupCheckpointDoneKept,
    MessageId::SetupCheckpointDeferred,
    MessageId::SetupStepSkipped,
    MessageId::SetupStepRetryRecorded,
    MessageId::SetupConstitutionChoiceLabel,
    MessageId::SetupConstitutionSourceLabel,
    MessageId::SetupConstitutionValidityLabel,
    MessageId::SetupConstitutionPreviewLabel,
    MessageId::SetupConstitutionExistingLabel,
    MessageId::SetupConstitutionGuidedHint,
    MessageId::SetupConstitutionGuidedAnswersHint,
    MessageId::SetupConstitutionPurposeLabel,
    MessageId::SetupConstitutionAutonomyLabel,
    MessageId::SetupConstitutionEvidenceLabel,
    MessageId::SetupConstitutionCommunicationLabel,
    MessageId::SetupConstitutionPrivacyLabel,
    MessageId::SetupConstitutionPrinciplesLabel,
    MessageId::SetupCardRouteLabel,
    MessageId::SetupCardModelLabel,
    MessageId::SetupCardAuthLabel,
    MessageId::SetupCardHealthLabel,
    MessageId::SetupCardIntentLabel,
    MessageId::SetupCardApprovalLabel,
    MessageId::SetupCardShellLabel,
    MessageId::SetupCardTrustLabel,
    MessageId::SetupCardSandboxLabel,
    MessageId::SetupCardNetworkLabel,
    MessageId::SetupProviderModelReadyHint,
    MessageId::SetupProviderModelNeedsActionHint,
    MessageId::SetupProviderModelReviewed,
    MessageId::SetupProviderModelNeedsActionSaved,
    MessageId::SetupRuntimePostureBoundary,
    MessageId::SetupRuntimePostureReviewHint,
    MessageId::SetupRuntimePostureReviewed,
    MessageId::SetupRuntimePresetSelectedLabel,
    MessageId::SetupRuntimePresetDiffLabel,
    MessageId::SetupRuntimePresetAskFirstTitle,
    MessageId::SetupRuntimePresetAskFirstDescription,
    MessageId::SetupRuntimePresetNormalAgentTitle,
    MessageId::SetupRuntimePresetNormalAgentDescription,
    MessageId::SetupRuntimePresetHighTrustTitle,
    MessageId::SetupRuntimePresetHighTrustDescription,
    MessageId::SetupRuntimePresetPreviewTitle,
    MessageId::SetupRuntimePresetSafetyFloor,
    MessageId::SetupRuntimePresetApplyHint,
    MessageId::SetupRuntimePresetApplied,
    MessageId::SetupRuntimeProjectOverrideLabel,
    MessageId::SetupRuntimeProjectOverrideNone,
    MessageId::SetupReportFirstRunLabel,
    MessageId::SetupReportUpdateLabel,
    MessageId::SetupReportSourceLabel,
    MessageId::SetupReportAutonomyLabel,
    MessageId::SetupReportRuntimePostureLabel,
    MessageId::SetupReportPersisted,
    MessageId::SetupReportInherited,
    MessageId::SetupReportReady,
    MessageId::SetupReportRequired,
    MessageId::SetupReportOptional,
    MessageId::SetupReportRowsLabel,
    MessageId::SetupReportNextActionLabel,
    MessageId::SetupReportNextActionNone,
    MessageId::SetupReportNextActionConstitution,
    MessageId::SetupReportNextActionProvider,
    MessageId::SetupReportNextActionRuntime,
    MessageId::SetupReportNextActionRequired,
    MessageId::SetupReportRecorded,
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
    MessageId::ModePickerPrompt,
    MessageId::AppModeAgent,
    MessageId::AppModeAuto,
    MessageId::AppModeYolo,
    MessageId::AppModePlan,
    MessageId::AppModeAgentHint,
    MessageId::AppModeAutoHint,
    MessageId::AppModePlanHint,
    MessageId::AppModeYoloHint,
    MessageId::VimModeNormal,
    MessageId::VimModeInsert,
    MessageId::VimModeVisual,
    MessageId::ApprovalRiskReview,
    MessageId::ApprovalRiskElevated,
    MessageId::ApprovalRiskDestructive,
    MessageId::ApprovalCategorySafe,
    MessageId::ApprovalCategoryFileWrite,
    MessageId::ApprovalCategoryShell,
    MessageId::ApprovalCategoryNetwork,
    MessageId::ApprovalCategoryMcpRead,
    MessageId::ApprovalCategoryMcpAction,
    MessageId::ApprovalCategoryAgent,
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
    MessageId::ElevationTitleSandboxDenied,
    MessageId::ElevationTitleRequired,
    MessageId::ElevationFieldTool,
    MessageId::ElevationFieldCmd,
    MessageId::ElevationFieldReason,
    MessageId::ElevationImpactHeader,
    MessageId::ElevationImpactNetwork,
    MessageId::ElevationImpactWrite,
    MessageId::ElevationImpactFullAccess,
    MessageId::ElevationPromptProceed,
    MessageId::ElevationOptionNetwork,
    MessageId::ElevationOptionWrite,
    MessageId::ElevationOptionFullAccess,
    MessageId::ElevationOptionAbort,
    MessageId::ElevationOptionNetworkDesc,
    MessageId::ElevationOptionWriteDesc,
    MessageId::ElevationOptionFullAccessDesc,
    MessageId::ElevationOptionAbortDesc,
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
    MessageId::CtxInspVHint,
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
    MessageId::ToolFamilyRead,
    MessageId::ToolFamilyPatch,
    MessageId::ToolFamilyRun,
    MessageId::ToolFamilyFind,
    MessageId::ToolFamilyDelegate,
    MessageId::ToolFamilyFanout,
    MessageId::ToolFamilyRlm,
    MessageId::ToolFamilyVerify,
    MessageId::ToolFamilyThink,
    MessageId::ToolFamilyGeneric,
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
    MessageId::VoiceErrTooShort,
    MessageId::VoiceRecording,
    MessageId::VoiceProcessing,
    MessageId::VoiceTranscribed,
];

pub fn tr(locale: Locale, id: MessageId) -> Cow<'static, str> {
    rust_i18n::t!(format!("{id:?}"), locale = locale.tag())
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

    pub fn missing_message_ids(locale: Locale) -> Vec<MessageId> {
        ALL_MESSAGE_IDS
            .iter()
            .copied()
            .filter(|id| tr(locale, *id).eq(&format!("{id:?}")))
            .collect()
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
    fn mode_picker_strings_are_translated_in_non_english_locales() {
        // The picker prompt and the three mode hints are full sentences; every
        // shipped non-English locale must provide a real translation rather than
        // leaking the English string through the fallback chain.
        let sentences = [
            MessageId::ModePickerPrompt,
            MessageId::AppModeAgentHint,
            MessageId::AppModeAutoHint,
            MessageId::AppModePlanHint,
            MessageId::AppModeYoloHint,
        ];
        for locale in Locale::shipped() {
            if *locale == Locale::En {
                continue;
            }
            for id in sentences {
                let localized = tr(*locale, id);
                assert!(!localized.is_empty(), "{} empty for {id:?}", locale.tag());
                assert_ne!(
                    localized,
                    tr(Locale::En, id),
                    "{} should translate {id:?}",
                    locale.tag()
                );
            }
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
