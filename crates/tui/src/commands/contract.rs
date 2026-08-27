//! FEAT-015 TUI command-boundary surface.
//!
//! This module holds the TUI-owned pieces of the staged command migration:
//! the pending-frontier projection (D4), the seven capability facet adapters
//! (D1), boundary-value and localization-key mappings (D3/D8), the envelope
//! construction helper (D1), and the seam helpers (D7-D9). It is deliberately
//! the only new TUI module for the migration surface; the production
//! registry/dispatch stay in `traits.rs` / `mod.rs`.
//!
//! FEAT-015 does NOT migrate any production command. The adapters below wrap
//! App-owned state behind the FEAT-014 contract shapes so later FEATs
//! (FEAT-018+) can adopt them one group at a time. Handlers only ever see
//! `&mut dyn` facets — concrete `App` is never exposed through an envelope.
//!
//! ## Authoritative host-proxy design (D1)
//!
//! `CommandContexts` holds ten independently borrowed facet objects, while
//! important behavior (mode transitions, model invalidation, cost accounting,
//! skill refresh) is authoritative on `App`. The adapters therefore share a
//! synchronous TUI-owned host proxy. Each trait call borrows `App` only for the
//! duration of that call and delegates to the real operation; handlers still
//! receive only portable facets and can never name concrete TUI state.
//!
//! ## Dead-code note
//!
//! FEAT-015 intentionally wires no production contextual command. Some bridge
//! helpers remain production-dead until the first slice migrates (FEAT-018+),
//! so this transitional module keeps a bounded dead-code allow.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use codewhale_command_contract::facets::{
    CommandCostContext, CommandMediaContext, CommandMemoryContext, CommandModePolicyContext,
    CommandModelContext, CommandPluginContext, CommandPresentationContext, CommandSessionContext,
    CommandSkillsContext, CommandSystemPromptContext, CommandWorkspaceContext,
    MediaAttachmentReceipt, MemoryDelete, MemoryDeleteScope, MemoryExport, MemoryGetOutcome,
    MemoryHit, MemoryImportOutcome, MemoryReindex, MemoryRememberTarget, MemoryRemembered,
    MemoryStatus, PluginDetail, PluginDiagnostic, PluginDiagnosticLevel, PluginExportReceipt,
    PluginLegacyScan, PluginLegacyTool, PluginManagedCandidate, PluginManagedScan,
    PluginMarketplaceAddReceipt, PluginMarketplaceCandidate, PluginMarketplaceCatalog,
    PluginMarketplaceInstallPlan, PluginMarketplaceState, PluginMcpServerDetail,
    PluginMcpTransport, PluginMutationOutcome, PluginMutationReceipt, PluginSuggestion,
    PluginSummary,
};
#[cfg(test)]
use codewhale_command_contract::handler::ContextParts;
use codewhale_command_contract::handler::{CommandCapabilities, CommandContexts};
use codewhale_command_contract::types::{
    CommandApprovalMode, CommandCurrency, CommandMode, CommandProviderId, CommandReasoningEffort,
};
use codewhale_config::AppMode;
use codewhale_core::request::{Message, SystemPrompt};
use codewhale_execpolicy::ApprovalMode;

use crate::commands::groups::plugins::{plugin_network_policy, run_async};
use crate::localization::{MessageId, tr};
use crate::pricing::CostCurrency;
use crate::tui::app::{App, ReasoningEffort};

// ---------------------------------------------------------------------------
// Pending frontier projection (D4)
// ---------------------------------------------------------------------------

/// Sorted, unique frontier of command groups that still use concrete-`App`
/// handlers. This is the TUI-visible projection of the checked-in migration
/// topology (`scripts/command-migration-topology.json`); the CI gate performs
/// the authoritative bidirectional source scan against that artifact.
///
/// Not referenced by production dispatch code — the fail-closed Python gate
/// (`scripts/check-command-migration-manifest.py`) reads this exact
/// declaration by source regex and the Rust frontier tests assert it.
#[allow(dead_code)]
pub(crate) const PENDING_GROUPS: &[&str] =
    &["config", "core", "debug", "project", "session", "skills"];

// ---------------------------------------------------------------------------
// Boundary-value mappings (D8)
// ---------------------------------------------------------------------------

/// Map the TUI operating mode onto the portable command boundary value.
pub(crate) fn to_command_mode(mode: AppMode) -> CommandMode {
    match mode {
        AppMode::Agent => CommandMode::Agent,
        AppMode::Auto => CommandMode::Auto,
        AppMode::Yolo => CommandMode::Yolo,
        AppMode::Plan => CommandMode::Plan,
        AppMode::Operate => CommandMode::Operate,
    }
}

fn from_command_mode(mode: CommandMode) -> AppMode {
    match mode {
        CommandMode::Agent => AppMode::Agent,
        CommandMode::Auto => AppMode::Auto,
        CommandMode::Yolo => AppMode::Yolo,
        CommandMode::Plan => AppMode::Plan,
        CommandMode::Operate => AppMode::Operate,
    }
}

/// Map the TUI approval posture onto the portable command boundary value.
pub(crate) fn to_command_approval(mode: ApprovalMode) -> CommandApprovalMode {
    match mode {
        ApprovalMode::Auto => CommandApprovalMode::Auto,
        ApprovalMode::Bypass => CommandApprovalMode::Bypass,
        ApprovalMode::Suggest => CommandApprovalMode::Suggest,
        ApprovalMode::Never => CommandApprovalMode::Never,
    }
}

/// Map the TUI reasoning-effort tier onto the portable command boundary value.
pub(crate) fn to_command_effort(effort: ReasoningEffort) -> CommandReasoningEffort {
    match effort {
        ReasoningEffort::Off => CommandReasoningEffort::Off,
        ReasoningEffort::Minimal => CommandReasoningEffort::Minimal,
        ReasoningEffort::Low => CommandReasoningEffort::Low,
        ReasoningEffort::Medium => CommandReasoningEffort::Medium,
        ReasoningEffort::High => CommandReasoningEffort::High,
        ReasoningEffort::XHigh => CommandReasoningEffort::XHigh,
        ReasoningEffort::Ultra => CommandReasoningEffort::Ultra,
        ReasoningEffort::Auto => CommandReasoningEffort::Auto,
        ReasoningEffort::Max => CommandReasoningEffort::Max,
    }
}

/// Map the TUI cost-display currency onto the portable command boundary value.
pub(crate) fn to_command_currency(currency: CostCurrency) -> CommandCurrency {
    match currency {
        CostCurrency::Usd => CommandCurrency::Usd,
        CostCurrency::Cny => CommandCurrency::Cny,
    }
}

fn from_command_currency(currency: CommandCurrency) -> CostCurrency {
    match currency {
        CommandCurrency::Usd => CostCurrency::Usd,
        CommandCurrency::Cny => CostCurrency::Cny,
    }
}

/// Stable provider identity text at the command boundary.
///
/// The TUI persists either the canonical `ApiProvider::as_str()` spelling or —
/// for named custom providers — the exact configured identity text. This
/// function never leaks URLs, credentials, or filesystem paths.
pub(crate) fn to_provider_id(identity: &str) -> CommandProviderId {
    CommandProviderId(identity.to_string())
}

/// Bridge a portable metadata description key onto the TUI localization id.
///
/// The key convention (D3) is mechanical: the contract key equals the
/// snake_case of the [`MessageId`] variant name. The match table is the
/// authoritative bridge; unknown keys fail deterministically.
pub(crate) fn key_to_message_id(key: &'static str) -> Option<MessageId> {
    Some(match key {
        "cmd_advisor_description" => MessageId::CmdAdvisorDescription,
        "cmd_agent_description" => MessageId::CmdAgentDescription,
        "cmd_anchor_description" => MessageId::CmdAnchorDescription,
        "cmd_attach_description" => MessageId::CmdAttachDescription,
        "cmd_auto_description" => MessageId::CmdAutoDescription,
        "cmd_auth_description" => MessageId::CmdAuthDescription,
        "cmd_automation_description" => MessageId::CmdAutomationDescription,
        "cmd_loop_description" => MessageId::CmdLoopDescription,
        "loop_usage" => MessageId::LoopUsage,
        "cmd_balance_description" => MessageId::CmdBalanceDescription,
        "cmd_branch_description" => MessageId::CmdBranchDescription,
        "cmd_cache_description" => MessageId::CmdCacheDescription,
        "cmd_change_description" => MessageId::CmdChangeDescription,
        "cmd_clear_description" => MessageId::CmdClearDescription,
        "cmd_compact_description" => MessageId::CmdCompactDescription,
        "cmd_config_description" => MessageId::CmdConfigDescription,
        "cmd_constitution_description" => MessageId::CmdConstitutionDescription,
        "cmd_context_description" => MessageId::CmdContextDescription,
        "cmd_cost_description" => MessageId::CmdCostDescription,
        "cmd_diff_description" => MessageId::CmdDiffDescription,
        "cmd_edit_description" => MessageId::CmdEditDescription,
        "cmd_effort_description" => MessageId::CmdEffortDescription,
        "cmd_exit_description" => MessageId::CmdExitDescription,
        "cmd_export_description" => MessageId::CmdExportDescription,
        "cmd_feedback_description" => MessageId::CmdFeedbackDescription,
        "cmd_fleet_description" => MessageId::CmdFleetDescription,
        "cmd_fork_description" => MessageId::CmdForkDescription,
        "cmd_goal_description" => MessageId::CmdGoalDescription,
        "cmd_help_description" => MessageId::CmdHelpDescription,
        "cmd_hf_description" => MessageId::CmdHfDescription,
        "cmd_home_description" => MessageId::CmdHomeDescription,
        "cmd_hooks_description" => MessageId::CmdHooksDescription,
        "cmd_hotbar_description" => MessageId::CmdHotbarDescription,
        "cmd_init_description" => MessageId::CmdInitDescription,
        "cmd_jobs_description" => MessageId::CmdJobsDescription,
        "cmd_lane_description" => MessageId::CmdLaneDescription,
        "cmd_links_description" => MessageId::CmdLinksDescription,
        "cmd_load_description" => MessageId::CmdLoadDescription,
        "cmd_logout_description" => MessageId::CmdLogoutDescription,
        "cmd_lsp_description" => MessageId::CmdLspDescription,
        "cmd_mcp_description" => MessageId::CmdMcpDescription,
        "cmd_memory_description" => MessageId::CmdMemoryDescription,
        "cmd_mode_description" => MessageId::CmdModeDescription,
        "cmd_model_db_description" => MessageId::CmdModelDbDescription,
        "cmd_model_description" => MessageId::CmdModelDescription,
        "cmd_models_description" => MessageId::CmdModelsDescription,
        "cmd_network_description" => MessageId::CmdNetworkDescription,
        "cmd_new_description" => MessageId::CmdNewDescription,
        "cmd_note_description" => MessageId::CmdNoteDescription,
        "cmd_permissions_description" => MessageId::CmdPermissionsDescription,
        "cmd_pin_description" => MessageId::CmdPinDescription,
        "cmd_plugin_description" => MessageId::CmdPluginDescription,
        "cmd_plugin_detail_description" => MessageId::CmdPluginDetailDescription,
        "cmd_preview_request_description" => MessageId::CmdPreviewRequestDescription,
        "cmd_profile_description" => MessageId::CmdProfileDescription,
        "cmd_provider_description" => MessageId::CmdProviderDescription,
        "cmd_purge_description" => MessageId::CmdPurgeDescription,
        "cmd_queue_description" => MessageId::CmdQueueDescription,
        "cmd_relay_description" => MessageId::CmdRelayDescription,
        "cmd_relaunch_description" => MessageId::CmdRelaunchDescription,
        "cmd_remote_control_description" => MessageId::CmdRemoteControlDescription,
        "cmd_remote_env_description" => MessageId::CmdRemoteEnvDescription,
        "cmd_rename_description" => MessageId::CmdRenameDescription,
        "cmd_restore_description" => MessageId::CmdRestoreDescription,
        "cmd_resume_description" => MessageId::CmdResumeDescription,
        "cmd_retry_description" => MessageId::CmdRetryDescription,
        "cmd_review_description" => MessageId::CmdReviewDescription,
        "cmd_rlm_description" => MessageId::CmdRlmDescription,
        "cmd_save_description" => MessageId::CmdSaveDescription,
        "cmd_sessions_description" => MessageId::CmdSessionsDescription,
        "cmd_settings_description" => MessageId::CmdSettingsDescription,
        "cmd_setup_description" => MessageId::CmdSetupDescription,
        "cmd_share_description" => MessageId::CmdShareDescription,
        "cmd_sidebar_description" => MessageId::CmdSidebarDescription,
        "cmd_skill_description" => MessageId::CmdSkillDescription,
        "cmd_skills_description" => MessageId::CmdSkillsDescription,
        "cmd_stash_description" => MessageId::CmdStashDescription,
        "cmd_status_description" => MessageId::CmdStatusDescription,
        "cmd_statusline_description" => MessageId::CmdStatuslineDescription,
        "cmd_structcopy_description" => MessageId::CmdStructcopyDescription,
        "cmd_subagents_description" => MessageId::CmdSubagentsDescription,
        "cmd_system_description" => MessageId::CmdSystemDescription,
        "cmd_task_description" => MessageId::CmdTaskDescription,
        "cmd_theme_description" => MessageId::CmdThemeDescription,
        "cmd_title_description" => MessageId::CmdTitleDescription,
        "cmd_tokens_description" => MessageId::CmdTokensDescription,
        "cmd_tools_description" => MessageId::CmdToolsDescription,
        "cmd_translate_description" => MessageId::CmdTranslateDescription,
        "cmd_tree_description" => MessageId::CmdTreeDescription,
        "cmd_trust_description" => MessageId::CmdTrustDescription,
        "cmd_turn_inspect_description" => MessageId::CmdTurnInspectDescription,
        "cmd_undo_description" => MessageId::CmdUndoDescription,
        "cmd_update_description" => MessageId::CmdUpdateDescription,
        "cmd_verbose_description" => MessageId::CmdVerboseDescription,
        "cmd_voice_control_description" => MessageId::CmdVoiceControlDescription,
        "cmd_voice_description" => MessageId::CmdVoiceDescription,
        "cmd_voice_send_description" => MessageId::CmdVoiceSendDescription,
        "cmd_workflow_description" => MessageId::CmdWorkflowDescription,
        "cmd_workflows_description" => MessageId::CmdWorkflowsDescription,
        "cmd_workspace_description" => MessageId::CmdWorkspaceDescription,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Capability facet adapters (D1)
// ---------------------------------------------------------------------------

/// Shared TUI host hidden behind the portable command facets.
///
/// The envelope needs ten independently borrowed facet objects, while the
/// authoritative mutation methods live on `App`. Each adapter therefore owns
/// an `Rc` clone of this synchronous host proxy. Trait calls borrow `App` only
/// for the duration of one method, delegate to the real TUI authority, and
/// return owned values. Command handlers never receive or name `App`.
struct CommandHost<'a> {
    app: RefCell<&'a mut App>,
}

type SharedCommandHost<'a> = Rc<CommandHost<'a>>;

/// Session identity, messages, queue operations, and token totals.
pub(crate) struct SessionAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandSessionContext for SessionAdapter<'_> {
    fn session_id(&self) -> Option<String> {
        self.host.app.borrow().current_session_id.clone()
    }

    fn api_messages(&self) -> Vec<Message> {
        self.host.app.borrow().api_messages.clone()
    }

    fn add_message(&mut self, message: Message) {
        self.host.app.borrow_mut().api_messages.push(message);
    }

    fn queued_message_count(&self) -> usize {
        self.host.app.borrow().queued_message_count()
    }

    fn remove_queued_message(&mut self, index: usize) -> Result<(), String> {
        self.host
            .app
            .borrow_mut()
            .remove_queued_message(index)
            .map(|_| ())
            .ok_or_else(|| format!("queued message index {index} out of bounds"))
    }

    fn total_tokens(&self) -> u64 {
        u64::from(self.host.app.borrow().session.total_tokens)
    }
}

/// Model selection, provider identity, effort, and fallback chain.
pub(crate) struct ModelAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandModelContext for ModelAdapter<'_> {
    fn current_model(&self) -> String {
        self.host.app.borrow().model.clone()
    }

    fn auto_model(&self) -> bool {
        self.host.app.borrow().auto_model
    }

    fn set_model_selection(&mut self, model: String, provider: Option<CommandProviderId>) {
        let mut app = self.host.app.borrow_mut();
        if let Some(provider) = provider {
            let identity = provider.0;
            let provider = crate::config::ApiProvider::parse(&identity)
                .unwrap_or(crate::config::ApiProvider::Custom);
            app.set_provider_identity(provider, identity);
        }
        app.set_model_selection(model);
    }

    fn reasoning_effort(&self) -> CommandReasoningEffort {
        to_command_effort(self.host.app.borrow().reasoning_effort)
    }

    fn provider_identity(&self) -> Option<CommandProviderId> {
        let app = self.host.app.borrow();
        let identity = app.provider_identity_for_persistence();
        (!identity.trim().is_empty()).then(|| to_provider_id(identity))
    }

    fn fallback_chain(&self) -> Vec<CommandProviderId> {
        self.host
            .app
            .borrow()
            .fallback_chain_entries()
            .into_iter()
            .map(|(_, provider, _)| to_provider_id(provider.as_str()))
            .collect()
    }
}

