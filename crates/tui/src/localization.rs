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
    Ko,
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
            Self::Ko => "ko",
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
            Self::Ko => "Korean (한국어)",
        }
    }

    /// Every locale the TUI exposes in pickers and runtime resolution.
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
            Self::Ko,
        ]
    }

    /// Complete UI packs held to `en.json` parity. `zh-Hant` is intentionally
    /// excluded — it remains selectable but falls back to English for missing
    /// keys until the pack catches up (#4057).
    #[allow(dead_code)]
    pub fn shipped_complete() -> &'static [Self] {
        &[
            Self::En,
            Self::Ja,
            Self::ZhHans,
            Self::PtBr,
            Self::Es419,
            Self::Vi,
            Self::Ko,
        ]
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn is_partial_pack(self) -> bool {
        matches!(self, Self::ZhHant)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageId {
    ComposerPlaceholder,
    ComposerDispatchFailedRestored,
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
    // Hotbar setup wizard chrome and validation.
    HotbarSetupTitle,
    HotbarSetupSourceApp,
    HotbarSetupSourceSlash,
    HotbarSetupSourceMcp,
    HotbarSetupSourceSkill,
    HotbarSetupSourcePlugin,
    HotbarSetupStatusDisabled,
    HotbarSetupStatusPrefill,
    HotbarSetupStatusReady,
    HotbarSetupDirtyModified,
    HotbarSetupDirtyClean,
    HotbarSetupNoAction,
    HotbarSetupStatusLine,
    HotbarSetupSlotOutOfRange,
    HotbarSetupNoActionSelected,
    HotbarSetupCannotAssign,
    HotbarSetupNoActions,
    HotbarSetupRecommended,
    HotbarSetupEmptySlot,
    HotbarSetupHelp,
    HotbarActionVoiceToggleName,
    HotbarActionVoiceToggleDescription,
    HotbarActionSessionCompactName,
    HotbarActionSessionCompactDescription,
    HotbarActionModePlanName,
    HotbarActionModePlanDescription,
    HotbarActionModeAgentName,
    HotbarActionModeAgentDescription,
    HotbarActionModeYoloName,
    HotbarActionModeYoloDescription,
    HotbarActionModeOperateName,
    HotbarActionModeOperateDescription,
    HotbarActionReasoningCycleName,
    HotbarActionReasoningCycleDescription,
    HotbarActionReasoningCycleAutoDisabled,
    HotbarActionSidebarToggleName,
    HotbarActionSidebarToggleDescription,
    HotbarActionFileTreeToggleName,
    HotbarActionFileTreeToggleDescription,
    HotbarActionPaletteOpenName,
    HotbarActionPaletteOpenDescription,
    HotbarActionTrustToggleName,
    HotbarActionTrustToggleDescription,
    CommandPaletteTitle,
    CommandPaletteSubtitle,
    ConfigTitle,
    ConfigSubtitle,
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
    ModelPickerAutoNetworkHint,
    ModelPickerAutoLocalHint,
    ModelPickerAutoLastRoute,
    AutoRouteSelectedToast,
    HelpTitle,
    HelpSubtitle,
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
    CmdAuthDescription,
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
    CmdPluginBundleUsage,
    CmdPluginBundleNoneFound,
    CmdPluginBundleListHeader,
    CmdPluginLegacyListHeader,
    CmdPluginBundleNotFound,
    CmdPluginBundleReloaded,
    CmdPluginBundleDetail,
    CmdPluginBundleDiagnosticsHeader,
    CmdPluginBundleMutationSuccess,
    CmdPluginActionFailed,
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
    CmdWorkflowDescription,
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
    HeaderAgentsChip,
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
    KbSelectText,
    KbSelectAllDraft,
    KbClearDraft,
    KbRestoreClearedDraft,
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
    KbExternalEditor,
    KbLiveTranscript,
    KbBacktrackMessage,
    KbCompleteCycleModes,
    KbCycleThinking,
    KbCyclePermissions,
    KbJumpPlanAgentYolo,
    KbAltJumpPlanAgentYolo,
    KbFocusSidebar,
    KbSessionPicker,
    KbTerminalPaste,
    KbPasteAttach,
    KbCopySelection,
    ClipboardSshPasteHint,
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
    LinksProjectTitle,
    LinksDocumentation,
    LinksCommunity,
    LinksGitHub,
    LinksManagedApp,
    LinksManagedAppNote,
    LinksTitle,
    LinksDashboard,
    LinksDocs,
    LinksKimiCodeRouteNote,
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
    HomeOperateModeTip,
    HomeOperateModeFleetTip,
    HomeGoalModeTip,
    // Onboarding screens — welcome.
    OnboardWelcomeVersion,
    OnboardWelcomeLead,
    OnboardWelcomeSetupBlurb,
    OnboardWelcomeSteps,
    OnboardWelcomeStepLanguage,
    OnboardWelcomeStepApiKey,
    OnboardWelcomeStepTrust,
    OnboardWelcomeStepMentalModels,
    OnboardWelcomeStepTips,
    OnboardWelcomeDefaults,
    OnboardWelcomeEnter,
    OnboardWelcomeExit,
    // Onboarding screens — language picker.
    OnboardLanguageTitle,
    OnboardLanguageBlurb,
    OnboardLanguageFooter,
    OnboardProviderTitle,
    OnboardProviderBlurb,
    OnboardProviderFooter,
    OnboardApiKeyTitle,
    OnboardApiKeyStep1,
    OnboardApiKeyStep2,
    OnboardApiKeyLocalHint,
    OnboardApiKeySavedHint,
    OnboardApiKeyFormatHint,
    KimiCodePlanApiKeyHint,
    KimiCodePlanRouteHint,
    KimiCodePlanNoImportHint,
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
    // Onboarding screens — product mental model primer.
    OnboardMentalTitle,
    OnboardMentalModesLabel,
    OnboardMentalPlanHint,
    OnboardMentalActHint,
    OnboardMentalOperateHint,
    OnboardMentalPermissionLabel,
    OnboardMentalCurrentLabel,
    OnboardMentalConstitution,
    OnboardMentalCycleMode,
    OnboardMentalCyclePermission,
    OnboardMentalContinue,
    OnboardMentalBack,
    // Onboarding screens — final tips screen.
    OnboardTipsTitle,
    OnboardTipsLine1,
    OnboardTipsLine2,
    OnboardTipsLine3,
    OnboardTipsLine4,
    OnboardTipsDoctorPrefix,
    OnboardTipsDoctorSuffix,
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
    SetupActionScrollBody,
    SetupActionGuided,
    SetupActionTuneGuided,
    SetupActionModelDraft,
    SetupActionFreeform,
    SetupActionKeepExisting,
    SetupActionProvider,
    SetupActionModel,
    SetupActionFleet,
    SetupActionHotbar,
    SetupActionRemote,
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
    SetupStepOperateFleetTitle,
    SetupStepOperateFleetWhy,
    SetupStepToolsMcpTitle,
    SetupStepToolsMcpWhy,
    SetupStepHotbarTitle,
    SetupStepHotbarWhy,
    SetupStepRemoteRuntimeTitle,
    SetupStepRemoteRuntimeWhy,
    SetupStepPersistenceTitle,
    SetupStepPersistenceWhy,
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
    SetupLanguageReviewed,
    SetupConstitutionChoiceLabel,
    SetupConstitutionSourceLabel,
    SetupConstitutionValidityLabel,
    SetupConstitutionPreviewLabel,
    SetupConstitutionExistingLabel,
    SetupConstitutionExpertOverrideLabel,
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
    SetupOperateRuntimeLabel,
    SetupOperateRosterLabel,
    SetupOperateConcurrencyLabel,
    SetupOperateReadinessLabel,
    SetupOperateReviewHint,
    SetupOperateReviewed,
    SetupOperateNeedsActionSaved,
    SetupHotbarBindingsLabel,
    SetupHotbarActionsLabel,
    SetupHotbarReviewHint,
    SetupHotbarReviewed,
    SetupToolsMcpServersLabel,
    SetupToolsMcpSkillsLabel,
    SetupToolsMcpToolsLabel,
    SetupToolsMcpPluginsLabel,
    SetupToolsMcpHotbarLabel,
    SetupToolsMcpReviewHint,
    SetupToolsMcpReviewed,
    SetupToolsMcpNeedsActionSaved,
    SetupToolsMcpPreviewTitle,
    SetupToolsMcpOnRampText,
    SetupRemoteCloudsLabel,
    SetupRemoteBridgesLabel,
    SetupRemoteProvidersLabel,
    SetupRemoteModeLabel,
    SetupRemoteReviewHint,
    SetupRemotePreviewTitle,
    SetupRemoteReviewed,
    SetupPersistenceHomeLabel,
    SetupPersistenceConfigLabel,
    SetupPersistenceStateLabel,
    SetupPersistenceConstitutionLabel,
    SetupPersistenceMemoryLabel,
    SetupPersistenceNotesLabel,
    SetupPersistenceReviewHint,
    SetupPersistenceReviewed,
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
    SetupReportOperateLabel,
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
    SetupReportNextActionOperate,
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

    // App mode picker (names, hints) and composer vim indicator.
    AppModeAgent,
    AppModeAuto,
    AppModeYolo,
    AppModePlan,
    AppModeOperate,
    AppModeAgentHint,
    AppModeAutoHint,
    AppModePlanHint,
    AppModeYoloHint,
    AppModeOperateHint,
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
    ApprovalAutoDeniedSession,
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
    // Notifications (turn/agent completion).
    NotificationTurnComplete,
    NotificationSubagentComplete,
    NotificationSubagentFailed,
    NotificationSubagentInterrupted,
    NotificationSubagentCancelled,
    NotificationSubagentBudgetExhausted,
    // Footer chips.
    FooterWorkedChip,
    // Fleet setup wizard.
    FleetDraftTitle,
    FleetDraftHeader,
    FleetPreviewHeader,
    // Remote setup on-ramp.
    SetupRemoteOnRampText,
    // Approval dialog — localized descriptions.
    ApprovalDescSafe,
    ApprovalDescFileWrite,
    ApprovalDescShell,
    ApprovalDescNetwork,
    ApprovalDescMcpRead,
    ApprovalDescMcpAction,
    ApprovalDescAgent,
    ApprovalDescUnknown,
    // Approval impact summaries.
    ApprovalImpactSafe,
    ApprovalImpactFileWrite,
    ApprovalImpactShell,
    ApprovalImpactNetwork,
    ApprovalImpactMcpRead,
    ApprovalImpactMcpAction,
    ApprovalImpactAgent,
    ApprovalImpactUnknown,
    // Approval detail labels.
    ApprovalLabelCommand,
    ApprovalLabelDir,
    ApprovalLabelFile,
    ApprovalLabelPreview,
    ApprovalLabelProposedContent,
    ApprovalLabelReplaceThis,
    ApprovalLabelWithThis,
    ApprovalLabelReplacementContent,
    ApprovalLabelPath,
    ApprovalLabelTarget,
    ApprovalLabelInput,
    ApprovalLabelAction,
    ApprovalLabelType,
    ApprovalLabelPrompt,
    // Approval header labels.
    ApprovalLabelAbout,
    ApprovalLabelImpact,
    // Setup wizard — constitution file state.
    SetupConstitutionFileNotChecked,
    SetupConstitutionFileMissing,
    SetupConstitutionFileLoadedSelected,
    SetupConstitutionFileLoadedInactive,
    SetupConstitutionFileLoadedUnselected,
    SetupConstitutionFileEmpty,
    SetupConstitutionFileInvalid,
    SetupConstitutionFileUnreadable,
    SetupConstitutionFilePathError,
    // Setup wizard — expert override state.
    SetupExpertOverrideNotChecked,
    SetupExpertOverrideMissing,
    SetupExpertOverrideActive,
    SetupExpertOverrideDisabled,
    SetupExpertOverrideEmpty,
    SetupExpertOverrideUnreadable,
    SetupExpertOverridePathError,
    // Setup wizard — autonomy fallback.
    SetupAutonomyUnspecified,
    // Setup wizard — purpose labels.
    SetupGuidedPurposeCoding,
    SetupGuidedPurposeResearch,
    SetupGuidedPurposeOperations,
    SetupGuidedPurposeMixed,
    // Setup wizard — purpose about descriptions.
    SetupGuidedPurposeAboutCoding,
    SetupGuidedPurposeAboutResearch,
    SetupGuidedPurposeAboutOperations,
    SetupGuidedPurposeAboutMixed,
    // Setup wizard — working style descriptions.
    SetupGuidedStyleCoding,
    SetupGuidedStyleResearch,
    SetupGuidedStyleOperations,
    SetupGuidedStyleMixed,
    // Setup wizard — evidence labels.
    SetupGuidedEvidenceAssumptions,
    SetupGuidedEvidenceTestsAndReceipts,
    SetupGuidedEvidenceReleaseReceipts,
    // Setup wizard — guided answer notes.
    SetupGuidedNotes,
    // Underwater launch screen (pre-session menu + worktree flow).
    LaunchMenuNewSession,
    LaunchMenuNewWorktree,
    LaunchMenuResumeSession,
    LaunchMenuChangelog,
    LaunchMenuQuit,
    LaunchMenuUnavailable,
    LaunchMenuSavedCount,
    LaunchWorktreePrompt,
    LaunchWorktreeNeedsGit,
    LaunchWorktreeNameLabel,
    LaunchHintMove,
    LaunchHintOpen,
    LaunchTipFlags,
    LaunchSavedSessionSingular,
    LaunchSavedSessionsPlural,
    LaunchCreatingWorktree,
    LaunchWorktreeFailed,
    LaunchNoSavedSessions,
    // Underwater shell phase words (footer status band).
    PhaseIdle,
    PhaseDraft,
    PhaseWorking,
    /// Metered verification pass (tests/checks) — distinct from `working`
    /// so checking reads differently from searching (ocean state model).
    PhaseVerifying,
    PhaseWaitingOnYou,
    PhaseDone,
    PhaseFailed,
    PhaseFinishing,
    // Underwater header chips: mode and permission words.
    ChipModeAct,
    ChipModePlan,
    ChipModeOperate,
    ChipPermissionReadOnly,
    ChipPermissionAsk,
    ChipPermissionAuto,
    ChipPermissionFullAccess,
    ChipPermissionNever,
    // Underwater footer right-hand hint words (keys stay literal in code).
    FooterHintKeys,
    FooterHintOutput,
    FooterHintContext,
    // Underwater post-launch empty state.
    EmptyStateNoGit,
    EmptyStateMcpLabel,
    EmptyStateFleetLabel,
    EmptyStateFleetSetupLabel,
    EmptyStateHelpHint,
    // Session picker surface.
    SessionsSurfaceTitle,
    SessionsPaneTitle,
    SessionsHistoryPaneTitle,
    SessionsActionResume,
    SessionsActionSearch,
    SessionsActionSort,
    SessionsActionRename,
    SessionsActionAllWorkspaces,
    SessionsActionDelete,
    SessionsActionClose,
    SessionsScopeSortHeader,
    SessionsEmptyTitle,
    SessionsEmptyHint,
    SessionsShowingAllWorkspaces,
    SessionsScopedToWorkspace,
    SessionsNewTitlePrompt,
    SessionsDeletePrompt,
    SessionsConfirmDelete,
    SessionsNewSessionTitle,
    SessionsOpenedHistory,
    SessionsSortStatus,
    SessionsSortRecent,
    SessionsSortName,
    SessionsSortSize,
    SessionsSearchPrompt,
    SessionsDeleteFailed,
    SessionsDeleted,
    SessionsNoSelection,
    SessionsTitleLength,
    SessionsOpenFailed,
    SessionsLoadFailed,
    SessionsRenameFailed,
    SessionsRenamed,
    SessionsNoResults,
    SessionsDirectoryFailed,
    SessionsPreviewFailed,
    SessionsDeleteCancelled,
    SessionsRenameCancelled,
    SessionsShowingRange,
    SessionsMessageCountCompact,
    SessionsForkCompact,
    SessionsUnknownMode,
    SessionsPreviewTitle,
    SessionsPreviewUpdated,
    SessionsPreviewMessagesModel,
    SessionsPreviewMode,
    SessionsToolCall,
    SessionsToolError,
    SessionsToolResult,
    SessionsServerTool,
    SessionsImage,
    SessionsTimeJustNow,
    SessionsTimeMinutesAgo,
    SessionsTimeHoursAgo,
    SessionsTimeDaysAgo,
    // Compact context inspector (Alt+C surface).
    CtxInspRowSystemPrompt,
    CtxInspRowMessages,
    CtxInspRowFree,
    CtxInspFreeTokensDetail,
    CtxInspDrillTitle,
    CtxInspSurfaceTitle,
    CtxInspActionSelect,
    CtxInspActionDrillDown,
    CtxInspActionClose,
    CtxInspUsedTokens,
    CtxInspAutoCompactAt,
    CtxInspRowTokens,
    // Model picker route surface.
    RouteSurfaceTitle,
    RouteBrowseCatalog,
    RouteActionType,
    RouteActionSearchAnyModel,
    RoutePanelHeader,
    RouteProviderLabel,
    RouteModelFirstAtomic,
    PickerActionMove,
    PickerActionSwitch,
    PickerActionApply,
    PickerActionCancel,
    PickerActionClear,
    PickerActionClearSearch,
    PickerActionBrowseAll,
    PickerActionCustom,
    PickerActionJump,
    PickerActionEditKey,
    PickerActionModels,
    PickerActionUnavailable,
    PickerActionSetKey,
    PickerActionConfigured,
    RouteNoModels,
    RouteNoModelMatch,
    ProviderNoMatchesTitle,
    ProviderNoMatchesHint,
    ProviderNoConfiguredTitle,
    ProviderNoConfiguredHint,
    ProviderNoCatalogModels,
    // Provider picker — informed external-credential consent.
    ProviderExternalActionRevoke,
    ProviderExternalActionChoices,
    ProviderExternalActionReuseGrok,
    ProviderExternalHintCodexReview,
    ProviderExternalHintXaiReview,
    ProviderExternalHintXaiApiKey,
    ProviderExternalDetailScope,
    ProviderExternalDormant,
    ProviderExternalOwnerPath,
    ProviderExternalPinnedPathWarning,
    ProviderExternalSemanticsRevoke,
    ProviderExternalRevoke,
    ProviderExternalChoiceTitle,
    ProviderExternalActionChoose,
    ProviderExternalChoiceIntro,
    ProviderExternalDisabledLabel,
    ProviderExternalDisabledDetail,
    ProviderExternalReadOnlyLabel,
    ProviderExternalReadOnlyDetail,
    ProviderExternalReadOnlySemantics,
    ProviderExternalManagedLabel,
    ProviderExternalManagedDetail,
    ProviderExternalConfirmTitle,
    ProviderExternalActionGrant,
    ProviderExternalOwnerLabel,
    ProviderExternalExactPathLabel,
    ProviderExternalSemanticsLabel,
    ProviderExternalRejectUnsafe,
    ProviderExternalRevokeLabel,
    ProviderExternalGrantedToast,
    ProviderExternalSaveFailedToast,
    ProviderExternalRevokedToast,
    ProviderExternalRevokeFailedToast,
    // Theme picker surface.
    ThemeSurfaceTitle,
    ThemeTreatmentOmbreUnavailable,
    ThemeTreatmentFlatActive,
    ThemeTreatmentOmbreActive,
    // Fleet roster room.
    FleetRosterHeaderLabel,
    FleetRosterTabRoster,
    FleetRosterTabSetup,
    FleetRosterWorkers,
    FleetRosterMembersCount,
    FleetRosterOperatorFirst,
    FleetRosterOperatorRow,
    FleetReadyNotice,
    /// Sticky error when Fleet profile save cannot prove collision safety.
    FleetProfileIdentityVerifyFailed,
    /// Sticky error when the drafted profile id collides with another file.
    FleetProfileIdConflict,
    /// Sticky error when the drafted profile pins an unconfigured provider.
    FleetProfileProviderUnconfigured,
    // Workflow panel.
    WorkflowStatusWaiting,
    WorkflowDebrief,
    // Sidebar work strip.
    SidebarTasksLabel,
    SidebarTodoLabel,
    SidebarStopControl,
    SidebarDestructiveArmed,
    WorkSurfaceStopConfirmHint,
    // Composer slash menu.
    ComposerSlashMenuHint,
    // Approval modal — repository law band.
    ApprovalRepoLawBadge,
    ApprovalRepoLawTitle,
    ApprovalRepoLawWarning,
    ApprovalRepoLawRuleLabel,
    // Fuzzy file picker (@ attach overlay).
    FilePickerMatchSingular,
    FilePickerMatchesPlural,
    // Quiet action-triggered product guidance.
    BehavioralTipPlanning,
    BehavioralTipBackgroundReceipt,
    BehavioralTipClearedInput,
    BehavioralTipMcpValidation,
    BehavioralTipRepeatedCommand,
}

#[allow(dead_code)]
pub const ALL_MESSAGE_IDS: &[MessageId] = &[
    MessageId::ComposerPlaceholder,
    MessageId::ComposerDispatchFailedRestored,
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
    MessageId::HotbarSetupTitle,
    MessageId::HotbarSetupSourceApp,
    MessageId::HotbarSetupSourceSlash,
    MessageId::HotbarSetupSourceMcp,
    MessageId::HotbarSetupSourceSkill,
    MessageId::HotbarSetupSourcePlugin,
    MessageId::HotbarSetupStatusDisabled,
    MessageId::HotbarSetupStatusPrefill,
    MessageId::HotbarSetupStatusReady,
    MessageId::HotbarSetupDirtyModified,
    MessageId::HotbarSetupDirtyClean,
    MessageId::HotbarSetupNoAction,
    MessageId::HotbarSetupStatusLine,
    MessageId::HotbarSetupSlotOutOfRange,
    MessageId::HotbarSetupNoActionSelected,
    MessageId::HotbarSetupCannotAssign,
    MessageId::HotbarSetupNoActions,
    MessageId::HotbarSetupRecommended,
    MessageId::HotbarSetupEmptySlot,
    MessageId::HotbarSetupHelp,
    MessageId::HotbarActionVoiceToggleName,
    MessageId::HotbarActionVoiceToggleDescription,
    MessageId::HotbarActionSessionCompactName,
    MessageId::HotbarActionSessionCompactDescription,
    MessageId::HotbarActionModePlanName,
    MessageId::HotbarActionModePlanDescription,
    MessageId::HotbarActionModeAgentName,
    MessageId::HotbarActionModeAgentDescription,
    MessageId::HotbarActionModeYoloName,
    MessageId::HotbarActionModeYoloDescription,
    MessageId::HotbarActionModeOperateName,
    MessageId::HotbarActionModeOperateDescription,
    MessageId::HotbarActionReasoningCycleName,
    MessageId::HotbarActionReasoningCycleDescription,
    MessageId::HotbarActionReasoningCycleAutoDisabled,
    MessageId::HotbarActionSidebarToggleName,
    MessageId::HotbarActionSidebarToggleDescription,
    MessageId::HotbarActionFileTreeToggleName,
    MessageId::HotbarActionFileTreeToggleDescription,
    MessageId::HotbarActionPaletteOpenName,
    MessageId::HotbarActionPaletteOpenDescription,
    MessageId::HotbarActionTrustToggleName,
    MessageId::HotbarActionTrustToggleDescription,
    MessageId::CommandPaletteTitle,
    MessageId::CommandPaletteSubtitle,
    MessageId::ConfigTitle,
    MessageId::ConfigSubtitle,
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
    MessageId::ModelPickerAutoNetworkHint,
    MessageId::ModelPickerAutoLocalHint,
    MessageId::ModelPickerAutoLastRoute,
    MessageId::AutoRouteSelectedToast,
    MessageId::HelpTitle,
    MessageId::HelpSubtitle,
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
    MessageId::CmdAuthDescription,
    MessageId::CmdConstitutionDescription,
    MessageId::CmdContextDescription,
    MessageId::CmdCostDescription,
    MessageId::CmdDiffDescription,
    MessageId::CmdEditDescription,
    MessageId::CmdExitDescription,
    MessageId::CmdExportDescription,
    MessageId::CmdFeedbackDescription,
    MessageId::CmdForkDescription,
    MessageId::CmdGoalDescription,
    MessageId::CmdThemeDescription,
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
    MessageId::CmdPluginBundleUsage,
    MessageId::CmdPluginBundleNoneFound,
    MessageId::CmdPluginBundleListHeader,
    MessageId::CmdPluginLegacyListHeader,
    MessageId::CmdPluginBundleNotFound,
    MessageId::CmdPluginBundleReloaded,
    MessageId::CmdPluginBundleDetail,
    MessageId::CmdPluginBundleDiagnosticsHeader,
    MessageId::CmdPluginBundleMutationSuccess,
    MessageId::CmdPluginActionFailed,
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
    MessageId::CmdWorkflowDescription,
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
    MessageId::HeaderAgentsChip,
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
    MessageId::KbSelectText,
    MessageId::KbSelectAllDraft,
    MessageId::KbClearDraft,
    MessageId::KbRestoreClearedDraft,
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
    MessageId::KbExternalEditor,
    MessageId::KbLiveTranscript,
    MessageId::KbBacktrackMessage,
    MessageId::KbCompleteCycleModes,
    MessageId::KbCycleThinking,
    MessageId::KbCyclePermissions,
    MessageId::KbJumpPlanAgentYolo,
    MessageId::KbAltJumpPlanAgentYolo,
    MessageId::KbFocusSidebar,
    MessageId::KbSessionPicker,
    MessageId::KbTerminalPaste,
    MessageId::KbPasteAttach,
    MessageId::KbCopySelection,
    MessageId::ClipboardSshPasteHint,
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
    MessageId::LinksProjectTitle,
    MessageId::LinksDocumentation,
    MessageId::LinksCommunity,
    MessageId::LinksGitHub,
    MessageId::LinksManagedApp,
    MessageId::LinksManagedAppNote,
    MessageId::LinksTitle,
    MessageId::LinksDashboard,
    MessageId::LinksDocs,
    MessageId::LinksKimiCodeRouteNote,
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
    MessageId::HomeOperateModeTip,
    MessageId::HomeOperateModeFleetTip,
    MessageId::HomeGoalModeTip,
    MessageId::OnboardWelcomeVersion,
    MessageId::OnboardWelcomeLead,
    MessageId::OnboardWelcomeSetupBlurb,
    MessageId::OnboardWelcomeSteps,
    MessageId::OnboardWelcomeStepLanguage,
    MessageId::OnboardWelcomeStepApiKey,
    MessageId::OnboardWelcomeStepTrust,
    MessageId::OnboardWelcomeStepMentalModels,
    MessageId::OnboardWelcomeStepTips,
    MessageId::OnboardWelcomeDefaults,
    MessageId::OnboardWelcomeEnter,
    MessageId::OnboardWelcomeExit,
    MessageId::OnboardLanguageTitle,
    MessageId::OnboardLanguageBlurb,
    MessageId::OnboardLanguageFooter,
    MessageId::OnboardProviderTitle,
    MessageId::OnboardProviderBlurb,
    MessageId::OnboardProviderFooter,
    MessageId::OnboardApiKeyTitle,
    MessageId::OnboardApiKeyStep1,
    MessageId::OnboardApiKeyStep2,
    MessageId::OnboardApiKeyLocalHint,
    MessageId::OnboardApiKeySavedHint,
    MessageId::OnboardApiKeyFormatHint,
    MessageId::KimiCodePlanApiKeyHint,
    MessageId::KimiCodePlanRouteHint,
    MessageId::KimiCodePlanNoImportHint,
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
    MessageId::OnboardMentalTitle,
    MessageId::OnboardMentalModesLabel,
    MessageId::OnboardMentalPlanHint,
    MessageId::OnboardMentalActHint,
    MessageId::OnboardMentalOperateHint,
    MessageId::OnboardMentalPermissionLabel,
    MessageId::OnboardMentalCurrentLabel,
    MessageId::OnboardMentalConstitution,
    MessageId::OnboardMentalCycleMode,
    MessageId::OnboardMentalCyclePermission,
    MessageId::OnboardMentalContinue,
    MessageId::OnboardMentalBack,
    MessageId::OnboardTipsTitle,
    MessageId::OnboardTipsLine1,
    MessageId::OnboardTipsLine2,
    MessageId::OnboardTipsLine3,
    MessageId::OnboardTipsLine4,
    MessageId::OnboardTipsDoctorPrefix,
    MessageId::OnboardTipsDoctorSuffix,
    MessageId::OnboardTipsFooterEnter,
    MessageId::OnboardTipsFooterAction,
    MessageId::SetupWizardTitle,
    MessageId::SetupWizardWhy,
    MessageId::SetupWizardProgress,
    MessageId::SetupActionBack,
    MessageId::SetupActionContinue,
    MessageId::SetupActionSkip,
    MessageId::SetupActionRetry,
    MessageId::SetupActionScrollBody,
    MessageId::SetupActionGuided,
    MessageId::SetupActionTuneGuided,
    MessageId::SetupActionModelDraft,
    MessageId::SetupActionFreeform,
    MessageId::SetupActionKeepExisting,
    MessageId::SetupActionProvider,
    MessageId::SetupActionModel,
    MessageId::SetupActionFleet,
    MessageId::SetupActionHotbar,
    MessageId::SetupActionRemote,
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
    MessageId::SetupStepOperateFleetTitle,
    MessageId::SetupStepOperateFleetWhy,
    MessageId::SetupStepToolsMcpTitle,
    MessageId::SetupStepToolsMcpWhy,
    MessageId::SetupStepHotbarTitle,
    MessageId::SetupStepHotbarWhy,
    MessageId::SetupStepRemoteRuntimeTitle,
    MessageId::SetupStepRemoteRuntimeWhy,
    MessageId::SetupStepPersistenceTitle,
    MessageId::SetupStepPersistenceWhy,
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
    MessageId::SetupLanguageReviewed,
    MessageId::SetupConstitutionChoiceLabel,
    MessageId::SetupConstitutionSourceLabel,
    MessageId::SetupConstitutionValidityLabel,
    MessageId::SetupConstitutionPreviewLabel,
    MessageId::SetupConstitutionExistingLabel,
    MessageId::SetupConstitutionExpertOverrideLabel,
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
    MessageId::SetupOperateRuntimeLabel,
    MessageId::SetupOperateRosterLabel,
    MessageId::SetupOperateConcurrencyLabel,
    MessageId::SetupOperateReadinessLabel,
    MessageId::SetupOperateReviewHint,
    MessageId::SetupOperateReviewed,
    MessageId::SetupOperateNeedsActionSaved,
    MessageId::SetupHotbarBindingsLabel,
    MessageId::SetupHotbarActionsLabel,
    MessageId::SetupHotbarReviewHint,
    MessageId::SetupHotbarReviewed,
    MessageId::SetupToolsMcpServersLabel,
    MessageId::SetupToolsMcpSkillsLabel,
    MessageId::SetupToolsMcpToolsLabel,
    MessageId::SetupToolsMcpPluginsLabel,
    MessageId::SetupToolsMcpHotbarLabel,
    MessageId::SetupToolsMcpReviewHint,
    MessageId::SetupToolsMcpReviewed,
    MessageId::SetupToolsMcpNeedsActionSaved,
    MessageId::SetupToolsMcpPreviewTitle,
    MessageId::SetupToolsMcpOnRampText,
    MessageId::SetupRemoteCloudsLabel,
    MessageId::SetupRemoteBridgesLabel,
    MessageId::SetupRemoteProvidersLabel,
    MessageId::SetupRemoteModeLabel,
    MessageId::SetupRemoteReviewHint,
    MessageId::SetupRemotePreviewTitle,
    MessageId::SetupRemoteReviewed,
    MessageId::SetupPersistenceHomeLabel,
    MessageId::SetupPersistenceConfigLabel,
    MessageId::SetupPersistenceStateLabel,
    MessageId::SetupPersistenceConstitutionLabel,
    MessageId::SetupPersistenceMemoryLabel,
    MessageId::SetupPersistenceNotesLabel,
    MessageId::SetupPersistenceReviewHint,
    MessageId::SetupPersistenceReviewed,
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
    MessageId::SetupReportOperateLabel,
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
    MessageId::SetupReportNextActionOperate,
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
    MessageId::AppModeAgent,
    MessageId::AppModeAuto,
    MessageId::AppModeYolo,
    MessageId::AppModePlan,
    MessageId::AppModeOperate,
    MessageId::AppModeAgentHint,
    MessageId::AppModeAutoHint,
    MessageId::AppModePlanHint,
    MessageId::AppModeYoloHint,
    MessageId::AppModeOperateHint,
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
    MessageId::ApprovalAutoDeniedSession,
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
    MessageId::NotificationTurnComplete,
    MessageId::NotificationSubagentComplete,
    MessageId::NotificationSubagentFailed,
    MessageId::NotificationSubagentInterrupted,
    MessageId::NotificationSubagentCancelled,
    MessageId::NotificationSubagentBudgetExhausted,
    MessageId::FooterWorkedChip,
    MessageId::FleetDraftTitle,
    MessageId::FleetDraftHeader,
    MessageId::FleetPreviewHeader,
    MessageId::SetupRemoteOnRampText,
    MessageId::ApprovalDescSafe,
    MessageId::ApprovalDescFileWrite,
    MessageId::ApprovalDescShell,
    MessageId::ApprovalDescNetwork,
    MessageId::ApprovalDescMcpRead,
    MessageId::ApprovalDescMcpAction,
    MessageId::ApprovalDescAgent,
    MessageId::ApprovalDescUnknown,
    MessageId::ApprovalImpactSafe,
    MessageId::ApprovalImpactFileWrite,
    MessageId::ApprovalImpactShell,
    MessageId::ApprovalImpactNetwork,
    MessageId::ApprovalImpactMcpRead,
    MessageId::ApprovalImpactMcpAction,
    MessageId::ApprovalImpactAgent,
    MessageId::ApprovalImpactUnknown,
    MessageId::ApprovalLabelCommand,
    MessageId::ApprovalLabelDir,
    MessageId::ApprovalLabelFile,
    MessageId::ApprovalLabelPreview,
    MessageId::ApprovalLabelProposedContent,
    MessageId::ApprovalLabelReplaceThis,
    MessageId::ApprovalLabelWithThis,
    MessageId::ApprovalLabelReplacementContent,
    MessageId::ApprovalLabelPath,
    MessageId::ApprovalLabelTarget,
    MessageId::ApprovalLabelInput,
    MessageId::ApprovalLabelAction,
    MessageId::ApprovalLabelType,
    MessageId::ApprovalLabelPrompt,
    MessageId::ApprovalLabelAbout,
    MessageId::ApprovalLabelImpact,
    MessageId::SetupConstitutionFileNotChecked,
    MessageId::SetupConstitutionFileMissing,
    MessageId::SetupConstitutionFileLoadedSelected,
    MessageId::SetupConstitutionFileLoadedInactive,
    MessageId::SetupConstitutionFileLoadedUnselected,
    MessageId::SetupConstitutionFileEmpty,
    MessageId::SetupConstitutionFileInvalid,
    MessageId::SetupConstitutionFileUnreadable,
    MessageId::SetupConstitutionFilePathError,
    MessageId::SetupExpertOverrideNotChecked,
    MessageId::SetupExpertOverrideMissing,
    MessageId::SetupExpertOverrideActive,
    MessageId::SetupExpertOverrideDisabled,
    MessageId::SetupExpertOverrideEmpty,
    MessageId::SetupExpertOverrideUnreadable,
    MessageId::SetupExpertOverridePathError,
    MessageId::SetupAutonomyUnspecified,
    MessageId::SetupGuidedPurposeCoding,
    MessageId::SetupGuidedPurposeResearch,
    MessageId::SetupGuidedPurposeOperations,
    MessageId::SetupGuidedPurposeMixed,
    MessageId::SetupGuidedPurposeAboutCoding,
    MessageId::SetupGuidedPurposeAboutResearch,
    MessageId::SetupGuidedPurposeAboutOperations,
    MessageId::SetupGuidedPurposeAboutMixed,
    MessageId::SetupGuidedStyleCoding,
    MessageId::SetupGuidedStyleResearch,
    MessageId::SetupGuidedStyleOperations,
    MessageId::SetupGuidedStyleMixed,
    MessageId::SetupGuidedEvidenceAssumptions,
    MessageId::SetupGuidedEvidenceTestsAndReceipts,
    MessageId::SetupGuidedEvidenceReleaseReceipts,
    MessageId::SetupGuidedNotes,
    MessageId::LaunchMenuNewSession,
    MessageId::LaunchMenuNewWorktree,
    MessageId::LaunchMenuResumeSession,
    MessageId::LaunchMenuChangelog,
    MessageId::LaunchMenuQuit,
    MessageId::LaunchMenuUnavailable,
    MessageId::LaunchMenuSavedCount,
    MessageId::LaunchWorktreePrompt,
    MessageId::LaunchWorktreeNeedsGit,
    MessageId::LaunchWorktreeNameLabel,
    MessageId::LaunchHintMove,
    MessageId::LaunchHintOpen,
    MessageId::LaunchTipFlags,
    MessageId::LaunchSavedSessionSingular,
    MessageId::LaunchSavedSessionsPlural,
    MessageId::LaunchCreatingWorktree,
    MessageId::LaunchWorktreeFailed,
    MessageId::LaunchNoSavedSessions,
    MessageId::PhaseIdle,
    MessageId::PhaseDraft,
    MessageId::PhaseWorking,
    MessageId::PhaseVerifying,
    MessageId::PhaseWaitingOnYou,
    MessageId::PhaseDone,
    MessageId::PhaseFailed,
    MessageId::PhaseFinishing,
    MessageId::ChipModeAct,
    MessageId::ChipModePlan,
    MessageId::ChipModeOperate,
    MessageId::ChipPermissionReadOnly,
    MessageId::ChipPermissionAsk,
    MessageId::ChipPermissionAuto,
    MessageId::ChipPermissionFullAccess,
    MessageId::ChipPermissionNever,
    MessageId::FooterHintKeys,
    MessageId::FooterHintOutput,
    MessageId::FooterHintContext,
    MessageId::EmptyStateNoGit,
    MessageId::EmptyStateMcpLabel,
    MessageId::EmptyStateFleetLabel,
    MessageId::EmptyStateFleetSetupLabel,
    MessageId::EmptyStateHelpHint,
    MessageId::SessionsSurfaceTitle,
    MessageId::SessionsPaneTitle,
    MessageId::SessionsHistoryPaneTitle,
    MessageId::SessionsActionResume,
    MessageId::SessionsActionSearch,
    MessageId::SessionsActionSort,
    MessageId::SessionsActionRename,
    MessageId::SessionsActionAllWorkspaces,
    MessageId::SessionsActionDelete,
    MessageId::SessionsActionClose,
    MessageId::SessionsScopeSortHeader,
    MessageId::SessionsEmptyTitle,
    MessageId::SessionsEmptyHint,
    MessageId::SessionsShowingAllWorkspaces,
    MessageId::SessionsScopedToWorkspace,
    MessageId::SessionsNewTitlePrompt,
    MessageId::SessionsDeletePrompt,
    MessageId::SessionsConfirmDelete,
    MessageId::SessionsNewSessionTitle,
    MessageId::SessionsOpenedHistory,
    MessageId::SessionsSortStatus,
    MessageId::SessionsSortRecent,
    MessageId::SessionsSortName,
    MessageId::SessionsSortSize,
    MessageId::SessionsSearchPrompt,
    MessageId::SessionsDeleteFailed,
    MessageId::SessionsDeleted,
    MessageId::SessionsNoSelection,
    MessageId::SessionsTitleLength,
    MessageId::SessionsOpenFailed,
    MessageId::SessionsLoadFailed,
    MessageId::SessionsRenameFailed,
    MessageId::SessionsRenamed,
    MessageId::SessionsNoResults,
    MessageId::SessionsDirectoryFailed,
    MessageId::SessionsPreviewFailed,
    MessageId::SessionsDeleteCancelled,
    MessageId::SessionsRenameCancelled,
    MessageId::SessionsShowingRange,
    MessageId::SessionsMessageCountCompact,
    MessageId::SessionsForkCompact,
    MessageId::SessionsUnknownMode,
    MessageId::SessionsPreviewTitle,
    MessageId::SessionsPreviewUpdated,
    MessageId::SessionsPreviewMessagesModel,
    MessageId::SessionsPreviewMode,
    MessageId::SessionsToolCall,
    MessageId::SessionsToolError,
    MessageId::SessionsToolResult,
    MessageId::SessionsServerTool,
    MessageId::SessionsImage,
    MessageId::SessionsTimeJustNow,
    MessageId::SessionsTimeMinutesAgo,
    MessageId::SessionsTimeHoursAgo,
    MessageId::SessionsTimeDaysAgo,
    MessageId::CtxInspRowSystemPrompt,
    MessageId::CtxInspRowMessages,
    MessageId::CtxInspRowFree,
    MessageId::CtxInspFreeTokensDetail,
    MessageId::CtxInspDrillTitle,
    MessageId::CtxInspSurfaceTitle,
    MessageId::CtxInspActionSelect,
    MessageId::CtxInspActionDrillDown,
    MessageId::CtxInspActionClose,
    MessageId::CtxInspUsedTokens,
    MessageId::CtxInspAutoCompactAt,
    MessageId::CtxInspRowTokens,
    MessageId::RouteSurfaceTitle,
    MessageId::RouteBrowseCatalog,
    MessageId::RouteActionType,
    MessageId::RouteActionSearchAnyModel,
    MessageId::RoutePanelHeader,
    MessageId::RouteProviderLabel,
    MessageId::RouteModelFirstAtomic,
    MessageId::PickerActionMove,
    MessageId::PickerActionSwitch,
    MessageId::PickerActionApply,
    MessageId::PickerActionCancel,
    MessageId::PickerActionClear,
    MessageId::PickerActionClearSearch,
    MessageId::PickerActionBrowseAll,
    MessageId::PickerActionCustom,
    MessageId::PickerActionJump,
    MessageId::PickerActionEditKey,
    MessageId::PickerActionModels,
    MessageId::PickerActionUnavailable,
    MessageId::PickerActionSetKey,
    MessageId::PickerActionConfigured,
    MessageId::RouteNoModels,
    MessageId::RouteNoModelMatch,
    MessageId::ProviderNoMatchesTitle,
    MessageId::ProviderNoMatchesHint,
    MessageId::ProviderNoConfiguredTitle,
    MessageId::ProviderNoConfiguredHint,
    MessageId::ProviderNoCatalogModels,
    MessageId::ProviderExternalActionRevoke,
    MessageId::ProviderExternalActionChoices,
    MessageId::ProviderExternalActionReuseGrok,
    MessageId::ProviderExternalHintCodexReview,
    MessageId::ProviderExternalHintXaiReview,
    MessageId::ProviderExternalHintXaiApiKey,
    MessageId::ProviderExternalDetailScope,
    MessageId::ProviderExternalDormant,
    MessageId::ProviderExternalOwnerPath,
    MessageId::ProviderExternalPinnedPathWarning,
    MessageId::ProviderExternalSemanticsRevoke,
    MessageId::ProviderExternalRevoke,
    MessageId::ProviderExternalChoiceTitle,
    MessageId::ProviderExternalActionChoose,
    MessageId::ProviderExternalChoiceIntro,
    MessageId::ProviderExternalDisabledLabel,
    MessageId::ProviderExternalDisabledDetail,
    MessageId::ProviderExternalReadOnlyLabel,
    MessageId::ProviderExternalReadOnlyDetail,
    MessageId::ProviderExternalReadOnlySemantics,
    MessageId::ProviderExternalManagedLabel,
    MessageId::ProviderExternalManagedDetail,
    MessageId::ProviderExternalConfirmTitle,
    MessageId::ProviderExternalActionGrant,
    MessageId::ProviderExternalOwnerLabel,
    MessageId::ProviderExternalExactPathLabel,
    MessageId::ProviderExternalSemanticsLabel,
    MessageId::ProviderExternalRejectUnsafe,
    MessageId::ProviderExternalRevokeLabel,
    MessageId::ProviderExternalGrantedToast,
    MessageId::ProviderExternalSaveFailedToast,
    MessageId::ProviderExternalRevokedToast,
    MessageId::ProviderExternalRevokeFailedToast,
    MessageId::ThemeSurfaceTitle,
    MessageId::ThemeTreatmentOmbreUnavailable,
    MessageId::ThemeTreatmentFlatActive,
    MessageId::ThemeTreatmentOmbreActive,
    MessageId::FleetRosterHeaderLabel,
    MessageId::FleetRosterTabRoster,
    MessageId::FleetRosterTabSetup,
    MessageId::FleetRosterWorkers,
    MessageId::FleetRosterMembersCount,
    MessageId::FleetRosterOperatorFirst,
    MessageId::FleetRosterOperatorRow,
    MessageId::FleetReadyNotice,
    MessageId::FleetProfileIdentityVerifyFailed,
    MessageId::FleetProfileIdConflict,
    MessageId::FleetProfileProviderUnconfigured,
    MessageId::WorkflowStatusWaiting,
    MessageId::WorkflowDebrief,
    MessageId::SidebarTasksLabel,
    MessageId::SidebarTodoLabel,
    MessageId::SidebarStopControl,
    MessageId::SidebarDestructiveArmed,
    MessageId::WorkSurfaceStopConfirmHint,
    MessageId::ComposerSlashMenuHint,
    MessageId::ApprovalRepoLawBadge,
    MessageId::ApprovalRepoLawTitle,
    MessageId::ApprovalRepoLawWarning,
    MessageId::ApprovalRepoLawRuleLabel,
    MessageId::FilePickerMatchSingular,
    MessageId::FilePickerMatchesPlural,
    MessageId::BehavioralTipPlanning,
    MessageId::BehavioralTipBackgroundReceipt,
    MessageId::BehavioralTipClearedInput,
    MessageId::BehavioralTipMcpValidation,
    MessageId::BehavioralTipRepeatedCommand,
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
        Locale::Ko => "생각하는 중입니다. 완료되면 번역합니다...",
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
        Locale::Ko => "생각 내용을 번역하는 중...",
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
        Locale::Ko => "생각 내용 번역 완료",
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
        Locale::Ko => "생각 내용 번역 실패",
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
        Locale::Ko => "번역에 실패했습니다. 원문은 숨겨져 있습니다.",
    }
}