/// Cost display and accounting operations delegated to App's cost authority.
pub(crate) struct CostAdapter<'a> {
    host: SharedCommandHost<'a>,
}

fn command_cost_estimate(amount: f64, currency: CommandCurrency) -> crate::pricing::CostEstimate {
    match currency {
        CommandCurrency::Usd => crate::pricing::CostEstimate {
            usd: amount,
            cny: 0.0,
        },
        CommandCurrency::Cny => crate::pricing::CostEstimate {
            usd: 0.0,
            cny: amount,
        },
    }
}

impl CommandCostContext for CostAdapter<'_> {
    fn display_currency(&self) -> CommandCurrency {
        let app = self.host.app.borrow();
        to_command_currency(app.cost_display_currency(app.cost_currency))
    }

    fn session_cost_for_currency(&self, currency: CommandCurrency) -> f64 {
        self.host
            .app
            .borrow()
            .session_cost_for_currency(from_command_currency(currency))
    }

    fn subagent_cost_for_currency(&self, currency: CommandCurrency) -> f64 {
        self.host
            .app
            .borrow()
            .subagent_cost_for_currency(from_command_currency(currency))
    }

    fn accrue_cost_estimate(&mut self, amount: f64, currency: CommandCurrency) {
        self.host
            .app
            .borrow_mut()
            .accrue_session_cost_estimate(command_cost_estimate(amount, currency));
    }

    fn record_turn_cost(
        &mut self,
        amount: f64,
        currency: CommandCurrency,
        route_receipt: Option<String>,
    ) {
        let mut app = self.host.app.borrow_mut();
        app.accrue_session_cost_estimate(command_cost_estimate(amount, currency));
        if let Some(receipt) = route_receipt {
            app.record_turn_cost_route_receipt(receipt);
        }
    }
}

/// Operating mode, approval posture, shell access, and policy lock.
pub(crate) struct ModePolicyAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandModePolicyContext for ModePolicyAdapter<'_> {
    fn mode(&self) -> CommandMode {
        to_command_mode(self.host.app.borrow().mode)
    }

    fn set_mode(&mut self, mode: CommandMode) {
        self.host.app.borrow_mut().set_mode(from_command_mode(mode));
    }

    fn approval_mode(&self) -> CommandApprovalMode {
        to_command_approval(self.host.app.borrow().approval_mode)
    }

    fn allow_shell(&self) -> bool {
        self.host.app.borrow().allow_shell
    }

    fn set_shell_access(&mut self, allow: bool) {
        self.host.app.borrow_mut().set_agent_shell_access(allow);
    }

    fn policy_locked(&self) -> bool {
        self.host.app.borrow().approval_policy_locked()
    }
}

/// Read access to the effective system prompt.
pub(crate) struct SystemPromptAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandSystemPromptContext for SystemPromptAdapter<'_> {
    fn system_prompt(&self) -> Option<SystemPrompt> {
        self.host.app.borrow().system_prompt.clone()
    }
}

/// Active skill identity and authoritative skill-cache refresh.
pub(crate) struct SkillsAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandSkillsContext for SkillsAdapter<'_> {
    fn active_skill(&self) -> Option<String> {
        self.host.app.borrow().active_skill.clone()
    }

    fn active_skill_provenance(&self) -> Option<String> {
        self.host
            .app
            .borrow()
            .active_skill_provenance
            .as_ref()
            .map(|authority| authority.plugin_name.clone())
    }

    fn refresh_skill_cache(&mut self) {
        self.host.app.borrow_mut().refresh_skill_cache();
    }
}

/// Workspace path and bounded serialized work-state snapshot.
pub(crate) struct WorkspaceAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandWorkspaceContext for WorkspaceAdapter<'_> {
    fn workspace(&self) -> PathBuf {
        self.host.app.borrow().workspace.clone()
    }

    fn work_state_snapshot(&self) -> Result<Option<String>, String> {
        self.host.app.borrow().work_state_snapshot().map(|state| {
            state.and_then(|state| crate::todo_snapshot::todo_snapshot_body(&state.todos))
        })
    }

    fn operation_digest(&mut self) -> Result<String, String> {
        let app = self.host.app.borrow();
        let Some(work) = app.runtime_services.work.as_ref() else {
            return Ok("No active operations or to-do items.".to_string());
        };
        match work.capture(app.current_session_id.as_deref()) {
            Ok(snapshot) => Ok(crate::work_graph::format_operation_digest(
                snapshot.as_ref(),
            )),
            Err(error) => Err(format!(
                "Operation digest is temporarily unavailable: {error}"
            )),
        }
    }
}

/// Stable-key translation adapter (FEAT-018 D3).
///
/// Maps stable snake_case utility message keys to the current catalog and
/// preserves the existing English fallback for intentionally incomplete locale
/// packs. Unknown keys and invalid replacement contracts fail safely; a raw
/// lookup key is never exposed.
pub(crate) struct PresentationAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandPresentationContext for PresentationAdapter<'_> {
    fn translate(&self, key: &str, replacements: &[(&str, &str)]) -> Result<String, String> {
        let message_id = key_to_utility_message_id(key)
            .or_else(|| key_to_plugin_message_id(key))
            .ok_or_else(|| "unknown translation key".to_string())?;
        let locale = self.host.app.borrow().ui_locale;
        let template = tr(locale, message_id);
        apply_named_replacements(&template, replacements)
            .ok_or_else(|| "invalid translation replacement contract".to_string())
    }
}

/// Resolve a stable plugin message key to the current catalog id (FEAT-020 D5).
///
/// Every plugin-group catalog message uses a stable snake_case key; the TUI
/// adapter maps it to the current `MessageId` value and preserves the
/// authoritative English fallback. Unknown keys fail safely.
pub(crate) fn key_to_plugin_message_id(key: &str) -> Option<MessageId> {
    Some(match key {
        "cmd_plugin_action_failed" => MessageId::CmdPluginActionFailed,
        "cmd_plugin_bundle_detail" => MessageId::CmdPluginBundleDetail,
        "cmd_plugin_bundle_diagnostics_header" => MessageId::CmdPluginBundleDiagnosticsHeader,
        "cmd_plugin_bundle_list_header" => MessageId::CmdPluginBundleListHeader,
        "cmd_plugin_bundle_mutation_success" => MessageId::CmdPluginBundleMutationSuccess,
        "cmd_plugin_bundle_none_found" => MessageId::CmdPluginBundleNoneFound,
        "cmd_plugin_bundle_not_found" => MessageId::CmdPluginBundleNotFound,
        "cmd_plugin_bundle_reloaded" => MessageId::CmdPluginBundleReloaded,
        "cmd_plugin_bundle_usage" => MessageId::CmdPluginBundleUsage,
        "cmd_plugin_detail_description" => MessageId::CmdPluginDetailDescription,
        "cmd_plugin_detail_approval" => MessageId::CmdPluginDetailApproval,
        "cmd_plugin_detail_path" => MessageId::CmdPluginDetailPath,
        "cmd_plugin_detail_schema" => MessageId::CmdPluginDetailSchema,
        "cmd_plugin_legacy_list_header" => MessageId::CmdPluginLegacyListHeader,
        "cmd_plugin_none_found" => MessageId::CmdPluginNoneFound,
        "cmd_plugin_not_found" => MessageId::CmdPluginNotFound,
        "plugin_kimi_applicable" => MessageId::PluginKimiApplicable,
        "plugin_kimi_candidate_changed" => MessageId::PluginKimiCandidateChanged,
        "plugin_kimi_candidate_details" => MessageId::PluginKimiCandidateDetails,
        "plugin_kimi_candidate_missing" => MessageId::PluginKimiCandidateMissing,
        "plugin_kimi_candidate_summary" => MessageId::PluginKimiCandidateSummary,
        "plugin_kimi_directory_name_mismatch" => MessageId::PluginKimiDirectoryNameMismatch,
        "plugin_kimi_entry_canonicalize_failed" => MessageId::PluginKimiEntryCanonicalizeFailed,
        "plugin_kimi_entry_inspect_failed" => MessageId::PluginKimiEntryInspectFailed,
        "plugin_kimi_entry_limit" => MessageId::PluginKimiEntryLimit,
        "plugin_kimi_entry_links_refused" => MessageId::PluginKimiEntryLinksRefused,
        "plugin_kimi_entry_outside_root" => MessageId::PluginKimiEntryOutsideRoot,
        "plugin_kimi_entry_read_failed" => MessageId::PluginKimiEntryReadFailed,
        "plugin_kimi_hash_unavailable" => MessageId::PluginKimiHashUnavailable,
        "plugin_kimi_home_missing" => MessageId::PluginKimiHomeMissing,
        "plugin_kimi_inspection_footer" => MessageId::PluginKimiInspectionFooter,
        "plugin_kimi_license_unspecified" => MessageId::PluginKimiLicenseUnspecified,
        "plugin_kimi_managed_root_heading" => MessageId::PluginKimiManagedRootHeading,
        "plugin_kimi_manifest_invalid" => MessageId::PluginKimiManifestInvalid,
        "plugin_kimi_manifest_must_be_file" => MessageId::PluginKimiManifestMustBeFile,
        "plugin_kimi_manifest_unreadable" => MessageId::PluginKimiManifestUnreadable,
        "plugin_kimi_marketplace_gzip_tarball" => MessageId::PluginKimiMarketplaceGzipTarball,
        "kimi_zip_unsupported" => MessageId::PluginKimiMarketplaceZipUnsupported,
        "kimi_remote_archive_unsupported" => MessageId::PluginKimiMarketplaceRemoteUnsupported,
        "kimi_gzip_tarball_url" => MessageId::PluginKimiMarketplaceGzipTarball,
        "plugin_kimi_marketplace_remote_unsupported" => {
            MessageId::PluginKimiMarketplaceRemoteUnsupported
        }
        "plugin_kimi_marketplace_zip_unsupported" => MessageId::PluginKimiMarketplaceZipUnsupported,
        "plugin_kimi_mismatch_removed" => MessageId::PluginKimiMismatchRemoved,
        "plugin_kimi_mismatch_rollback_failed" => MessageId::PluginKimiMismatchRollbackFailed,
        "plugin_kimi_none_found" => MessageId::PluginKimiNoneFound,
        "plugin_kimi_not_applicable" => MessageId::PluginKimiNotApplicable,
        "plugin_kimi_rejected_heading" => MessageId::PluginKimiRejectedHeading,
        "plugin_kimi_rollback_destination_missing" => {
            MessageId::PluginKimiRollbackDestinationMissing
        }
        "plugin_kimi_root_canonicalize_failed" => MessageId::PluginKimiRootCanonicalizeFailed,
        "plugin_kimi_root_inspect_failed" => MessageId::PluginKimiRootInspectFailed,
        "plugin_kimi_root_list_failed" => MessageId::PluginKimiRootListFailed,
        "plugin_kimi_root_must_be_directory" => MessageId::PluginKimiRootMustBeDirectory,
        "plugin_kimi_usage" => MessageId::PluginKimiUsage,
        "plugin_kimi_user_plugin_directory" => MessageId::PluginKimiUserPluginDirectory,
        _ => return None,
    })
}

/// Resolve a stable utility message key to the current catalog id.
fn key_to_utility_message_id(key: &str) -> Option<MessageId> {
    Some(match key {
        "automation_usage" => MessageId::AutomationUsage,
        "mcp_recommended_unknown_id" => MessageId::McpRecommendedUnknownId,
        "mcp_recommendations_heading" => MessageId::McpRecommendationsHeading,
        "mcp_recommendations_safety" => MessageId::McpRecommendationsSafety,
        "mcp_recommendation_github" => MessageId::McpRecommendationGithub,
        "mcp_recommendation_chrome" => MessageId::McpRecommendationChrome,
        "mcp_recommendation_playwright" => MessageId::McpRecommendationPlaywright,
        "mcp_recommendation_cua" => MessageId::McpRecommendationCua,
        "mcp_recommendation_container_use" => MessageId::McpRecommendationContainerUse,
        _ => return None,
    })
}

/// Replace `{name}` placeholders with the supplied named values.
///
/// Returns `None` when the replacement set does not exactly cover every
/// placeholder in the template (missing, extra, or duplicate names).
fn apply_named_replacements(template: &str, replacements: &[(&str, &str)]) -> Option<String> {
    let supplied: std::collections::BTreeMap<&str, &str> = replacements.iter().copied().collect();
    if supplied.len() != replacements.len() {
        return None; // duplicate replacement name
    }
    let mut placeholders = std::collections::BTreeSet::new();
    let mut cursor = 0usize;
    while let Some(start) = template[cursor..].find('{') {
        let start = cursor + start;
        let Some(end) = template[start + 1..].find('}') else {
            break;
        };
        let end = start + 1 + end;
        let name = &template[start + 1..end];
        if !name.is_empty() {
            placeholders.insert(name);
        }
        cursor = end + 1;
    }
    if placeholders != supplied.keys().copied().collect() {
        return None;
    }
    let mut out = template.to_string();
    for (name, value) in replacements {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    Some(out)
}

/// Atomic composer/media adapter (FEAT-018 D4).
///
/// Performs media validation and composer insertion as one host operation by
/// delegating to the authoritative image-validation and attachment behavior.
pub(crate) struct MediaAdapter<'a> {
    host: SharedCommandHost<'a>,
}

impl CommandMediaContext for MediaAdapter<'_> {
    fn attach_media(&mut self, resolved_path: &Path) -> Result<MediaAttachmentReceipt, String> {
        let Ok(path) = resolved_path.canonicalize() else {
            return Err(format!("Attachment not found: {}", resolved_path.display()));
        };
        if !path.is_file() {
            return Err(format!("Attachment is not a file: {}", path.display()));
        }
        let Some(kind) = media_kind(&path) else {
            return Err(
                "Unsupported attachment type. /attach is for image/video paths; use @path for \
                 text files or directories."
                    .to_string(),
            );
        };
        if kind == "image"
            && let Err(error) = crate::image_attach::attach_image_from_path(&path)
        {
            return Err(error.to_string());
        }
        let mut app = self.host.app.borrow_mut();
        app.insert_media_attachment(kind, &path, None);
        Ok(MediaAttachmentReceipt {
            kind: kind.to_string(),
            path,
        })
    }
}

/// Classify a media path by extension (image or video).
fn media_kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ppm" => Some("image"),
        "mp4" | "mov" | "m4v" | "webm" | "avi" | "mkv" => Some("video"),
        _ => None,
    }
}

/// Memory host-data adapter (FEAT-019 D1).
///
/// Derives the authoritative native store exactly like the legacy `/memory`
/// handler (`from_global_path` on the app memory path, falling back to a
/// `memory` root beside it) and converts every host value/error to a portable
/// contract value before it crosses the boundary. All methods are `&self` and
/// borrow `App` only for the duration of one call; workspace state is passed
/// per call and never retained by the facet (D8).
pub(crate) struct MemoryAdapter<'a> {
    host: SharedCommandHost<'a>,
}

/// Derive the authoritative native-memory store from the resolved user-memory
/// file path, mirroring the pre-migration `/memory` handler exactly.
fn native_store_from_memory_path(memory_path: &Path) -> crate::native_memory::NativeMemoryStore {
    if let Some(store) = crate::native_memory::NativeMemoryStore::from_global_path(memory_path) {
        return store;
    }
    let root = memory_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("memory");
    crate::native_memory::NativeMemoryStore::new(root)
}

/// Convert a TUI-owned native hit into the portable contract hit. Only the
/// semantic fields the handler consumes for rendering cross the boundary (D2).
fn portable_hit(hit: crate::native_memory::MemoryHit) -> MemoryHit {
    MemoryHit {
        source: hit.source,
        line_start: hit.line_start,
        line_end: hit.line_end,
        text: hit.text,
    }
}