pub fn normalize_configured_locale(input: &str) -> Option<&'static str> {
    let normalized = normalize_locale_input(input);
    if matches!(normalized.as_str(), "" | "auto" | "system") {
        return Some("auto");
    }
    parse_locale(&normalized).map(Locale::tag)
}

/// Human-facing list of accepted `locale` setting values, derived from the
/// shipped packs so config hints and error messages cannot go stale as new
/// locales land. `separator` is `", "` for prose and `" | "` for hints.
#[must_use]
pub fn configured_locale_values(separator: &str) -> String {
    let mut out = String::from("auto");
    for locale in Locale::shipped() {
        out.push_str(separator);
        out.push_str(locale.tag());
    }
    out
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
    if value.starts_with("ko") {
        return Some(Locale::Ko);
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

    fn locale_json_source(locale: Locale) -> &'static str {
        match locale {
            Locale::En => include_str!("../locales/en.json"),
            Locale::Ja => include_str!("../locales/ja.json"),
            Locale::ZhHans => include_str!("../locales/zh-Hans.json"),
            Locale::ZhHant => include_str!("../locales/zh-Hant.json"),
            Locale::PtBr => include_str!("../locales/pt-BR.json"),
            Locale::Es419 => include_str!("../locales/es-419.json"),
            Locale::Vi => include_str!("../locales/vi.json"),
            Locale::Ko => include_str!("../locales/ko.json"),
        }
    }

    #[test]
    fn shipped_complete_packs_have_no_missing_core_messages() {
        for locale in Locale::shipped_complete() {
            assert!(
                missing_message_ids(*locale).is_empty(),
                "{} is missing messages",
                locale.tag()
            );
        }
    }

    #[test]
    fn work_stop_confirmation_is_explicitly_localized() {
        for locale in Locale::shipped_complete() {
            if *locale == Locale::En {
                continue;
            }
            assert_ne!(tr(*locale, MessageId::SidebarStopControl), "stop");
            assert_ne!(
                tr(*locale, MessageId::WorkSurfaceStopConfirmHint),
                "confirm stop · Esc cancels"
            );
        }
    }

    fn raw_locale_keys(locale: Locale) -> std::collections::BTreeSet<String> {
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(locale_json_source(
            locale,
        ))
        .unwrap_or_else(|err| panic!("{} locale json should parse: {err}", locale.tag()))
        .keys()
        .cloned()
        .collect()
    }

    /// `missing_message_ids` is blind to keys that exist in en but not in a
    /// "complete" pack — the English fallback returns the English string, so
    /// nothing looks missing. Keep the enum, en.json, and ALL_MESSAGE_IDS in
    /// exact sync so every other parity gate actually sees every message.
    #[test]
    fn message_id_list_english_pack_stay_in_exact_sync() {
        let en = raw_locale_keys(Locale::En);
        let ids: std::collections::BTreeSet<String> =
            ALL_MESSAGE_IDS.iter().map(|id| format!("{id:?}")).collect();
        assert_eq!(
            ids.len(),
            ALL_MESSAGE_IDS.len(),
            "ALL_MESSAGE_IDS contains duplicates"
        );
        let unlisted: Vec<_> = en.difference(&ids).collect();
        assert!(
            unlisted.is_empty(),
            "en.json keys absent from ALL_MESSAGE_IDS — every parity test is blind to them: {unlisted:?}"
        );
        let untranslatable: Vec<_> = ids.difference(&en).collect();
        assert!(
            untranslatable.is_empty(),
            "ALL_MESSAGE_IDS entries without an en.json string: {untranslatable:?}"
        );
    }

    /// Raw key-set parity for every pack that claims completeness, in both
    /// directions. This is the test that fails when a new en key ships
    /// without translations instead of silently falling back to English.
    #[test]
    fn shipped_complete_packs_have_raw_key_parity_with_english() {
        let en = raw_locale_keys(Locale::En);
        for locale in Locale::shipped_complete() {
            if *locale == Locale::En {
                continue;
            }
            let pack = raw_locale_keys(*locale);
            let missing: Vec<_> = en.difference(&pack).collect();
            assert!(
                missing.is_empty(),
                "{} claims completeness but lacks {} key(s); the English fallback hides these at runtime: {missing:?}",
                locale.tag(),
                missing.len()
            );
            let extra: Vec<_> = pack.difference(&en).collect();
            assert!(
                extra.is_empty(),
                "{} defines key(s) en.json lacks: {extra:?}",
                locale.tag()
            );
        }
    }

    #[test]
    fn zh_hant_is_scoped_as_partial_pack() {
        assert!(
            Locale::ZhHant.is_partial_pack(),
            "zh-Hant must be marked partial until it reaches en.json parity"
        );
        let en_keys = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            locale_json_source(Locale::En),
        )
        .expect("en locale json");
        let zh_hant_keys = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            locale_json_source(Locale::ZhHant),
        )
        .expect("zh-Hant locale json");
        assert!(
            zh_hant_keys.len() < en_keys.len(),
            "partial zh-Hant should not claim full parity"
        );
        assert!(
            !Locale::shipped_complete().contains(&Locale::ZhHant),
            "parity gates must exclude partial zh-Hant"
        );
    }

    #[test]
    fn shipped_setup_strings_are_explicitly_localized() {
        let setup_keys = ALL_MESSAGE_IDS
            .iter()
            .map(|id| format!("{id:?}"))
            .filter(|id| id.starts_with("Setup"))
            .collect::<Vec<_>>();

        for locale in Locale::shipped_complete() {
            let messages = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
                locale_json_source(*locale),
            )
            .unwrap_or_else(|err| panic!("{} locale json should parse: {err}", locale.tag()));
            for key in &setup_keys {
                assert!(
                    messages.contains_key(key),
                    "{} should define {key} explicitly",
                    locale.tag()
                );
            }
        }
    }

    #[test]
    fn zh_hans_constitution_copy_uses_functional_terms() {
        let messages = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(
            locale_json_source(Locale::ZhHans),
        )
        .expect("zh-Hans locale json");

        for (key, value) in &messages {
            let Some(value) = value.as_str() else {
                continue;
            };
            for literal_metaphor in ["宪法", "教义", "自由原则", "仓库法则"] {
                assert!(
                    !value.contains(literal_metaphor),
                    "zh-Hans {key} should use functional terminology instead of {literal_metaphor}: {value}"
                );
            }
        }

        let setup_intro = tr(Locale::ZhHans, MessageId::SetupStepConstitutionWhy);
        assert!(setup_intro.contains("Codewhale"));
        assert!(setup_intro.contains("协作准则"));
        assert!(!setup_intro.contains("代码"));
        let welcome = tr(Locale::ZhHans, MessageId::OnboardWelcomeLead);
        assert!(welcome.contains("Codewhale"));
        assert!(!welcome.contains("代码"));
        assert!(tr(Locale::ZhHans, MessageId::OnboardTipsLine2).contains("/constitution"));
        assert!(
            tr(
                Locale::ZhHans,
                MessageId::SetupConstitutionFileLoadedUnselected
            )
            .contains("constitution.json")
        );
    }

    #[test]
    fn route_and_provider_picker_strings_are_translated_in_complete_locales() {
        // High-visibility model/provider empty states and footers must not
        // leak English through the fallback chain in complete packs.
        let ids = [
            MessageId::PickerActionMove,
            MessageId::PickerActionSwitch,
            MessageId::PickerActionApply,
            MessageId::PickerActionCancel,
            MessageId::PickerActionClear,
            MessageId::PickerActionClearSearch,
            MessageId::PickerActionBrowseAll,
            MessageId::PickerActionCustom,
            MessageId::PickerActionJump,
            MessageId::PickerActionEditKey,
            MessageId::PickerActionModels,
            MessageId::PickerActionConfigured,
            MessageId::RouteNoModels,
            MessageId::RouteNoModelMatch,
            MessageId::ProviderNoMatchesTitle,
            MessageId::ProviderNoMatchesHint,
            MessageId::ProviderNoConfiguredTitle,
            MessageId::ProviderNoConfiguredHint,
            MessageId::ProviderNoCatalogModels,
            MessageId::SessionsOpenedHistory,
            MessageId::SessionsTimeJustNow,
        ];
        for locale in Locale::shipped_complete() {
            if *locale == Locale::En {
                continue;
            }
            for id in ids {
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
    fn mode_picker_strings_are_translated_in_non_english_locales() {
        // The mode hints are full sentences; every shipped non-English locale
        // must provide a real translation rather than leaking the English
        // string through the fallback chain.
        let sentences = [
            MessageId::AppModeAgentHint,
            MessageId::AppModeAutoHint,
            MessageId::AppModePlanHint,
            MessageId::AppModeYoloHint,
            MessageId::AppModeOperateHint,
        ];
        for locale in Locale::shipped_complete() {
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
    fn zh_hant_hotbar_command_and_keybinding_strings_are_native() {
        for id in [
            MessageId::CmdHotbarDescription,
            MessageId::KbJumpPlanAgentYolo,
            MessageId::KbAltJumpPlanAgentYolo,
        ] {
            let localized = tr(Locale::ZhHant, id);
            assert!(!localized.is_empty(), "zh-Hant empty for {id:?}");
            assert_ne!(
                localized,
                tr(Locale::En, id),
                "zh-Hant should translate {id:?}"
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
    fn provider_description_is_present_for_all_locales() {
        for locale in Locale::shipped_complete() {
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

    fn visible_row_text(buf: &Buffer, area: Rect, y: u16) -> String {
        let mut out = String::new();
        let mut skip_cells = 0usize;
        for x in area.left()..area.right() {
            if skip_cells > 0 {
                skip_cells -= 1;
                continue;
            }
            let symbol = buf[(x, y)].symbol();
            out.push_str(symbol);
            skip_cells = UnicodeWidthStr::width(symbol).saturating_sub(1);
        }
        out
    }

    // --- Unicode / CJK / terminal-width QA (issue #3488) -------------------
    // `truncate_to_width` is the localization-layer truncation helper. These
    // verify it clips by display width (never byte/char count), preserves
    // semantic prefixes, never splits a grapheme cluster, and that mixed
    // English/CJK rows wrap inside a narrow (40-col) and medium (80-col)
    // terminal buffer without overflowing the column.

    #[test]
    fn truncate_to_width_clips_cjk_by_display_width_and_keeps_prefix_intact() {
        // Each Han glyph is two columns. A 12-column budget fits the six-glyph
        // title exactly, so no truncation/ellipsis happens and the prefix survives.
        let title = "项目报告结果"; // 12 columns
        assert_eq!(truncate_to_width(title, 12), title);

        // Oversized: clip on a whole-glyph boundary, append the ellipsis, and
        // stay within the budget by display width.
        let out = truncate_to_width("数据库迁移任务结果", 7); // 10 glyphs = 20 cols
        assert!(
            UnicodeWidthStr::width(out.as_str()) <= 7,
            "{out:?} overflowed"
        );
        assert!(out.ends_with('…'), "expected ellipsis, got {out:?}");
        assert!(!out.contains('\u{FFFD}'), "split a wide glyph: {out:?}");
        // The kept body is whole wide glyphs (each two columns) — never a half cell.
        let body = out.strip_suffix('…').unwrap_or(&out);
        assert!(
            body.chars()
                .map(|c| UnicodeWidthChar::width(c).unwrap_or(0))
                .sum::<usize>()
                <= 6,
            "body exceeded budget-minus-ellipsis: {out:?}"
        );

        // A semantic ASCII prefix (e.g. a status verb) survives when it fits.
        let row = "running 数据库迁移任务结果预览测试";
        let out = truncate_to_width(row, 16);
        assert!(
            out.starts_with("running"),
            "semantic prefix dropped: {out:?}"
        );
        assert!(UnicodeWidthStr::width(out.as_str()) <= 16);
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn truncate_to_width_never_splits_combining_marks_or_emoji() {
        // Combining mark (U+0301) and ZWJ are zero-width; they must not be
        // counted as columns and must never be cut mid-cluster into U+FFFD.
        let cafe = "cafe\u{0301}"; // "café", 4 columns
        assert_eq!(truncate_to_width(cafe, 10), cafe);
        let out = truncate_to_width("cafe\u{0301} overflow here", 6);
        assert!(UnicodeWidthStr::width(out.as_str()) <= 6);
        assert!(!out.contains('\u{FFFD}'));

        // Emoji is two columns; truncation lands on a cluster boundary.
        let out = truncate_to_width("\u{1F433}\u{1F433}\u{1F433} whales everywhere", 5);
        assert!(UnicodeWidthStr::width(out.as_str()) <= 5);
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn narrow_and_medium_terminal_wraps_mixed_width_rows_without_overflow() {
        // Issue #3488 acceptance: at a 40-col (narrow, macOS-Terminal-like) and
        // 80-col (medium) terminal, mixed English/CJK task titles and transcript
        // lines must (a) truncate to the column by display width, and (b) wrap
        // inside the buffer so no rendered row exceeds the terminal width.
        let fixtures = [
            "Task: 数据库迁移任务 — verify provider routing for issue #3488",
            "抹香鲸 is running codex/issue-3439-zhipu-glm-fixture @ issue-3439",
            "满員電車🫠 — full-width punctuation：『』【】 mixes with ASCII ids",
        ];

        for width in [40usize, 80] {
            // (a) The truncation helper clips by display width.
            for fixture in fixtures {
                let out = truncate_to_width(fixture, width);
                assert!(
                    UnicodeWidthStr::width(out.as_str()) <= width,
                    "width={width}: truncated row overflowed: {out:?}"
                );
                assert!(
                    !out.contains('\u{FFFD}'),
                    "width={width}: split a glyph: {out:?}"
                );
            }

            // (b) Wrapping the full mixed-width line inside a buffer of `width`
            // columns never lets a rendered row exceed the terminal width.
            for fixture in fixtures {
                let area = Rect::new(0, 0, width as u16, 6);
                let mut buf = Buffer::empty(area);
                Paragraph::new(fixture)
                    .wrap(Wrap { trim: false })
                    .render(area, &mut buf);
                let mut saw_text = false;
                for (row_idx, y) in (area.top()..area.bottom()).enumerate() {
                    let row = visible_row_text(&buf, area, y);
                    let trimmed = row.trim_end_matches('\u{0}').trim_end();
                    assert!(
                        UnicodeWidthStr::width(trimmed) <= width,
                        "width={width} row {row_idx}: wrapped row overflowed ({} cols): {trimmed:?}",
                        UnicodeWidthStr::width(trimmed)
                    );
                    saw_text |= trimmed.chars().any(|ch| !ch.is_whitespace());
                }
                assert!(
                    saw_text,
                    "width={width}: mixed fixture produced an empty render"
                );
            }
        }
    }
}