impl CommandMemoryContext for MemoryAdapter<'_> {
    fn memory_path(&self) -> PathBuf {
        self.host.app.borrow().memory_path.clone()
    }

    fn memory_enabled(&self) -> bool {
        self.host.app.borrow().use_memory
    }

    fn status(&self) -> Result<MemoryStatus, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        Ok(MemoryStatus {
            root: store.root().to_path_buf(),
            source: store.global_path(),
            index: store.index_path(),
        })
    }

    fn path(&self) -> Result<PathBuf, String> {
        let app = self.host.app.borrow();
        Ok(native_store_from_memory_path(&app.memory_path)
            .root()
            .to_path_buf())
    }

    fn workspace_id(&self, workspace: &Path) -> Result<String, String> {
        match crate::native_memory::NativeMemoryStore::workspace_id(workspace) {
            Ok(Some(id)) => Ok(id),
            Ok(None) => {
                Err("workspace memory requires a git repository with an origin".to_string())
            }
            Err(err) => Err(format!("failed to resolve workspace identity: {err}")),
        }
    }

    fn search(
        &self,
        workspace: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        match store.search_for_workspace(workspace, query, limit) {
            Ok(hits) => Ok(hits.into_iter().map(portable_hit).collect()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn remember(
        &self,
        target: MemoryRememberTarget,
        note: &str,
    ) -> Result<MemoryRemembered, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        let (scope, workspace_id) = match target {
            MemoryRememberTarget::Global => (crate::native_memory::MemoryScope::Global, None),
            MemoryRememberTarget::Workspace { workspace_id } => (
                crate::native_memory::MemoryScope::Workspace,
                Some(workspace_id),
            ),
        };
        match store.remember(scope, workspace_id.as_deref(), note) {
            Ok(hit) => Ok(MemoryRemembered {
                source: hit.source,
                line_start: hit.line_start,
            }),
            Err(err) => Err(err.to_string()),
        }
    }

    fn import(&self) -> Result<MemoryImportOutcome, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        let legacy_path = store
            .root()
            .parent()
            .map(|parent| parent.join("memory.md"))
            .unwrap_or_else(|| app.memory_path.clone());
        match store.import_legacy(&legacy_path) {
            Ok(true) => Ok(MemoryImportOutcome::Imported {
                destination: store.global_path(),
            }),
            Ok(false) => Ok(MemoryImportOutcome::Skipped),
            Err(err) => Err(err.to_string()),
        }
    }

    fn get(&self, workspace: &Path, id: i64) -> Result<MemoryGetOutcome, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        match store.get_for_workspace(workspace, id) {
            Ok(Some(hit)) => Ok(MemoryGetOutcome::Found(portable_hit(hit))),
            Ok(None) => Ok(MemoryGetOutcome::NotFound),
            Err(err) => Err(err.to_string()),
        }
    }

    fn export(&self) -> Result<MemoryExport, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        match store.export() {
            Ok(content) => Ok(MemoryExport { content }),
            Err(err) => Err(err.to_string()),
        }
    }

    fn reindex(&self) -> Result<MemoryReindex, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        match store.reindex() {
            Ok(entry_count) => Ok(MemoryReindex { entry_count }),
            Err(err) => Err(err.to_string()),
        }
    }

    fn delete(&self, scope: MemoryDeleteScope) -> Result<MemoryDelete, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        let result = match scope {
            MemoryDeleteScope::All => store.delete_all(None, None),
            MemoryDeleteScope::Global => {
                store.delete_all(Some(crate::native_memory::MemoryScope::Global), None)
            }
        };
        result.map(|()| MemoryDelete).map_err(|err| err.to_string())
    }

    fn delete_workspace(&self, workspace: &Path) -> Result<MemoryDelete, String> {
        let app = self.host.app.borrow();
        let store = native_store_from_memory_path(&app.memory_path);
        match crate::native_memory::NativeMemoryStore::workspace_id(workspace) {
            Ok(Some(id)) => store
                .delete_all(
                    Some(crate::native_memory::MemoryScope::Workspace),
                    Some(&id),
                )
                .map(|()| MemoryDelete)
                .map_err(|err| err.to_string()),
            Ok(None) => {
                Err("workspace memory requires a git repository with an origin".to_string())
            }
            Err(err) => Err(format!("failed to resolve workspace identity: {err}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Envelope construction (D1)
// ---------------------------------------------------------------------------

/// Plugin host-data adapter (FEAT-020 D1/D11).
///
/// Owns every concrete plugin service the live `/plugin` branch closure
/// consumes: registry reads/mutations, the async mutation/network-policy
/// bridge (D11), export, legacy executable-tool scan, Kimi managed import,
/// and the marketplace store (including the builtin `official` catalog).
/// Every method borrows `App` only for the duration of one call and converts
/// host values to portable contract values before returning. Handlers receive
/// only the portable facet and never name `PluginRegistry`, `LoadedPlugin`,
/// `Config`, or another concrete host service.
pub(crate) struct PluginAdapter<'a> {
    host: SharedCommandHost<'a>,
}

/// Convert a TUI-owned diagnostic to the portable contract diagnostic.
fn portable_diagnostic(diagnostic: &crate::plugins::types::PluginDiagnostic) -> PluginDiagnostic {
    PluginDiagnostic {
        level: match diagnostic.level {
            crate::plugins::types::PluginDiagnosticLevel::Warning => PluginDiagnosticLevel::Warning,
            crate::plugins::types::PluginDiagnosticLevel::Error => PluginDiagnosticLevel::Error,
        },
        code: diagnostic.code.to_string(),
        message: diagnostic.message.clone(),
        path: diagnostic.path.clone(),
    }
}

/// Convert a TUI marketplace diagnostic into the portable contract diagnostic.
fn portable_marketplace_diagnostic(
    diagnostic: &crate::plugins::marketplace::types::MarketplaceDiagnostic,
) -> PluginDiagnostic {
    PluginDiagnostic {
        level: match diagnostic.level {
            crate::plugins::types::PluginDiagnosticLevel::Warning => PluginDiagnosticLevel::Warning,
            crate::plugins::types::PluginDiagnosticLevel::Error => PluginDiagnosticLevel::Error,
        },
        code: diagnostic.code.clone(),
        message: diagnostic.message.clone(),
        path: None,
    }
}

/// Convert a TUI-owned loaded plugin into the portable list summary.
fn portable_summary(plugin: &crate::plugins::types::LoadedPlugin) -> PluginSummary {
    PluginSummary {
        name: plugin.name().to_string(),
        id: plugin.id.as_str().to_string(),
        state_label: plugin.state_label().to_string(),
        scope: plugin.scope.as_str().to_string(),
        trust_status: plugin.trust_status.as_str().to_string(),
        compatibility: plugin.compatibility().as_str().to_string(),
        inventory: plugin.inventory.summary(),
        active: plugin.active(),
        trusted: plugin.trusted(),
        enabled: plugin.enabled,
    }
}

/// Convert one TUI MCP server config into the portable review detail.
fn portable_mcp_server(name: &str, server: &crate::mcp::McpServerConfig) -> PluginMcpServerDetail {
    let transport = if server.url.is_some() {
        PluginMcpTransport::Http
    } else if server.command.is_some() {
        PluginMcpTransport::Stdio
    } else {
        PluginMcpTransport::Invalid
    };
    let mut env = server
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    env.sort_unstable();
    let mut env_headers = server
        .env_headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    env_headers.sort_unstable();
    PluginMcpServerDetail {
        name: name.to_string(),
        transport,
        command: server.command.clone(),
        argv: server.args.clone(),
        cwd: server.cwd.clone(),
        env,
        url: server.url.clone(),
        env_headers,
        bearer_token_env_var: server.bearer_token_env_var.clone(),
        connect_timeout_secs: server.connect_timeout,
        execute_timeout_secs: server.execute_timeout,
        read_timeout_secs: server.read_timeout,
        required: server.required,
        enabled_tools: server.enabled_tools.clone(),
        disabled_tools: server.disabled_tools.clone(),
        enabled: server.is_enabled(),
    }
}

/// Convert a TUI-owned loaded plugin into the portable full detail.
fn portable_detail(plugin: &crate::plugins::types::LoadedPlugin) -> PluginDetail {
    let mcp_servers = plugin
        .manifest
        .mcp_servers
        .as_ref()
        .map(|servers| {
            let mut list = servers
                .iter()
                .map(|(name, server)| portable_mcp_server(name, server))
                .collect::<Vec<_>>();
            list.sort_by(|a, b| a.name.cmp(&b.name));
            list
        })
        .unwrap_or_default();
    PluginDetail {
        name: plugin.name().to_string(),
        id: plugin.id.as_str().to_string(),
        inventory_summary: plugin.inventory.summary(),
        version: plugin.manifest.plugin.version.clone(),
        origin: plugin.origin.as_str().to_string(),
        scope: plugin.scope.as_str().to_string(),
        state_label: plugin.state_label().to_string(),
        trust_status: plugin.trust_status.as_str().to_string(),
        compatibility: plugin.compatibility().as_str().to_string(),
        content_hash: plugin.content_hash.clone(),
        capability_hash: plugin.capability_hash.clone(),
        canonical_root: plugin.canonical_root.clone(),
        active: plugin.active(),
        trusted: plugin.trusted(),
        enabled: plugin.enabled,
        unsupported_labels: plugin
            .inventory
            .unsupported_labels()
            .into_iter()
            .map(str::to_string)
            .collect(),
        supported_labels: plugin
            .inventory
            .supported_labels()
            .into_iter()
            .map(str::to_string)
            .collect(),
        skills: plugin
            .skill_snapshots
            .iter()
            .map(|skill| format!("{}:{}", plugin.name(), skill.name))
            .collect(),
        filesystem_roots: plugin.inventory.filesystem_roots.clone(),
        network_hosts: plugin.inventory.network_hosts.clone(),
        stdio_mcp_servers: plugin.inventory.stdio_mcp_servers,
        lifecycle_mutation: plugin.inventory.lifecycle_mutation,
        mcp_servers,
        diagnostics: plugin.diagnostics.iter().map(portable_diagnostic).collect(),
    }
}

/// Convert a TUI mutation receipt into the portable contract receipt.
fn portable_mutation_receipt(
    receipt: &crate::plugins::mutation::PluginMutationReceipt,
) -> PluginMutationReceipt {
    let outcome = match &receipt.outcome {
        crate::plugins::mutation::PluginMutationOutcome::Installed => {
            PluginMutationOutcome::Installed
        }
        crate::plugins::mutation::PluginMutationOutcome::Updated => PluginMutationOutcome::Updated,
        crate::plugins::mutation::PluginMutationOutcome::NoChange => {
            PluginMutationOutcome::NoChange
        }
        crate::plugins::mutation::PluginMutationOutcome::Uninstalled => {
            PluginMutationOutcome::Uninstalled
        }
        crate::plugins::mutation::PluginMutationOutcome::NeedsApproval(host) => {
            PluginMutationOutcome::NeedsApproval(host.clone())
        }
        crate::plugins::mutation::PluginMutationOutcome::NetworkDenied(host) => {
            PluginMutationOutcome::NetworkDenied(host.clone())
        }
    };
    PluginMutationReceipt {
        name: receipt.name.clone(),
        path: receipt.path.clone(),
        content_hash: receipt.content_hash.clone(),
        installed_content_hash: receipt.installed_content_hash.clone(),
        outcome,
    }
}

/// Convert a TUI export receipt into the portable contract receipt.
fn portable_export_receipt(
    receipt: &crate::plugins::export::PluginExportReceipt,
) -> PluginExportReceipt {
    PluginExportReceipt {
        exported_name: receipt.exported_name.clone(),
        target: receipt.target.clone(),
        display_name: receipt.display_name.clone(),
        wrote_mcp_json: receipt.wrote_mcp_json,
        files_copied: receipt.files_copied as u64,
        skills_normalized: receipt.skills_normalized,
    }
}

/// Convert one TUI legacy tool entry into the portable value.
fn portable_legacy_tool(
    path: &Path,
    metadata: &crate::tools::plugin::PluginMetadata,
) -> PluginLegacyTool {
    PluginLegacyTool {
        name: metadata.name.clone(),
        description: metadata.description.clone(),
        approval: match metadata.approval {
            crate::tools::spec::ApprovalRequirement::Auto => "auto",
            crate::tools::spec::ApprovalRequirement::Suggest => "suggest",
            crate::tools::spec::ApprovalRequirement::Required => "required",
        }
        .to_string(),
        input_schema: Some(
            serde_json::to_string_pretty(&metadata.input_schema).unwrap_or_default(),
        ),
        path: path.to_path_buf(),
    }
}

/// Convert one TUI marketplace candidate into the portable value.
fn portable_marketplace_candidate(
    candidate: &crate::plugins::marketplace::types::MarketplaceCandidate,
) -> PluginMarketplaceCandidate {
    let install_plan = match &candidate.install_plan {
        crate::plugins::marketplace::types::MarketplaceInstallPlan::Supported {
            spec,
            source_kind,
        } => PluginMarketplaceInstallPlan::Supported {
            spec: spec.clone(),
            source_kind: source_kind.clone(),
        },
        crate::plugins::marketplace::types::MarketplaceInstallPlan::Unsupported {
            reason, ..
        } => PluginMarketplaceInstallPlan::Unsupported {
            reason: reason.clone(),
        },
    };
    PluginMarketplaceCandidate {
        name: candidate.name.clone(),
        display_name: candidate.display_name.clone(),
        version: candidate.version.clone(),
        tier: candidate.provenance.tier.as_str().to_string(),
        compatibility: candidate
            .compatibility
            .as_ref()
            .map(|c| c.as_str().to_string()),
        install_plan,
        description: candidate.description.clone(),
        homepage: candidate.homepage.clone(),
        repository: candidate.repository.clone(),
        author: candidate.author.clone(),
        license: candidate.license.clone(),
        keywords: candidate.keywords.clone(),
        when: candidate.when.as_ref().map(|when| format!("{when:?}")),
        diagnostics: candidate
            .diagnostics
            .iter()
            .map(portable_marketplace_diagnostic)
            .collect(),
        has_errors: candidate.has_errors(),
    }
}

/// Convert one TUI marketplace catalog into the portable value.
fn portable_marketplace_catalog(
    catalog: &crate::plugins::marketplace::types::MarketplaceCatalog,
) -> PluginMarketplaceCatalog {
    portable_marketplace_catalog_with_source(catalog, None)
}

/// Convert one stored TUI marketplace catalog (with its source path).
fn portable_marketplace_catalog_with_source(
    catalog: &crate::plugins::marketplace::types::MarketplaceCatalog,
    source_path: Option<&str>,
) -> PluginMarketplaceCatalog {
    PluginMarketplaceCatalog {
        id: catalog.id.as_str().to_string(),
        source_path: source_path.map(str::to_string),
        display_name: catalog.display_name.clone(),
        description: catalog.description.clone(),
        format: catalog.format.as_str().to_string(),
        tier: catalog.provenance.tier.as_str().to_string(),
        publisher: catalog.provenance.publisher.clone(),
        total_candidates: catalog.total_candidates(),
        warning_count: catalog.warning_count(),
        candidates: catalog
            .candidates
            .iter()
            .map(portable_marketplace_candidate)
            .collect(),
        diagnostics: catalog
            .diagnostics
            .iter()
            .map(portable_marketplace_diagnostic)
            .collect(),
    }
}

/// Kimi managed-plugin scan (host-side, FEAT-020 D1). Mirrors the legacy
/// `/plugin import kimi` scan exactly: only immediate canonical children of
/// `~/.kimi-code/plugins/managed`, rejecting symlinks/reparse points,
/// non-directories, and children that escape the root. Returns portable
/// candidate values; rejection reasons cross as safe text.
fn scan_managed_plugins_portable(
    home_override: Option<&Path>,
) -> Result<PluginManagedScan, String> {
    use std::fs;
    use std::path::PathBuf;

    const MAX_MANAGED_CHILDREN: usize = 128;
    const KIMI_PLUGIN_JSON_NAME: &str = crate::plugins::agent_plugin::KIMI_PLUGIN_JSON_NAME;

    struct Candidate {
        name: String,
        version: String,
        license: Option<String>,
        canonical_path: PathBuf,
        content_hash: String,
        capability_hash: String,
        inventory: String,
        applicable: bool,
    }

    fn inspect_candidate(canonical_path: &Path) -> Result<Candidate, String> {
        let manifest_path = canonical_path.join(KIMI_PLUGIN_JSON_NAME);
        let metadata = fs::symlink_metadata(&manifest_path).map_err(|error| {
            format!(
                "Kimi manifest unreadable at {}: {}",
                canonical_path.display(),
                error
            )
        })?;
        if crate::plugins::metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
            return Err(format!(
                "Kimi manifest must be a regular file at {}",
                canonical_path.display()
            ));
        }
        let validated = crate::plugins::manifest::PluginManifest::validate_from_path(
            &manifest_path,
        )
        .map_err(|error| {
            format!(
                "Kimi manifest invalid at {}: {error}",
                canonical_path.display()
            )
        })?;
        let name = validated.manifest.plugin.name.clone();
        if canonical_path.file_name().and_then(|part| part.to_str()) != Some(name.as_str()) {
            return Err(format!(
                "Kimi directory name `{}` does not match manifest name `{}`",
                canonical_path.display(),
                name
            ));
        }
        Ok(Candidate {
            name,
            version: validated.manifest.plugin.version.clone(),
            license: validated.manifest.plugin.license.clone(),
            canonical_path: validated.canonical_root,
            content_hash: validated.content_hash,
            capability_hash: validated.capability_hash,
            inventory: validated.inventory.summary(),
            applicable: validated.applicable,
        })
    }

    let home = match home_override {
        Some(home) => home.to_path_buf(),
        None => crate::config::effective_home_dir().ok_or_else(|| {
            tr(
                crate::localization::Locale::En,
                crate::localization::MessageId::PluginKimiHomeMissing,
            )
            .into_owned()
            .to_string()
        })?,
    };
    let configured_root = home.join(".kimi-code/plugins/managed");
    let metadata = match fs::symlink_metadata(&configured_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PluginManagedScan {
                root: configured_root,
                candidates: Vec::new(),
                rejected: Vec::new(),
            });
        }
        Err(error) => {
            let root_text = escape_review_text(&configured_root.display().to_string());
            let error_text = escape_review_text(&error.to_string());
            return Err(tr(
                crate::localization::Locale::En,
                crate::localization::MessageId::PluginKimiRootInspectFailed,
            )
            .replace("{root}", &root_text)
            .replace("{error}", &error_text));
        }
    };
    if crate::plugins::metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
        let root_text = escape_review_text(&configured_root.display().to_string());
        return Err(tr(
            crate::localization::Locale::En,
            crate::localization::MessageId::PluginKimiRootMustBeDirectory,
        )
        .replace("{root}", &root_text));
    }
    let canonical_root = configured_root.canonicalize().map_err(|error| {
        let root_text = escape_review_text(&configured_root.display().to_string());
        let error_text = escape_review_text(&error.to_string());
        tr(
            crate::localization::Locale::En,
            crate::localization::MessageId::PluginKimiRootCanonicalizeFailed,
        )
        .replace("{root}", &root_text)
        .replace("{error}", &error_text)
    })?;
    let mut entries = fs::read_dir(&canonical_root)
        .map_err(|error| {
            let root_text = escape_review_text(&canonical_root.display().to_string());
            let error_text = escape_review_text(&error.to_string());
            tr(
                crate::localization::Locale::En,
                crate::localization::MessageId::PluginKimiRootListFailed,
            )
            .replace("{root}", &root_text)
            .replace("{error}", &error_text)
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            let error_text = escape_review_text(&error.to_string());
            tr(
                crate::localization::Locale::En,
                crate::localization::MessageId::PluginKimiEntryReadFailed,
            )
            .replace("{error}", &error_text)
        })?;
    if entries.len() > MAX_MANAGED_CHILDREN {
        return Err(tr(
            crate::localization::Locale::En,
            crate::localization::MessageId::PluginKimiEntryLimit,
        )
        .replace("{count}", &entries.len().to_string())
        .replace("{max}", &MAX_MANAGED_CHILDREN.to_string()));
    }
    entries.sort_by_key(fs::DirEntry::file_name);

    let mut candidates = Vec::new();
    let mut rejected = Vec::new();
    for entry in entries {
        let path = entry.path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                let path_text = escape_review_text(&path.display().to_string());
                let error_text = escape_review_text(&error.to_string());
                rejected.push(
                    tr(
                        crate::localization::Locale::En,
                        crate::localization::MessageId::PluginKimiEntryInspectFailed,
                    )
                    .replace("{path}", &path_text)
                    .replace("{error}", &error_text),
                );
                continue;
            }
        };
        if crate::plugins::metadata_is_link_or_reparse(&metadata) {
            let path_text = escape_review_path(&path);
            rejected.push(
                tr(
                    crate::localization::Locale::En,
                    crate::localization::MessageId::PluginKimiEntryLinksRefused,
                )
                .replace("{path}", &path_text),
            );
            continue;
        }
        if !metadata.is_dir() {
            continue;
        }
        let canonical_path = match path.canonicalize() {
            Ok(path) if path.parent() == Some(canonical_root.as_path()) => path,
            Ok(canonical_path) => {
                let path_text = escape_review_text(&path.display().to_string());
                let canonical_text = escape_review_text(&canonical_path.display().to_string());
                rejected.push(
                    tr(
                        crate::localization::Locale::En,
                        crate::localization::MessageId::PluginKimiEntryOutsideRoot,
                    )
                    .replace("{path}", &path_text)
                    .replace("{canonical_path}", &canonical_text),
                );
                continue;
            }
            Err(error) => {
                let path_text = escape_review_text(&path.display().to_string());
                let error_text = escape_review_text(&error.to_string());
                rejected.push(
                    tr(
                        crate::localization::Locale::En,
                        crate::localization::MessageId::PluginKimiEntryCanonicalizeFailed,
                    )
                    .replace("{path}", &path_text)
                    .replace("{error}", &error_text),
                );
                continue;
            }
        };
        match inspect_candidate(&canonical_path) {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => rejected.push(error),
        }
    }
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(PluginManagedScan {
        root: canonical_root,
        candidates: candidates
            .into_iter()
            .map(|candidate| PluginManagedCandidate {
                name: candidate.name,
                version: candidate.version,
                license: candidate.license,
                canonical_path: candidate.canonical_path,
                content_hash: candidate.content_hash,
                capability_hash: candidate.capability_hash,
                inventory: candidate.inventory,
                applicable: candidate.applicable,
            })
            .collect(),
        rejected,
    })
}

/// Escape review text exactly like the plugin render helpers (FEAT-020 D2).
fn escape_review_text(value: &str) -> String {
    crate::commands::groups::plugins::render::escape_review_text(value)
}

/// Escape a review path exactly like the plugin render helpers (FEAT-020 D2).
fn escape_review_path(path: &Path) -> String {
    crate::commands::groups::plugins::render::escape_review_path(path)
}

/// The catalog built into every Codewhale release. It lists bundles that
/// ship inside the binary (`builtin:<name>` install specs), so there is
/// nothing to fetch; installing still goes through the reviewed installer
/// and lands disabled and untrusted like everything else.
fn builtin_official_catalog() -> crate::plugins::marketplace::store::StoredMarketplaceCatalog {
    fn official_catalog_document() -> serde_json::Value {
        serde_json::json!({
            "name": "official",
            "description": "Plugins built into this Codewhale release",
            "version": crate::plugins::install::BUILTIN_BUNDLE_NAMES.len().to_string(),
            "plugins": [
                {
                    "name": codewhale_computer_use::bundle::BUNDLE_NAME,
                    "source": format!("builtin:{}", codewhale_computer_use::bundle::BUNDLE_NAME),
                    "version": codewhale_computer_use::bundle::version(),
                    "description": "See and operate this desktop or an attached Android / HarmonyOS device with a vision model (deepseek-v4-flash-vision-exp): screenshots, clicks, typing, scrolling, app launch. Also: `codewhale computer-use setup`.",
                    "homepage": "https://github.com/Hmbown/CodeWhale/blob/main/docs/COMPUTER_USE.md"
                }
            ]
        })
    }
    use crate::plugins::marketplace::parsers::{MarketplaceDocument, parse_catalog};
    use crate::plugins::marketplace::types::{
        CatalogTier, MarketplaceCatalogId, MarketplaceFormat,
    };
    let mut catalog = parse_catalog(MarketplaceDocument {
        catalog_id: MarketplaceCatalogId::new("official"),
        format: MarketplaceFormat::Codewhale,
        root: official_catalog_document(),
        base: None,
    });
    catalog.provenance.tier = CatalogTier::Official;
    catalog.provenance.publisher = Some("Codewhale".to_string());
    crate::plugins::marketplace::store::StoredMarketplaceCatalog {
        added_at: "builtin".to_string(),
        source_path: "builtin:official".to_string(),
        catalog,
    }
}

impl CommandPluginContext for PluginAdapter<'_> {
    fn summaries(&self) -> Result<Vec<PluginSummary>, String> {
        let app = self.host.app.borrow();
        Ok(app
            .plugin_registry
            .list()
            .iter()
            .map(|plugin| portable_summary(plugin))
            .collect())
    }

    fn detail(&self, selector: &str) -> Result<PluginDetail, String> {
        let app = self.host.app.borrow();
        let plugin = app
            .plugin_registry
            .get(selector)
            .ok_or_else(|| format!("no plugin named {selector}"))?;
        Ok(portable_detail(plugin))
    }

    fn registry_diagnostics(&self) -> Vec<PluginDiagnostic> {
        self.host
            .app
            .borrow()
            .plugin_registry
            .diagnostics()
            .iter()
            .map(portable_diagnostic)
            .collect()
    }

    fn validation_is_clean(&self) -> bool {
        self.host.app.borrow().plugin_registry.validation_is_clean()
    }

    fn len(&self) -> usize {
        self.host.app.borrow().plugin_registry.len()
    }

    fn is_empty(&self) -> bool {
        self.host.app.borrow().plugin_registry.is_empty()
    }

    fn reload(&mut self) -> Result<usize, String> {
        let mut app = self.host.app.borrow_mut();
        let workspace = app.workspace.clone();
        app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
        app.refresh_skill_cache();
        Ok(app.plugin_registry.len())
    }

    fn state_path(&self) -> Option<PathBuf> {
        self.host
            .app
            .borrow()
            .plugin_registry
            .state_path()
            .map(Path::to_path_buf)
    }

    fn suggest(&self, task: &str) -> Result<Vec<PluginSuggestion>, String> {
        let task = task.trim();
        if task.chars().count() < 3 {
            return Err("Usage: /plugin suggest <task of at least 3 characters>".to_string());
        }
        let app = self.host.app.borrow();
        let mut skills = std::collections::BTreeMap::new();
        for plugin in app.plugin_registry.list() {
            let mut description_parts = plugin
                .manifest
                .plugin
                .description
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            let mut keywords = Vec::new();
            for skill in &plugin.skill_snapshots {
                description_parts.push(skill.name.clone());
                description_parts.push(skill.description.clone());
                keywords.push(skill.name.clone());
                keywords.extend(skill.aliases.iter().cloned());
            }
            skills.insert(
                plugin.name().to_string(),
                crate::skills::RegistryEntry {
                    source: plugin.id.as_str().to_string(),
                    description: (!description_parts.is_empty())
                        .then(|| description_parts.join(" ")),
                    keywords,
                    domains: plugin.inventory.network_hosts.clone(),
                },
            );
        }
        let index = crate::skills::RegistryDocument { skills };
        let recommendations = crate::skills::recommend::recommend_remote_skills(task, &index, 3);
        let mut suggestions = Vec::new();
        for recommendation in recommendations {
            let Some(plugin) = app.plugin_registry.get(&recommendation.entry.source) else {
                continue;
            };
            let description = plugin
                .manifest
                .plugin
                .description
                .as_deref()
                .filter(|description| !description.trim().is_empty())
                .unwrap_or("No description provided.")
                .to_string();
            let next_step = if plugin.active() {
                format!("Already active: /plugin show {}", plugin.name())
            } else if !plugin.trusted() {
                format!("Review before enabling: /plugin trust {}", plugin.name())
            } else if !plugin.enabled {
                format!(
                    "Enable if that review still applies: /plugin enable {}",
                    plugin.name()
                )
            } else {
                format!("Inspect its inactive state: /plugin show {}", plugin.name())
            };
            suggestions.push(PluginSuggestion {
                name: plugin.name().to_string(),
                state_label: plugin.state_label().to_string(),
                description,
                why: recommendation.matched_terms.clone(),
                next_step,
            });
        }
        Ok(suggestions)
    }

    fn trust(&mut self, selector: &str, token: &str) -> Result<(), String> {
        let expected = {
            let app = self.host.app.borrow();
            app.plugin_registry
                .get(selector)
                .map(|plugin| format!("{}.{}", plugin.content_hash, plugin.capability_hash))
                .ok_or_else(|| format!("no plugin named {selector}"))?
        };
        if token != expected {
            return Err(
                "Review token does not match this bundle content and capability set; run `/plugin trust <name>` again"
                    .to_string(),
            );
        }
        {
            let mut app = self.host.app.borrow_mut();
            std::sync::Arc::make_mut(&mut app.plugin_registry).trust(selector)?;
            app.refresh_skill_cache();
        }
        Ok(())
    }

    fn enable(&mut self, selector: &str) -> Result<(), String> {
        let needs_review = self
            .host
            .app
            .borrow()
            .plugin_registry
            .get(selector)
            .is_some_and(|plugin| !plugin.trusted());
        if needs_review {
            // Enabling is the natural entry point; open the capability review
            // instead of an opaque denial (matches the legacy handler).
            return Err("plugin requires review before enabling".to_string());
        }
        let mut app = self.host.app.borrow_mut();
        std::sync::Arc::make_mut(&mut app.plugin_registry).enable(selector)?;
        app.refresh_skill_cache();
        Ok(())
    }

    fn disable(&mut self, selector: &str) -> Result<(), String> {
        let mut app = self.host.app.borrow_mut();
        std::sync::Arc::make_mut(&mut app.plugin_registry).disable(selector)?;
        app.refresh_skill_cache();
        app.active_skill = None;
        app.active_skill_provenance = None;
        Ok(())
    }

    fn revoke_trust(&mut self, selector: &str) -> Result<(), String> {
        let mut app = self.host.app.borrow_mut();
        std::sync::Arc::make_mut(&mut app.plugin_registry).revoke_trust(selector)?;
        app.refresh_skill_cache();
        app.active_skill = None;
        app.active_skill_provenance = None;
        Ok(())
    }

    fn install(
        &mut self,
        source: &str,
        expected_content_hash: Option<&str>,
    ) -> Result<PluginMutationReceipt, String> {
        use crate::plugins::install::PluginInstallSource;
        use crate::plugins::mutation::{
            PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
        };

        let plugin_source = PluginInstallSource::parse(source).map_err(|error| {
            format!(
                "Invalid plugin install source `{source}`: {error:#}\n\
                 Expected a local path, github:owner/repo, an HTTPS tarball URL, or builtin:<name>."
            )
        })?;
        let network = plugin_network_policy();
        let expected_content_hash = expected_content_hash.map(str::to_string);
        let expected_for_request = expected_content_hash.clone();
        let mut app = self.host.app.borrow_mut();
        let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
        let outcome = run_async(async move {
            let ctx = PluginMutationContext {
                network: &network,
                max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
            };
            let request = match expected_for_request {
                Some(expected_content_hash) => PluginMutationRequest::InstallExact {
                    source: plugin_source,
                    expected_content_hash,
                },
                None => PluginMutationRequest::Install {
                    source: plugin_source,
                },
            };
            crate::plugins::mutation::execute(request, &ctx, registry).await
        });
        match outcome {
            Ok(receipt) => {
                let portable = portable_mutation_receipt(&receipt);
                // Rediscover and refresh the skill cache after any install.
                if matches!(receipt.outcome, PluginMutationOutcome::Installed) {
                    let workspace = app.workspace.clone();
                    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
                    app.refresh_skill_cache();
                }
                Ok(portable)
            }
            Err(error) => Err(format!("Plugin install failed: {error:#}")),
        }
    }

    fn update(&mut self, selector: &str) -> Result<PluginMutationReceipt, String> {
        use crate::plugins::mutation::{
            PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
        };
        let network = plugin_network_policy();
        let selector_owned = selector.to_string();
        let mut app = self.host.app.borrow_mut();
        let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
        let outcome = run_async(async move {
            let ctx = PluginMutationContext {
                network: &network,
                max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
            };
            crate::plugins::mutation::execute(
                PluginMutationRequest::Update {
                    selector: selector_owned,
                },
                &ctx,
                registry,
            )
            .await
        });
        match outcome {
            Ok(receipt) => {
                let portable = portable_mutation_receipt(&receipt);
                if matches!(receipt.outcome, PluginMutationOutcome::Updated) {
                    let workspace = app.workspace.clone();
                    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
                    app.refresh_skill_cache();
                }
                Ok(portable)
            }
            Err(error) => Err(format!("Plugin update failed: {error:#}")),
        }
    }

    fn uninstall(&mut self, selector: &str) -> Result<PluginMutationReceipt, String> {
        use crate::plugins::mutation::{
            PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
        };
        let network = plugin_network_policy();
        let selector_owned = selector.to_string();
        let mut app = self.host.app.borrow_mut();
        let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
        let outcome = run_async(async move {
            let ctx = PluginMutationContext {
                network: &network,
                max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
            };
            crate::plugins::mutation::execute(
                PluginMutationRequest::Uninstall {
                    selector: selector_owned,
                },
                &ctx,
                registry,
            )
            .await
        });
        match outcome {
            Ok(receipt) => {
                let portable = portable_mutation_receipt(&receipt);
                if matches!(receipt.outcome, PluginMutationOutcome::Uninstalled) {
                    let workspace = app.workspace.clone();
                    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
                    app.refresh_skill_cache();
                    app.active_skill = None;
                    app.active_skill_provenance = None;
                }
                Ok(portable)
            }
            Err(error) => Err(format!("Plugin uninstall failed: {error:#}")),
        }
    }

    fn export(&self, selector: &str, target: &Path) -> Result<PluginExportReceipt, String> {
        let app = self.host.app.borrow();
        let plugin = app
            .plugin_registry
            .get(selector)
            .ok_or_else(|| format!("no plugin named {selector}"))?
            .clone();
        let existing_names: std::collections::BTreeSet<String> = app
            .plugin_registry
            .list()
            .iter()
            .map(|other| other.name().to_string())
            .filter(|name| name != plugin.name())
            .collect();
        let target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            app.workspace.join(target)
        };
        crate::plugins::export::export_plugin_bundle(&plugin, &target, &existing_names)
            .map(|receipt| portable_export_receipt(&receipt))
            .map_err(|error| format!("Export of `{}` failed: {}", plugin.name(), error))
    }

    fn legacy_scan(&self) -> Result<Option<PluginLegacyScan>, String> {
        let app = self.host.app.borrow();
        let Some(dir) = app
            .legacy_plugin_tools_dir
            .clone()
            .or_else(default_codewhale_tools_dir)
        else {
            return Ok(None);
        };
        if !dir.exists() {
            return Ok(None);
        }
        let tools = crate::tools::plugin::scan_plugin_dir(&dir)
            .into_iter()
            .map(|(path, metadata)| portable_legacy_tool(&path, &metadata))
            .collect();
        Ok(Some(PluginLegacyScan { dir, tools }))
    }

    fn managed_scan(&self, home_override: Option<&Path>) -> Result<PluginManagedScan, String> {
        scan_managed_plugins_portable(home_override)
    }

    fn managed_install(
        &mut self,
        canonical_path: &Path,
        expected_content_hash: &str,
    ) -> Result<PluginMutationReceipt, String> {
        use crate::plugins::install::PluginInstallSource;
        use crate::plugins::mutation::{
            PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
        };
        let network = plugin_network_policy();
        let expected_content_hash = expected_content_hash.to_string();
        let path = canonical_path.to_path_buf();
        let mut app = self.host.app.borrow_mut();
        let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
        let outcome = run_async(async move {
            let ctx = PluginMutationContext {
                network: &network,
                max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
            };
            crate::plugins::mutation::execute(
                PluginMutationRequest::InstallExact {
                    source: PluginInstallSource::LocalPath(path),
                    expected_content_hash,
                },
                &ctx,
                registry,
            )
            .await
        });
        match outcome {
            Ok(receipt) => {
                let portable = portable_mutation_receipt(&receipt);
                if matches!(receipt.outcome, PluginMutationOutcome::Installed) {
                    let workspace = app.workspace.clone();
                    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
                    app.refresh_skill_cache();
                }
                Ok(portable)
            }
            Err(error) => Err(format!("Plugin install failed: {error:#}")),
        }
    }

    fn marketplace_state(&self) -> Result<PluginMarketplaceState, String> {
        let app = self.host.app.borrow();
        let official = builtin_official_catalog();
        let official = portable_marketplace_catalog(&official.catalog);
        let store = crate::plugins::marketplace::store::MarketplaceStore::open(
            app.plugin_registry.state_path(),
        )
        .ok_or_else(|| {
            "This plugin registry has no persistence store, so marketplace catalogs cannot be saved."
                .to_string()
        })?;
        let state = store.load()?;
        let stored = state
            .catalogs()
            .values()
            .map(|entry| {
                portable_marketplace_catalog_with_source(
                    &entry.catalog,
                    Some(entry.source_path.as_str()),
                )
            })
            .collect();
        Ok(PluginMarketplaceState { official, stored })
    }

    fn marketplace_add(
        &mut self,
        name: &str,
        path: &Path,
    ) -> Result<PluginMarketplaceAddReceipt, String> {
        let name_valid = !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
        if !name_valid {
            return Err(
                "Marketplace name must be 1-64 characters of letters, digits, `-`, `_`, or `.`"
                    .to_string(),
            );
        }
        if name == "official" {
            return Err(
                "`official` is the catalog built into Codewhale; pick another name.".to_string(),
            );
        }
        let app = self.host.app.borrow();
        let store = crate::plugins::marketplace::store::MarketplaceStore::open(
            app.plugin_registry.state_path(),
        )
        .ok_or_else(|| {
            "This plugin registry has no persistence store, so marketplace catalogs cannot be saved."
                .to_string()
        })?;
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            app.workspace.join(path)
        };
        let canonical = canonical_document(&path)?;
        let body = read_bounded(&canonical)?;
        let root = serde_json::from_str::<serde_json::Value>(&body).map_err(|error| {
            format!(
                "Catalog at {} is not valid JSON: {error}",
                canonical.display()
            )
        })?;
        let document = crate::plugins::marketplace::parsers::MarketplaceDocument {
            catalog_id: crate::plugins::marketplace::types::MarketplaceCatalogId::new(name),
            format: crate::plugins::marketplace::types::MarketplaceFormat::Auto,
            root,
            base: Some(canonical.display().to_string()),
        };
        let catalog = crate::plugins::marketplace::parsers::parse_catalog(document);
        if catalog.candidates.is_empty() && catalog.error_count() > 0 {
            return Err(format!(
                "Catalog `{}` could not be parsed as any known marketplace format (kimi, claude, codex, codewhale):\n{}",
                name,
                render_diagnostics_inline(&catalog.diagnostics)
            ));
        }
        let candidate_count = catalog.total_candidates();
        let warning_count = catalog.warning_count();
        let portable_catalog = portable_marketplace_catalog(&catalog);
        let entry = crate::plugins::marketplace::store::StoredMarketplaceCatalog {
            added_at: chrono::Utc::now().to_rfc3339(),
            source_path: canonical.display().to_string(),
            catalog,
        };
        store
            .add(&entry.catalog.id.clone(), entry)
            .map_err(|error| error.to_string())?;
        Ok(PluginMarketplaceAddReceipt {
            name: name.to_string(),
            candidate_count,
            warning_count,
            catalog: portable_catalog,
        })
    }

    fn marketplace_remove(&mut self, name: &str) -> Result<bool, String> {
        if name == "official" {
            return Err("`official` is built into Codewhale and cannot be removed.".to_string());
        }
        let app = self.host.app.borrow();
        let store = crate::plugins::marketplace::store::MarketplaceStore::open(
            app.plugin_registry.state_path(),
        )
        .ok_or_else(|| {
            "This plugin registry has no persistence store, so marketplace catalogs cannot be saved."
                .to_string()
        })?;
        store.remove(name)
    }

    fn marketplace_install(
        &mut self,
        catalog: &str,
        candidate: &str,
    ) -> Result<PluginMutationReceipt, String> {
        let app = self.host.app.borrow();
        let store = crate::plugins::marketplace::store::MarketplaceStore::open(
            app.plugin_registry.state_path(),
        )
        .ok_or_else(|| {
            "This plugin registry has no persistence store, so marketplace catalogs cannot be saved."
                .to_string()
        })?;
        let state = store.load()?;
        let entry = if catalog == "official" {
            Some(builtin_official_catalog())
        } else {
            state.get(catalog).cloned()
        };
        let Some(entry) = entry else {
            return Err(format!(
                "No marketplace named `{}`. Use /plugin marketplace list.",
                catalog
            ));
        };
        let Some(candidate_entry) = entry.catalog.candidate_by_name(candidate) else {
            return Err(format!(
                "No candidate `{}` in marketplace `{}`.",
                candidate, catalog
            ));
        };
        if candidate_entry.has_errors() {
            return Err(format!(
                "Candidate `{}` has parse errors and cannot be installed:\n{}",
                candidate,
                render_diagnostics_inline(&candidate_entry.diagnostics)
            ));
        }
        let crate::plugins::marketplace::types::MarketplaceInstallPlan::Supported { spec, .. } =
            &candidate_entry.install_plan
        else {
            return Err(format!(
                "Candidate `{}` cannot be installed by Codewhale.",
                candidate
            ));
        };
        let spec = resolve_marketplace_spec(&entry.source_path, &candidate_entry.source, spec);
        drop(app);
        self.install(&spec, None)
    }
}

/// Resolve the default Codewhale tools directory (mirrors the legacy handler).
fn default_codewhale_tools_dir() -> Option<PathBuf> {
    codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join("tools"))
}

/// Resolve a user-supplied document path to an existing regular file without
/// following a final symlink (the document is untrusted input).
fn canonical_document(path: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot read catalog at {}: {e}", path.display()))?;
    if metadata.is_symlink() {
        return Err(format!(
            "Catalog path {} is a symlink; marketplace documents must be regular files",
            path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Catalog path {} is not a regular file",
            path.display()
        ));
    }
    Ok(path.to_path_buf())
}

/// Read a catalog document with a bounded size (4 MiB cap, mirrors legacy).
fn read_bounded(path: &Path) -> Result<String, String> {
    use std::io::Read;
    const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;
    let file = std::fs::File::open(path)
        .map_err(|e| format!("Cannot read catalog at {}: {e}", path.display()))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_CATALOG_BYTES {
        return Err(format!(
            "Catalog at {} exceeds the {} byte limit",
            path.display(),
            MAX_CATALOG_BYTES
        ));
    }
    let mut text = String::new();
    let mut limited = file.take(MAX_CATALOG_BYTES + 1);
    limited
        .read_to_string(&mut text)
        .map_err(|e| format!("Cannot read catalog at {}: {e}", path.display()))?;
    Ok(text)
}

/// Resolve a marketplace install spec against the catalog's own directory.
fn resolve_marketplace_spec(
    source_path: &str,
    source: &crate::plugins::marketplace::types::MarketplaceSourceSpec,
    spec: &str,
) -> String {
    if let crate::plugins::marketplace::types::MarketplaceSourceSpec::LocalPath { path } = source
        && path.is_relative()
        && let Some(dir) = Path::new(source_path).parent()
    {
        return format!("path:{}", dir.join(path).display());
    }
    spec.to_string()
}

/// Inline diagnostics renderer shared by marketplace error paths.
fn render_diagnostics_inline(
    diagnostics: &[crate::plugins::marketplace::types::MarketplaceDiagnostic],
) -> String {
    diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} {}: {}",
                match d.level {
                    crate::plugins::types::PluginDiagnosticLevel::Error => "error",
                    crate::plugins::types::PluginDiagnosticLevel::Warning => "warning",
                },
                d.code,
                d.message
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Owns eleven facet objects sharing one synchronous TUI host proxy.
///
/// Handlers borrow only these adapters. Every method delegates to the real App
/// authority and releases its `RefCell` borrow before returning, so facets can
/// be called sequentially without exposing TUI types across the boundary.
pub(crate) struct CommandContextBundle<'a> {
    session: SessionAdapter<'a>,
    model: ModelAdapter<'a>,
    cost: CostAdapter<'a>,
    mode_policy: ModePolicyAdapter<'a>,
    system_prompt: SystemPromptAdapter<'a>,
    skills: SkillsAdapter<'a>,
    workspace: WorkspaceAdapter<'a>,
    presentation: PresentationAdapter<'a>,
    media: MediaAdapter<'a>,
    memory: MemoryAdapter<'a>,
    plugin: PluginAdapter<'a>,
}

impl<'a> CommandContextBundle<'a> {
    /// Expose exactly the capabilities declared by the command registration.
    pub(crate) fn contexts(&mut self, capabilities: CommandCapabilities) -> CommandContexts<'_> {
        let mut contexts = CommandContexts::empty();
        if capabilities.contains(CommandCapabilities::SESSION) {
            contexts = contexts.with_session(&mut self.session);
        }
        if capabilities.contains(CommandCapabilities::MODEL) {
            contexts = contexts.with_model(&mut self.model);
        }
        if capabilities.contains(CommandCapabilities::COST) {
            contexts = contexts.with_cost(&mut self.cost);
        }
        if capabilities.contains(CommandCapabilities::MODE_POLICY) {
            contexts = contexts.with_mode_policy(&mut self.mode_policy);
        }
        if capabilities.contains(CommandCapabilities::SYSTEM_PROMPT) {
            contexts = contexts.with_system_prompt(&mut self.system_prompt);
        }
        if capabilities.contains(CommandCapabilities::SKILLS) {
            contexts = contexts.with_skills(&mut self.skills);
        }
        if capabilities.contains(CommandCapabilities::WORKSPACE) {
            contexts = contexts.with_workspace(&mut self.workspace);
        }
        if capabilities.contains(CommandCapabilities::PRESENTATION) {
            contexts = contexts.with_presentation(&mut self.presentation);
        }
        if capabilities.contains(CommandCapabilities::MEDIA) {
            contexts = contexts.with_media(&mut self.media);
        }
        if capabilities.contains(CommandCapabilities::MEMORY) {
            contexts = contexts.with_memory(&mut self.memory);
        }
        if capabilities.contains(CommandCapabilities::PLUGIN) {
            contexts = contexts.with_plugin(&mut self.plugin);
        }
        contexts
    }

    /// Test-only: consume the bundle into independent facet parts.
    #[cfg(test)]
    pub(crate) fn parts(&mut self) -> ContextParts<'_> {
        let all_test_capabilities = CommandCapabilities::SESSION
            .union(CommandCapabilities::MODEL)
            .union(CommandCapabilities::COST)
            .union(CommandCapabilities::MODE_POLICY)
            .union(CommandCapabilities::SYSTEM_PROMPT)
            .union(CommandCapabilities::SKILLS)
            .union(CommandCapabilities::WORKSPACE)
            .union(CommandCapabilities::PRESENTATION)
            .union(CommandCapabilities::MEDIA)
            .union(CommandCapabilities::MEMORY)
            .union(CommandCapabilities::PLUGIN);
        self.contexts(all_test_capabilities).into_parts()
    }
}

impl App {
    /// Build an App-free capability envelope backed by authoritative TUI
    /// operations. The shared proxy is synchronous and local to one dispatch.
    pub(crate) fn command_contexts(&mut self) -> CommandContextBundle<'_> {
        let host = Rc::new(CommandHost {
            app: RefCell::new(self),
        });
        CommandContextBundle {
            session: SessionAdapter { host: host.clone() },
            model: ModelAdapter { host: host.clone() },
            cost: CostAdapter { host: host.clone() },
            mode_policy: ModePolicyAdapter { host: host.clone() },
            system_prompt: SystemPromptAdapter { host: host.clone() },
            skills: SkillsAdapter { host: host.clone() },
            workspace: WorkspaceAdapter { host: host.clone() },
            presentation: PresentationAdapter { host: host.clone() },
            media: MediaAdapter { host: host.clone() },
            memory: MemoryAdapter { host: host.clone() },
            plugin: PluginAdapter { host },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::Locale;
    use crate::models::Role;
    use tempfile::TempDir;

    fn test_app() -> App {
        crate::test_support::test_app_with_options(crate::test_support::test_tui_options(
            PathBuf::from("."),
        ))
    }

    /// A 1x1 PNG for media adapter tests.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn pending_groups_is_sorted_unique_and_matches_checked_in_frontier() {
        let mut sorted = PENDING_GROUPS.to_vec();
        sorted.sort_unstable();
        assert_eq!(PENDING_GROUPS, sorted.as_slice(), "frontier must be sorted");
        let unique: std::collections::BTreeSet<&str> = PENDING_GROUPS.iter().copied().collect();
        assert_eq!(
            unique.len(),
            PENDING_GROUPS.len(),
            "frontier must be unique"
        );

        let topology: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../scripts/command-migration-topology.json"
        ))
        .expect("checked-in topology must be valid JSON");
        let frontier = topology["frontier"]
            .as_array()
            .expect("topology frontier")
            .iter()
            .map(|entry| entry.as_str().expect("string frontier entry"))
            .collect::<Vec<_>>();
        assert_eq!(PENDING_GROUPS, frontier.as_slice());
    }

    #[test]
    fn boundary_mappings_cover_every_variant() {
        for mode in [
            AppMode::Agent,
            AppMode::Auto,
            AppMode::Yolo,
            AppMode::Plan,
            AppMode::Operate,
        ] {
            let command = to_command_mode(mode);
            assert_eq!(from_command_mode(command), mode);
        }
        for approval in [
            ApprovalMode::Auto,
            ApprovalMode::Bypass,
            ApprovalMode::Suggest,
            ApprovalMode::Never,
        ] {
            let _ = to_command_approval(approval);
        }
        for effort in [
            ReasoningEffort::Off,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
            ReasoningEffort::Ultra,
            ReasoningEffort::Auto,
            ReasoningEffort::Max,
        ] {
            let _ = to_command_effort(effort);
        }
        for currency in [CostCurrency::Usd, CostCurrency::Cny] {
            let command = to_command_currency(currency);
            assert_eq!(from_command_currency(command), currency);
        }
    }

    #[test]
    fn key_to_message_id_resolves_convention_keys_and_rejects_unknown() {
        assert_eq!(
            key_to_message_id("cmd_balance_description"),
            Some(MessageId::CmdBalanceDescription)
        );
        assert_eq!(
            key_to_message_id("cmd_voice_control_description"),
            Some(MessageId::CmdVoiceControlDescription)
        );
        assert_eq!(key_to_message_id("cmd_nonexistent_description"), None);
        assert_eq!(key_to_message_id(""), None);
    }

    #[test]
    fn cost_adapter_delegates_totals_high_water_and_route_receipt_to_app() {
        let mut app = test_app();
        app.cost_currency = CostCurrency::Usd;
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let cost = parts.cost.as_mut().expect("cost facet");
            cost.accrue_cost_estimate(3.0, CommandCurrency::Usd);
            cost.record_turn_cost(
                4.0,
                CommandCurrency::Cny,
                Some("provider=deepseek model=x".to_string()),
            );
            assert_eq!(cost.session_cost_for_currency(CommandCurrency::Usd), 3.0);
            assert_eq!(cost.session_cost_for_currency(CommandCurrency::Cny), 4.0);
        }
        assert_eq!(app.session_cost_for_currency(CostCurrency::Usd), 3.0);
        assert_eq!(app.session_cost_for_currency(CostCurrency::Cny), 4.0);
        assert_eq!(
            app.displayed_session_cost_for_currency(CostCurrency::Usd),
            3.0
        );
        assert!(
            app.session
                .cost_route_receipts
                .contains("provider=deepseek model=x")
        );
    }

    #[test]
    fn session_adapter_delegates_message_and_queue_operations_to_app() {
        let mut app = test_app();
        app.current_session_id = Some("s1".to_string());
        app.session.total_tokens = 42;
        app.queue_message(crate::tui::app::QueuedMessage {
            display: "q".to_string(),
            skill_instruction: None,
            skill_provenance: None,
        });
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let session = parts.session.as_mut().expect("session facet");
            assert_eq!(session.session_id().as_deref(), Some("s1"));
            session.add_message(Message {
                role: Role::User,
                content: vec![],
            });
            assert_eq!(session.api_messages().len(), 1);
            assert_eq!(session.queued_message_count(), 1);
            assert!(session.remove_queued_message(0).is_ok());
            assert!(session.remove_queued_message(5).is_err());
            assert_eq!(session.total_tokens(), 42);
        }
        assert_eq!(app.api_messages.len(), 1);
        assert_eq!(app.queued_message_count(), 0);
    }

    #[test]
    fn model_adapter_delegates_selection_and_route_invalidation_to_app() {
        let mut app = test_app();
        app.last_effective_model = Some("stale-model".to_string());
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let model = parts.model.as_mut().expect("model facet");
            model.set_model_selection("auto".to_string(), Some(to_provider_id("deepseek")));
            assert!(model.auto_model());
            assert_eq!(model.current_model(), "auto");
            assert_eq!(
                model.provider_identity().map(|id| id.0).as_deref(),
                Some("deepseek")
            );
        }
        assert!(app.last_effective_model.is_none());
        assert_eq!(app.provider_identity_for_persistence(), "deepseek");
    }

    #[test]
    fn mode_policy_adapter_delegates_mode_and_shell_policy_to_app() {
        let mut app = test_app();
        app.set_agent_shell_access(false);
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let policy = parts.mode_policy.as_mut().expect("mode facet");
            policy.set_shell_access(true);
            policy.set_mode(CommandMode::Yolo);
            assert!(policy.allow_shell());
            assert_eq!(policy.approval_mode(), CommandApprovalMode::Bypass);
        }
        assert_eq!(
            app.mode,
            AppMode::Agent,
            "YOLO is an Agent compatibility mode"
        );
        assert!(app.yolo);
        assert!(app.allow_shell);
    }

    #[test]
    fn system_prompt_adapter_returns_owned_prompt() {
        let mut app = test_app();
        app.system_prompt = Some(SystemPrompt::Text("system".to_string()));
        let mut bundle = app.command_contexts();
        let parts = bundle.parts();
        assert!(
            parts
                .system_prompt
                .expect("system prompt facet")
                .system_prompt()
                .is_some()
        );
    }

    #[test]
    fn workspace_adapter_returns_path_and_snapshot() {
        let mut app = test_app();
        let expected = app.workspace.clone();
        let mut bundle = app.command_contexts();
        let parts = bundle.parts();
        let workspace = parts.workspace.expect("workspace facet");
        assert_eq!(workspace.workspace(), expected);
        assert!(workspace.work_state_snapshot().is_ok());
    }

    #[test]
    fn envelope_exposes_all_facets_without_app_in_handler_surface() {
        let mut app = test_app();
        let mut bundle = app.command_contexts();
        let parts = bundle.parts();
        assert!(parts.session.is_some());
        assert!(parts.model.is_some());
        assert!(parts.cost.is_some());
        assert!(parts.mode_policy.is_some());
        assert!(parts.system_prompt.is_some());
        assert!(parts.skills.is_some());
        assert!(parts.workspace.is_some());
        assert!(parts.presentation.is_some());
        assert!(parts.media.is_some());
    }

    // -----------------------------------------------------------------------
    // FEAT-018 adapter tests: presentation (D3), media (D4), digest (D5)
    // -----------------------------------------------------------------------

    #[test]
    fn presentation_adapter_resolves_utility_keys_with_english_fallback() {
        let mut app = test_app();
        app.ui_locale = Locale::En;
        let mut bundle = app.command_contexts();
        let mut parts = bundle.parts();
        let presentation = parts.presentation.as_mut().expect("presentation facet");

        // automation_usage has no placeholders.
        let usage = presentation
            .translate("automation_usage", &[])
            .expect("automation usage key");
        assert!(
            usage.contains("/automation"),
            "expected usage text, got {usage}"
        );

        // mcp_recommended_unknown_id needs {recommendations_command}.
        let unknown = presentation
            .translate(
                "mcp_recommended_unknown_id",
                &[("recommendations_command", "/mcp recommendations")],
            )
            .expect("mcp unknown-id key");
        assert!(
            unknown.contains("/mcp recommendations"),
            "expected replacement text, got {unknown}"
        );

        // mcp_recommendation_github needs {endpoint}, {login_command}, {add_command}.
        let github = presentation
            .translate(
                "mcp_recommendation_github",
                &[
                    ("endpoint", "https://api.githubcopilot.com/mcp/"),
                    ("login_command", "/mcp login github"),
                    ("add_command", "/mcp add recommended github"),
                ],
            )
            .expect("github recommendation key");
        assert!(
            github.contains("https://api.githubcopilot.com/mcp/"),
            "{github}"
        );
        assert!(
            !github.contains("{endpoint}"),
            "placeholder must be replaced"
        );
    }

    #[test]
    fn presentation_adapter_rejects_unknown_keys_and_invalid_replacements() {
        let mut app = test_app();
        app.ui_locale = Locale::En;
        let mut bundle = app.command_contexts();
        let mut parts = bundle.parts();
        let presentation = parts.presentation.as_mut().expect("presentation facet");

        let unknown = presentation.translate("no_such_key", &[]);
        assert!(unknown.is_err(), "unknown key must fail safely");
        let err = unknown.unwrap_err();
        assert!(
            !err.contains("no_such_key"),
            "no raw lookup key exposure (D3): {err}"
        );

        // Missing required replacement.
        assert!(
            presentation
                .translate("mcp_recommendation_github", &[])
                .is_err()
        );
        // Extra replacement not present in the template.
        assert!(
            presentation
                .translate("automation_usage", &[("no_such_placeholder", "value")],)
                .is_err()
        );
        // Duplicate replacement names.
        assert!(
            presentation
                .translate(
                    "mcp_recommendation_github",
                    &[
                        ("endpoint", "a"),
                        ("endpoint", "b"),
                        ("login_command", "c"),
                        ("add_command", "d"),
                    ],
                )
                .is_err()
        );
    }

    #[test]
    fn media_adapter_attaches_valid_image_and_preserves_confirm() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let image_path = tmpdir.path().join("photo.png");
        std::fs::write(&image_path, PNG_1X1).expect("write image fixture");

        let mut app = test_app();
        let mut bundle = app.command_contexts();
        let mut parts = bundle.parts();
        let media = parts.media.as_mut().expect("media facet");
        let receipt = media
            .attach_media(&image_path)
            .expect("valid image attaches");
        assert_eq!(receipt.kind, "image");
        assert_eq!(receipt.path, image_path.canonicalize().expect("canonical"));
        assert!(
            app.input.contains("[Attached image:"),
            "composer must contain the attachment reference"
        );
    }

    #[test]
    fn media_adapter_rejects_invalid_media_atomically() {
        let tmpdir = tempfile::TempDir::new().expect("tempdir");

        // Missing path.
        let mut app = test_app();
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let media = parts.media.as_mut().expect("media facet");
            let missing = tmpdir.path().join("missing.png");
            let err = media.attach_media(&missing).unwrap_err();
            assert!(err.contains("Attachment not found"), "{err}");
        }
        assert!(
            app.input.is_empty(),
            "refused attachment must not reach composer"
        );

        // Directory is not a file.
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let media = parts.media.as_mut().expect("media facet");
            let dir = tmpdir.path().to_path_buf();
            let err = media.attach_media(&dir).unwrap_err();
            assert!(err.contains("Attachment is not a file"), "{err}");
        }
        assert!(app.input.is_empty());

        // Unsupported extension.
        std::fs::write(tmpdir.path().join("notes.txt"), b"text").expect("write fixture");
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let media = parts.media.as_mut().expect("media facet");
            let err = media
                .attach_media(&tmpdir.path().join("notes.txt"))
                .unwrap_err();
            assert!(err.contains("Unsupported attachment type"), "{err}");
        }
        assert!(app.input.is_empty());

        // Corrupt image with a valid extension.
        std::fs::write(tmpdir.path().join("bad.png"), b"not an image").expect("write fixture");
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let media = parts.media.as_mut().expect("media facet");
            let err = media
                .attach_media(&tmpdir.path().join("bad.png"))
                .unwrap_err();
            assert!(!err.is_empty(), "corrupt image must fail");
        }
        assert!(app.input.is_empty());
    }

    #[test]
    fn media_adapter_attaches_valid_video_reference() {
        // A real (non-image) media file with a video extension passes the
        // extension gate without byte validation, matching baseline /attach.
        let tmpdir = tempfile::TempDir::new().expect("tempdir");
        let video_path = tmpdir.path().join("clip.mp4");
        std::fs::write(&video_path, b"not a real mp4 but extension-gated").expect("write");

        let mut app = test_app();
        let mut bundle = app.command_contexts();
        let mut parts = bundle.parts();
        let media = parts.media.as_mut().expect("media facet");
        let receipt = media
            .attach_media(&video_path)
            .expect("video path attaches by extension");
        assert_eq!(receipt.kind, "video");
        assert!(app.input.contains("[Attached video:"), "{}", app.input);
    }

    #[test]
    fn workspace_digest_adapter_preserves_no_active_and_failure_semantics() {
        let mut app = test_app();
        app.runtime_services.work = None;
        {
            let mut bundle = app.command_contexts();
            let mut parts = bundle.parts();
            let workspace = parts.workspace.as_mut().expect("workspace facet");
            assert_eq!(
                workspace.operation_digest().expect("no-runtime digest"),
                "No active operations or to-do items."
            );
        }
    }

    #[test]
    fn bundle_construction_performs_no_eager_work() {
        let mut app = test_app();
        let input_before = app.input.clone();
        {
            let mut bundle = app.command_contexts();
            let parts = bundle.parts();
            // Merely constructing the bundle must not mutate composer state or
            // perform capability work; the adapters only run on method calls.
            let _ = parts.media.is_some();
            let _ = parts.presentation.is_some();
            let _ = parts.memory.is_some();
        }
        assert_eq!(app.input, input_before, "no eager composer mutation");
    }

    // -----------------------------------------------------------------------
    // FEAT-019: memory adapter mappings (D6/D9)
    // -----------------------------------------------------------------------

    /// App with an isolated temp memory file; memory feature enabled or not.
    fn memory_test_app(tmpdir: &TempDir, use_memory: bool) -> App {
        let options = crate::test_support::test_tui_options(tmpdir.path());
        let options = crate::tui::app::TuiOptions {
            memory_path: tmpdir.path().join("memory.md"),
            use_memory,
            ..options
        };
        crate::test_support::test_app_with_options(options)
    }

    /// Give a temp workspace a git origin so workspace identity resolves.
    fn git_origin(workspace: &Path) {
        let init = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["init", "-q"])
            .status()
            .unwrap();
        assert!(init.success(), "git init must succeed");
        let remote = std::process::Command::new("git")
            .arg("-C")
            .arg(workspace)
            .args(["remote", "add", "origin", "https://example.test/repo.git"])
            .status()
            .unwrap();
        assert!(remote.success(), "git remote add must succeed");
    }

    #[test]
    fn memory_adapter_maps_path_and_enablement() {
        let tmp = TempDir::new().unwrap();
        let mut enabled = memory_test_app(&tmp, true);
        let mut bundle = enabled.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet must be present");
        assert_eq!(memory.memory_path(), tmp.path().join("memory.md"));
        assert!(memory.memory_enabled());

        let mut disabled = memory_test_app(&tmp, false);
        let mut bundle = disabled.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet must be present");
        assert!(!memory.memory_enabled());
    }

    #[test]
    fn memory_adapter_status_and_path_map_native_store() {
        let tmp = TempDir::new().unwrap();
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet");

        // Fallback root derivation mirrors the legacy handler: a plain
        // `memory.md` file is not a native global source, so the root is the
        // sibling `memory` directory.
        let status = memory.status().expect("status");
        assert_eq!(status.root, tmp.path().join("memory"));
        assert_eq!(
            status.source,
            tmp.path().join("memory").join("global").join("MEMORY.md")
        );
        assert_eq!(
            status.index,
            tmp.path().join("memory").join("index.sqlite3")
        );
        assert_eq!(memory.path().expect("path"), tmp.path().join("memory"));
    }

    #[test]
    fn memory_adapter_workspace_identity_resolves_and_preserves_errors() {
        let tmp = TempDir::new().unwrap();
        git_origin(tmp.path());
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet");
        // A git origin resolves to a stable workspace identity (sha256 digest).
        let id = memory.workspace_id(tmp.path()).expect("workspace id");
        assert!(!id.is_empty());
        assert_eq!(id, memory.workspace_id(tmp.path()).expect("stable id"));

        // A plain directory without git origin preserves the established error.
        let plain = TempDir::new().unwrap();
        let err = memory
            .workspace_id(plain.path())
            .expect_err("missing origin");
        assert_eq!(
            err,
            "workspace memory requires a git repository with an origin"
        );
    }

    #[test]
    fn memory_adapter_search_remember_get_export_reindex_work() {
        let tmp = TempDir::new().unwrap();
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet");

        // Global remember produces a portable remembered location.
        let remembered = memory
            .remember(MemoryRememberTarget::Global, "alpha note")
            .expect("remember global");
        assert!(remembered.source.ends_with("global/MEMORY.md"));
        assert_eq!(remembered.line_start, 2);

        // Workspace remember targets the workspace scope with the typed id.
        git_origin(tmp.path());
        let workspace_id = memory.workspace_id(tmp.path()).expect("id");
        let workspace_note = memory
            .remember(
                MemoryRememberTarget::Workspace { workspace_id },
                "workspace-only note",
            )
            .expect("remember workspace");
        assert!(
            workspace_note
                .source
                .to_string_lossy()
                .contains("workspace")
        );

        // Search finds workspace-scoped content only for the given workspace.
        let hits = memory
            .search(tmp.path(), "workspace-only", 10)
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("workspace-only note"));
        assert_eq!(hits[0].line_start, 2);
        // Empty results stay a typed empty vec, never an error.
        assert!(
            memory
                .search(tmp.path(), "zzz-no-match", 10)
                .expect("empty search")
                .is_empty()
        );

        // Get distinguishes found from not-found (first rowid is 1).
        match memory.get(tmp.path(), 1) {
            Ok(MemoryGetOutcome::Found(hit)) => assert!(!hit.text.is_empty()),
            other => panic!("expected found entry, got {other:?}"),
        }
        assert_eq!(
            memory.get(tmp.path(), 999_999).expect("get"),
            MemoryGetOutcome::NotFound
        );

        // Export carries the document; reindex reports the typed count.
        let exported = memory.export().expect("export");
        assert!(exported.content.contains("alpha note"));
        assert!(exported.content.contains("workspace-only note"));
        assert!(memory.reindex().expect("reindex").entry_count >= 1);
    }

    #[test]
    fn memory_adapter_import_distinguishes_imported_from_skipped() {
        let tmp = TempDir::new().unwrap();
        let legacy = tmp.path().join("memory.md");
        std::fs::write(&legacy, "# legacy\n\n- imported line").unwrap();
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet");

        let imported = memory.import().expect("import");
        let MemoryImportOutcome::Imported { destination } = imported else {
            panic!("first import must be imported");
        };
        assert!(destination.ends_with("global/MEMORY.md"));

        // Idempotent: an existing global source reports skipped.
        assert_eq!(
            memory.import().expect("second"),
            MemoryImportOutcome::Skipped
        );
    }

    #[test]
    fn memory_adapter_deletes_are_scoped_and_preserve_other_memory() {
        let tmp = TempDir::new().unwrap();
        git_origin(tmp.path());
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet");

        memory
            .remember(MemoryRememberTarget::Global, "keep global")
            .expect("global");
        let workspace_id = memory.workspace_id(tmp.path()).expect("id");
        memory
            .remember(
                MemoryRememberTarget::Workspace { workspace_id },
                "remove workspace",
            )
            .expect("workspace");

        // Workspace deletion removes only the workspace scope.
        memory
            .delete_workspace(tmp.path())
            .expect("workspace delete");
        assert!(
            memory
                .search(tmp.path(), "remove workspace", 10)
                .expect("search")
                .is_empty()
        );
        assert_eq!(
            memory.search(tmp.path(), "keep global", 10).unwrap().len(),
            1
        );

        // Global deletion removes the global scope but keeps the workspace one.
        memory
            .remember(
                MemoryRememberTarget::Workspace {
                    workspace_id: memory.workspace_id(tmp.path()).expect("id"),
                },
                "workspace survivor",
            )
            .expect("workspace again");
        memory
            .delete(MemoryDeleteScope::Global)
            .expect("global delete");
        assert!(
            memory
                .search(tmp.path(), "keep global", 10)
                .expect("search")
                .is_empty()
        );
        assert_eq!(
            memory
                .search(tmp.path(), "workspace survivor", 10)
                .unwrap()
                .len(),
            1
        );

        // All deletion removes every scope.
        memory.delete(MemoryDeleteScope::All).expect("all delete");
        assert!(
            memory
                .search(tmp.path(), "workspace survivor", 10)
                .expect("search")
                .is_empty()
        );
    }

    #[test]
    fn memory_adapter_preserves_workspace_delete_error_text() {
        let tmp = TempDir::new().unwrap();
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();
        let memory = bundle.parts().memory.expect("memory facet");
        let err = memory
            .delete_workspace(tmp.path())
            .expect_err("missing origin");
        assert_eq!(
            err,
            "workspace memory requires a git repository with an origin"
        );
    }

    #[test]
    fn envelope_exposes_only_declared_capabilities() {
        let tmp = TempDir::new().unwrap();
        let mut app = memory_test_app(&tmp, true);
        let mut bundle = app.command_contexts();

        // Memory-only: memory present, workspace/session absent.
        let parts = bundle.contexts(CommandCapabilities::MEMORY).into_parts();
        assert!(parts.memory.is_some());
        assert!(parts.workspace.is_none());
        assert!(parts.session.is_none());

        // Workspace-only: memory absent.
        let parts = bundle.contexts(CommandCapabilities::WORKSPACE).into_parts();
        assert!(parts.workspace.is_some());
        assert!(parts.memory.is_none());

        // Workspace | MEMORY: both present, presentation/media absent.
        let parts = bundle
            .contexts(CommandCapabilities::WORKSPACE.union(CommandCapabilities::MEMORY))
            .into_parts();
        assert!(parts.workspace.is_some());
        assert!(parts.memory.is_some());
        assert!(parts.presentation.is_none());
        assert!(parts.media.is_none());

        // Unrelated capability: memory absent.
        let parts = bundle.contexts(CommandCapabilities::SESSION).into_parts();
        assert!(parts.session.is_some());
        assert!(parts.memory.is_none());
    }

    // ------------------------------------------------------------------
    // FEAT-020 plugin adapter tests
    // ------------------------------------------------------------------

    fn plugin_test_app(tmpdir: &TempDir) -> App {
        let options = crate::test_support::test_tui_options(tmpdir.path());
        let mut app = crate::test_support::test_app_with_options(options);
        app.ui_locale = Locale::En;
        app
    }

    /// Write a minimal plugin bundle into the temp workspace's
    /// `.codewhale/plugins` so the adapter can read real host data.
    fn write_demo_bundle(root: &Path) {
        let bundle = root.join(".codewhale/plugins/demo");
        std::fs::create_dir_all(bundle.join("skills/hello")).unwrap();
        std::fs::write(
            bundle.join("plugin.toml"),
            "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\ndescription = \"Import spreadsheet data safely\"\n[skills]\npath = \"skills\"\n",
        )
        .unwrap();
        std::fs::write(
            bundle.join("skills/hello/SKILL.md"),
            "---\nname: hello\ndescription: hello\n---\nbody\n",
        )
        .unwrap();
    }

    #[test]
    fn plugin_adapter_summaries_and_detail_project_host_data() {
        let tmp = TempDir::new().unwrap();
        write_demo_bundle(tmp.path());
        let mut app = plugin_test_app(&tmp);
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        app.plugin_registry = discovery.registry_for_workspace(tmp.path());
        let mut bundle = app.command_contexts();
        let mut parts = bundle
            .contexts(
                CommandCapabilities::WORKSPACE
                    .union(CommandCapabilities::PRESENTATION)
                    .union(CommandCapabilities::PLUGIN),
            )
            .into_parts();
        let plugin = parts.plugin.as_deref_mut().unwrap();

        let summaries = plugin.summaries().unwrap();
        assert!(!summaries.is_empty());
        let summary = summaries
            .iter()
            .find(|s| s.name == "demo")
            .expect("demo summary");
        assert_eq!(summary.compatibility, "full");
        assert!(
            summary.inventory.starts_with("skills=1"),
            "inventory summary: {}",
            summary.inventory
        );

        let detail = plugin.detail("demo").unwrap();
        assert_eq!(detail.name, "demo");
        assert_eq!(detail.version, "1.0.0");
        assert_eq!(detail.skills, vec!["demo:hello"]);
        assert_eq!(detail.trust_status, "not-reviewed");

        // Unknown selector fails safely.
        assert!(plugin.detail("nope").is_err());
        // Registry diagnostics empty for a clean bundle.
        assert!(plugin.registry_diagnostics().is_empty());
        assert!(plugin.validation_is_clean());
    }

    #[test]
    fn plugin_adapter_registry_mutations_and_suggest_are_behavior_faithful() {
        let tmp = TempDir::new().unwrap();
        write_demo_bundle(tmp.path());
        let mut app = plugin_test_app(&tmp);
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        app.plugin_registry = discovery.registry_for_workspace(tmp.path());
        // Capture the review token before borrowing the mutable facet.
        let demo = app.plugin_registry.get("demo").unwrap();
        let token = format!("{}.{}", demo.content_hash, demo.capability_hash);

        let mut bundle = app.command_contexts();
        let mut parts = bundle.contexts(CommandCapabilities::PLUGIN).into_parts();
        let plugin = parts.plugin.as_deref_mut().unwrap();

        // Read-only suggest does not mutate anything.
        let before = plugin.len();
        let _ = plugin.suggest("spreadsheet");
        assert_eq!(plugin.len(), before);
        assert_eq!(plugin.summaries().unwrap().len(), before);

        // enable on an untrusted bundle routes to review (safe error), not a mutation.
        let err = plugin.enable("demo").unwrap_err();
        assert!(err.contains("requires review"));

        // trust with a wrong token fails safely.
        assert!(plugin.trust("demo", "bogus.token").is_err());

        // trust with the exact token succeeds.
        plugin.trust("demo", &token).unwrap();
        assert!(plugin.detail("demo").unwrap().trusted);

        // enable now succeeds.
        plugin.enable("demo").unwrap();
        assert!(plugin.detail("demo").unwrap().enabled);

        // disable clears active skill and marks disabled.
        plugin.disable("demo").unwrap();
        assert!(!plugin.detail("demo").unwrap().enabled);

        // revoke_trust flips trust back off.
        plugin.revoke_trust("demo").unwrap();
        assert!(!plugin.detail("demo").unwrap().trusted);
    }

    #[test]
    fn plugin_adapter_exposure_is_exactly_declared_capabilities() {
        let tmp = TempDir::new().unwrap();
        let mut app = plugin_test_app(&tmp);
        let mut bundle = app.command_contexts();

        // Plugin-only: plugin present, everything else absent.
        let parts = bundle.contexts(CommandCapabilities::PLUGIN).into_parts();
        assert!(parts.plugin.is_some());
        assert!(parts.workspace.is_none());
        assert!(parts.presentation.is_none());
        assert!(parts.memory.is_none());

        // Workspace | PRESENTATION | PLUGIN: all three present, media/memory absent.
        let parts = bundle
            .contexts(
                CommandCapabilities::WORKSPACE
                    .union(CommandCapabilities::PRESENTATION)
                    .union(CommandCapabilities::PLUGIN),
            )
            .into_parts();
        assert!(parts.plugin.is_some());
        assert!(parts.workspace.is_some());
        assert!(parts.presentation.is_some());
        assert!(parts.media.is_none());
        assert!(parts.memory.is_none());

        // Undeclared capability: plugin absent.
        let parts = bundle.contexts(CommandCapabilities::SESSION).into_parts();
        assert!(parts.session.is_some());
        assert!(parts.plugin.is_none());
    }
}
