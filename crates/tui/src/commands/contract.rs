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
//! `CommandContexts` holds seven independently borrowed facet objects, while
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
    CommandApprovalState, CommandCostContext, CommandMediaContext, CommandModePolicyContext,
    CommandModelContext, CommandPresentationContext, CommandProjectContext, CommandSessionContext,
    CommandSkillGroupContext, CommandSkillsContext, CommandSystemPromptContext,
    CommandWorkspaceContext, MediaAttachmentReceipt, ProjectGoalState, ProjectGoalStatus,
    ProjectShareProjection, RemoteRegistryOutcome, RemoteSkillEntry, ReviewOutcome,
    SkillActivationError, SkillActivationOutcome, SkillBundledTier, SkillEntry,
    SkillMutationOutcome, SkillMutationReceipt, SkillRecommendation, SkillRegistryProjection,
    SkillSourceKind, SkillSyncEntry, SkillSyncOutcome, SkillTargetScope, SnapshotEntry,
};
use codewhale_command_contract::handler::CommandContexts;
#[cfg(test)]
use codewhale_command_contract::handler::ContextParts;
use codewhale_command_contract::types::{
    CommandApprovalMode, CommandCurrency, CommandMode, CommandProviderId, CommandReasoningEffort,
};
use codewhale_config::AppMode;
use codewhale_core::request::{Message, SystemPrompt};
use codewhale_execpolicy::ApprovalMode;

use crate::localization::{MessageId, tr};
use crate::network_policy::NetworkPolicy;
use crate::pricing::CostCurrency;
use crate::tui::app::{App, ReasoningEffort};
use crate::tui::history::HistoryCell;

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
    &["config", "core", "debug", "memory", "plugins", "session"];

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
        "cmd_dispatch_description" => MessageId::CmdDispatchDescription,
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
/// The envelope needs seven independently borrowed facet objects, while the
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
        let Some(message_id) =
            key_to_utility_message_id(key).or_else(|| key_to_project_message_id(key))
        else {
            return Err("unknown translation key".to_string());
        };
        let locale = self.host.app.borrow().ui_locale;
        let template = tr(locale, message_id);
        apply_named_replacements(&template, replacements)
            .ok_or_else(|| "invalid translation replacement contract".to_string())
    }
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

/// Resolve a stable project message key to the current catalog id (FEAT-021 D5).
///
/// Only `/goal` uses runtime translations (`GoalControlAccepted`,
/// `GoalStatusIdleHint`); all four description keys resolve through the
/// metadata bridge (`key_to_message_id`) and do not require the presentation
/// facet.
pub(crate) fn key_to_project_message_id(key: &str) -> Option<MessageId> {
    Some(match key {
        "goal_control_accepted" => MessageId::GoalControlAccepted,
        "goal_status_idle_hint" => MessageId::GoalStatusIdleHint,
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

// ---------------------------------------------------------------------------
// Project host adapter (FEAT-021 D1/D3)
// ---------------------------------------------------------------------------

/// Concrete TUI host mapping for the project command group (FEAT-021 D1/D3).
///
/// The only place that touches `App` goal/share/LSP state, `config::config`
/// (cross-group LSP bridge), and the session manager. Every method borrows
/// `App` for one call and converts host values to portable contract values
/// before returning; the `/init` workspace path flows through the existing
/// `WORKSPACE` facet (D2), so no init-specific method exists here.
pub(crate) struct ProjectAdapter<'a> {
    host: SharedCommandHost<'a>,
}

/// Map the TUI-owned goal status onto the portable project status.
fn portable_goal_status(status: crate::tools::goal::GoalStatus) -> ProjectGoalStatus {
    match status {
        crate::tools::goal::GoalStatus::Active => ProjectGoalStatus::Active,
        crate::tools::goal::GoalStatus::Paused => ProjectGoalStatus::Paused,
        crate::tools::goal::GoalStatus::Complete => ProjectGoalStatus::Complete,
        crate::tools::goal::GoalStatus::Blocked => ProjectGoalStatus::Blocked,
    }
}

/// Map the durable session goal status onto the portable project status.
fn portable_session_goal_status(
    status: crate::session_manager::SessionGoalStatus,
) -> ProjectGoalStatus {
    match status {
        crate::session_manager::SessionGoalStatus::Active => ProjectGoalStatus::Active,
        crate::session_manager::SessionGoalStatus::Paused => ProjectGoalStatus::Paused,
        crate::session_manager::SessionGoalStatus::Complete => ProjectGoalStatus::Complete,
        crate::session_manager::SessionGoalStatus::Blocked => ProjectGoalStatus::Blocked,
    }
}

impl CommandProjectContext for ProjectAdapter<'_> {
    fn lsp_enabled(&self) -> bool {
        self.host.app.borrow().lsp_enabled
    }

    fn lsp_set(&mut self, enabled: bool) -> Result<(), String> {
        // Cross-group LSP behavior stays host-side (D3): the adapter owns the
        // `config::config::lsp_command` invocation. The portable handler
        // composes the byte-identical user-facing message from the typed
        // state, so the formatted result is intentionally not forwarded.
        let mut app = self.host.app.borrow_mut();
        let arg = if enabled { "on" } else { "off" };
        let _ = crate::commands::groups::config::config::lsp_command(&mut app, Some(arg));
        Ok(())
    }

    fn share_projection(&self) -> ProjectShareProjection {
        let app = self.host.app.borrow();
        ProjectShareProjection {
            history_is_empty: app.history.is_empty(),
            history_len: app.history.len(),
            model: app.model.clone(),
            mode_label: app.mode.label().to_string(),
        }
    }

    fn goal_state(&self) -> ProjectGoalState {
        let app = self.host.app.borrow();
        let pending_controls = !app.pending_goal_controls.is_empty();
        let last_known = app.last_known_goal_state.as_ref();
        ProjectGoalState {
            objective: app.goal.objective.clone(),
            status: portable_goal_status(app.goal.status),
            pause_reason: app
                .goal
                .pause_reason
                .map(|reason| reason.label().to_string()),
            started_at_elapsed_seconds: app.goal.started_at.map(|t| t.elapsed().as_secs()),
            time_used_seconds: app.goal.time_used_seconds,
            token_budget: app.goal.token_budget,
            tokens_used: app.goal.tokens_used,
            session_total_tokens: app.session.total_conversation_tokens,
            continuation_count: app.goal.continuation_count,
            pending_controls,
            last_known_objective: last_known.map(|goal| goal.objective.clone()),
            last_known_status: last_known.map(|goal| portable_session_goal_status(goal.status)),
            conversation_present: !app.api_messages.is_empty(),
            is_loading: app.is_loading,
            goal_continuation_waiting: app.goal_continuation_waiting,
        }
    }
}

// ---------------------------------------------------------------------------
// Skill group adapter (FEAT-022 D1/D3)
// ---------------------------------------------------------------------------

/// The single new skills-specific host adapter.
///
/// Owns every concrete skills touch: `App` skill state, `crate::skills`
/// discovery/mutation/install/recommend services, `crate::plugins` authority
/// verification, `SnapshotRepo`, config/network policy, and the async bridge
/// (`tokio::task::block_in_place`). Portable handlers never name these
/// subsystems (D3); every method returns portable contract values or safe
/// error text (D1).
pub(crate) struct SkillGroupAdapter<'a> {
    host: SharedCommandHost<'a>,
}

/// Bridge a sync slash-command handler back into the async ecosystem.
///
/// We are on the TUI's thread, which is part of the multi-threaded runtime;
/// `block_in_place` + `Handle::current().block_on` bridges sync handlers back
/// into the async ecosystem. Mirrors `groups/skills/skills.rs::run_async`;
/// the legacy copy is removed in Phase 4 when the handlers are ported.
fn run_async<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

/// Read the active config knobs for the installer (network policy, max size,
/// registry URL). `Config::load` is cheap and `App` does not carry a `Config`;
/// on parse failure we fall back to defaults so the user still gets a
/// network-gated install rather than a silent crash. Mirrors
/// `groups/skills/skills.rs::installer_settings`.
fn installer_settings() -> (NetworkPolicy, u64, String) {
    let cfg = crate::config::Config::load(None, None).unwrap_or_default();
    let network = cfg
        .network
        .clone()
        .map(|policy| policy.into_runtime())
        .unwrap_or_default();
    let skills_cfg = cfg.skills.as_ref();
    let max_size = skills_cfg
        .and_then(|s| s.max_install_size_bytes)
        .unwrap_or(crate::skills::install::DEFAULT_MAX_SIZE_BYTES);
    let registry_url = skills_cfg
        .and_then(|s| s.registry_url.clone())
        .unwrap_or_else(|| crate::skills::install::DEFAULT_REGISTRY_URL.to_string());
    (network, max_size, registry_url)
}

/// Inspect an anyhow chain and surface a one-line hint pointing at the most
/// common cause of a registry fetch failure (DNS, refused, TLS, HTTP status,
/// timeout). Mirrors `groups/skills/skills.rs::registry_fetch_error_hint`.
fn registry_fetch_error_hint(err: &anyhow::Error) -> Option<&'static str> {
    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("dns")
        || msg.contains("name resolution")
        || msg.contains("getaddrinfo")
        || msg.contains("nodename nor servname")
    {
        Some(
            "Hint: DNS lookup failed. Check internet/DNS connectivity, or override the registry URL in [skills] of ~/.codewhale/config.toml.",
        )
    } else if msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("connection aborted")
    {
        Some(
            "Hint: connection refused/reset. The registry host may be unreachable from this network (corporate proxy, firewall, offline).",
        )
    } else if msg.contains("tls")
        || msg.contains("certificate")
        || msg.contains("ssl")
        || msg.contains("handshake")
    {
        Some(
            "Hint: TLS handshake failed. The system trust store may be missing the registry's CA, or a TLS-intercepting proxy is rewriting the certificate.",
        )
    } else if msg.contains(" 404") || msg.contains("not found") {
        Some(
            "Hint: registry URL returned 404. Verify the registry URL in [skills] of ~/.codewhale/config.toml.",
        )
    } else if msg.contains(" 401") || msg.contains(" 403") || msg.contains("forbidden") {
        Some(
            "Hint: registry returned an auth error. The registry may require credentials or have been moved.",
        )
    } else if msg.contains(" 429") || msg.contains("rate limit") || msg.contains("too many") {
        Some("Hint: rate-limited by the registry. Try again in a moment.")
    } else if msg.contains("timed out") || msg.contains("timeout") {
        Some("Hint: request timed out. Network may be slow or the registry host may be down.")
    } else {
        None
    }
}

/// Append the actionable hint to a registry fetch error. Mirrors
/// `groups/skills/skills.rs::format_registry_error`.
fn format_registry_error(prefix: &str, err: &anyhow::Error) -> String {
    let mut out = format!("{prefix}: {err:#}");
    if let Some(hint) = registry_fetch_error_hint(err) {
        out.push_str("\n\n");
        out.push_str(hint);
    }
    out
}

/// Discover the enabled visible skills for the current App state.
fn discover_visible(app: &App) -> crate::skills::SkillRegistry {
    crate::skills::discover_for_workspace_and_dir_with_mode_and_plugins(
        &app.workspace,
        &app.skills_dir,
        crate::skills::SkillDiscoveryMode::from_codewhale_only(app.skills_scan_codewhale_only),
        Some(app.plugin_registry.as_ref()),
    )
    .into_enabled()
}

/// Map a TUI skill to its portable projection entry.
fn portable_skill_entry(skill: &crate::skills::Skill) -> SkillEntry {
    let source = match &skill.source {
        crate::skills::SkillSource::Native => SkillSourceKind::Native,
        crate::skills::SkillSource::Plugin {
            plugin_id,
            plugin_name,
            ..
        } => SkillSourceKind::Plugin {
            plugin_name: plugin_name.clone(),
            plugin_id: plugin_id.clone(),
        },
    };
    let path = match &skill.source {
        crate::skills::SkillSource::Native => Some(skill.path.display().to_string()),
        crate::skills::SkillSource::Plugin { .. } => None,
    };
    let bundled_tier = crate::skills::bundled_skill_tier(&skill.name).map(|tier| match tier {
        crate::skills::BundledSkillTier::CoreAgentic => SkillBundledTier::CoreAgentic,
        crate::skills::BundledSkillTier::FormatTooling => SkillBundledTier::FormatTooling,
    });
    SkillEntry {
        name: skill.name.clone(),
        description: skill.description.clone(),
        source,
        path,
        bundled_tier,
    }
}

/// Map a TUI mutation receipt to its portable receipt.
fn portable_mutation_receipt(
    receipt: &crate::skills::mutation::SkillMutationReceipt,
) -> SkillMutationReceipt {
    use crate::skills::mutation::SkillMutationOutcome as TuiOutcome;
    let outcome = match &receipt.outcome {
        TuiOutcome::Installed => SkillMutationOutcome::Installed,
        TuiOutcome::Updated => SkillMutationOutcome::Updated,
        TuiOutcome::NoChange => SkillMutationOutcome::NoChange,
        TuiOutcome::Removed => SkillMutationOutcome::Removed,
        TuiOutcome::Trusted => SkillMutationOutcome::Trusted,
        TuiOutcome::Imported => SkillMutationOutcome::Imported,
        TuiOutcome::AlreadyPresent => SkillMutationOutcome::AlreadyPresent,
        TuiOutcome::NeedsApproval(host) => SkillMutationOutcome::NeedsApproval(host.clone()),
        TuiOutcome::NetworkDenied(host) => SkillMutationOutcome::NetworkDenied(host.clone()),
    };
    SkillMutationReceipt {
        name: receipt.name.clone(),
        safe_target_path: receipt.safe_target_path.clone(),
        outcome,
    }
}

/// Map a portable target scope to the TUI scope.
fn portable_scope(
    scope: Option<SkillTargetScope>,
) -> Option<crate::skills::mutation::SkillTargetScope> {
    use crate::skills::mutation::SkillTargetScope as TuiScope;
    scope.map(|s| match s {
        SkillTargetScope::Project => TuiScope::Project,
        SkillTargetScope::Global => TuiScope::Global,
    })
}

/// Map a curated registry document to portable entries.
fn portable_registry_entries(
    doc: &crate::skills::install::RegistryDocument,
) -> Vec<RemoteSkillEntry> {
    doc.skills
        .iter()
        .map(|(name, entry)| RemoteSkillEntry {
            name: name.clone(),
            description: entry.description.clone(),
            source: entry.source.clone(),
        })
        .collect()
}

/// Message shown when a network-policy host requires approval. Moved
/// verbatim from `groups/skills/skills.rs`; the legacy copy is removed in
/// Phase 4. Rendered by the portable handler from the typed outcome.
fn needs_approval_message(host: &str) -> String {
    format!(
        "Network policy requires approval for {host}.\n\
         Add it to your allow list with `/network allow {host}` (or set [network].default = \"allow\" in ~/.codewhale/config.toml), then retry."
    )
}

/// Message shown when a network-policy host is denied. Moved verbatim from
/// `groups/skills/skills.rs`; the legacy copy is removed in Phase 4.
fn network_denied_message(host: &str) -> String {
    format!(
        "Network policy denied access to {host}.\n\
         Remove the deny entry from ~/.codewhale/config.toml under [network] or contact your administrator."
    )
}

impl CommandSkillGroupContext for SkillGroupAdapter<'_> {
    fn skill_registry_projection(&self) -> SkillRegistryProjection {
        let app = self.host.app.borrow();
        let mode =
            crate::skills::SkillDiscoveryMode::from_codewhale_only(app.skills_scan_codewhale_only);
        let dirs = crate::skills::skill_directories_for_workspace_and_dir(
            &app.workspace,
            &app.skills_dir,
            mode,
        );
        let registry = discover_visible(&app);
        let mode_label = match mode {
            crate::skills::SkillDiscoveryMode::Compatible => "compatible",
            crate::skills::SkillDiscoveryMode::CodeWhaleOnly => "codewhale-only",
        };
        SkillRegistryProjection {
            workspace: app.workspace.display().to_string(),
            skills_dir: app.skills_dir.display().to_string(),
            mode_label: mode_label.to_string(),
            dirs: dirs.iter().map(|dir| dir.display().to_string()).collect(),
            entries: registry.list().iter().map(portable_skill_entry).collect(),
            warnings: registry.warnings().to_vec(),
            total: registry.len(),
        }
    }

    fn activate_skill(
        &mut self,
        name: &str,
    ) -> Result<SkillActivationOutcome, SkillActivationError> {
        let registry = {
            let app = self.host.app.borrow();
            discover_visible(&app)
        };
        if let Some(skill) = registry.get(name) {
            let plugin_provenance = match &skill.source {
                crate::skills::SkillSource::Native => None,
                crate::skills::SkillSource::Plugin { authority, .. } => {
                    if let Err(reason) = crate::plugins::registry::verify_plugin_component_authority(
                        authority,
                        crate::plugins::activation::PluginActivationCapability::Skills,
                    ) {
                        return Err(SkillActivationError::PluginRejected {
                            name: skill.name.clone(),
                            reason,
                        });
                    }
                    Some(authority.as_ref().clone())
                }
            };
            let skill = skill.clone();
            let instruction = format!(
                "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
                skill.name, skill.body
            );
            let mut app = self.host.app.borrow_mut();
            app.add_message(HistoryCell::System {
                content: format!("Activated skill: {}\n\n{}", skill.name, skill.description),
            });
            app.active_skill = Some(instruction);
            app.active_skill_provenance = plugin_provenance;
            Ok(SkillActivationOutcome {
                name: skill.name,
                description: skill.description,
            })
        } else {
            let available: Vec<String> = registry.list().iter().map(|s| s.name.clone()).collect();
            Err(SkillActivationError::NotFound {
                requested: name.to_string(),
                available,
                warnings: registry.warnings().to_vec(),
            })
        }
    }

    fn install_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        spec: &str,
    ) -> Result<SkillMutationReceipt, String> {
        use crate::skills::mutation::{MutationContext, SkillMutationRequest};
        let source = match crate::skills::install::InstallSource::parse(spec) {
            Ok(source) => source,
            Err(err) => return Err(format!("Invalid install source: {err}")),
        };
        let target =
            portable_scope(scope).unwrap_or(crate::skills::mutation::SkillTargetScope::Global);
        let workspace = self.host.app.borrow().workspace.clone();
        let home = crate::config::effective_home_dir();
        let (network, max_size, registry_url) = installer_settings();
        let outcome = run_async(async move {
            let ctx = MutationContext {
                workspace: &workspace,
                home: home.as_deref(),
                configured_skills_dir: None,
                network: &network,
                max_size,
                registry_url: &registry_url,
            };
            crate::skills::mutation::execute(
                SkillMutationRequest::InstallRemote { source, target },
                &ctx,
            )
            .await
        });
        match outcome {
            Ok(receipt) => Ok(portable_mutation_receipt(&receipt)),
            Err(err) => Err(format!("Install failed: {err:#}")),
        }
    }

    fn update_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        name: &str,
    ) -> Result<SkillMutationReceipt, String> {
        use crate::skills::mutation::{MutationContext, SkillMutationRequest};
        let workspace = self.host.app.borrow().workspace.clone();
        let home = crate::config::effective_home_dir();
        let (network, max_size, registry_url) = installer_settings();
        let owned_name = name.to_string();
        let scope = portable_scope(scope);
        let outcome = run_async(async move {
            let ctx = MutationContext {
                workspace: &workspace,
                home: home.as_deref(),
                configured_skills_dir: None,
                network: &network,
                max_size,
                registry_url: &registry_url,
            };
            crate::skills::mutation::execute(
                SkillMutationRequest::UpdateByName {
                    name: owned_name,
                    scope,
                    expected_digest: None,
                },
                &ctx,
            )
            .await
        });
        match outcome {
            Ok(receipt) => Ok(portable_mutation_receipt(&receipt)),
            Err(err) => Err(format!("Update failed: {err:#}")),
        }
    }

    fn uninstall_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        name: &str,
    ) -> Result<SkillMutationReceipt, String> {
        use crate::skills::mutation::{MutationContext, SkillMutationRequest};
        let workspace = self.host.app.borrow().workspace.clone();
        let home = crate::config::effective_home_dir();
        let (network, max_size, registry_url) = installer_settings();
        let ctx = MutationContext {
            workspace: &workspace,
            home: home.as_deref(),
            configured_skills_dir: None,
            network: &network,
            max_size,
            registry_url: &registry_url,
        };
        match crate::skills::mutation::execute_sync(
            SkillMutationRequest::RemoveByName {
                name: name.to_string(),
                scope: portable_scope(scope),
                expected_digest: None,
            },
            &ctx,
        ) {
            Ok(receipt) => Ok(portable_mutation_receipt(&receipt)),
            Err(err) => Err(format!("Uninstall failed: {err:#}")),
        }
    }

    fn trust_skill(
        &mut self,
        scope: Option<SkillTargetScope>,
        name: &str,
    ) -> Result<SkillMutationReceipt, String> {
        use crate::skills::mutation::{MutationContext, SkillMutationRequest};
        let workspace = self.host.app.borrow().workspace.clone();
        let home = crate::config::effective_home_dir();
        let (network, max_size, registry_url) = installer_settings();
        let ctx = MutationContext {
            workspace: &workspace,
            home: home.as_deref(),
            configured_skills_dir: None,
            network: &network,
            max_size,
            registry_url: &registry_url,
        };
        match crate::skills::mutation::execute_sync(
            SkillMutationRequest::TrustByName {
                name: name.to_string(),
                scope: portable_scope(scope),
                expected_digest: None,
            },
            &ctx,
        ) {
            Ok(receipt) => Ok(portable_mutation_receipt(&receipt)),
            Err(err) => Err(format!("Trust failed: {err:#}")),
        }
    }

    fn fetch_remote_registry(&mut self) -> Result<RemoteRegistryOutcome, String> {
        let (network, _max_size, registry_url) = installer_settings();
        let registry = run_async(async move {
            crate::skills::install::fetch_registry(&network, &registry_url).await
        });
        match registry {
            Ok(crate::skills::install::RegistryFetchResult::Loaded(doc)) => {
                Ok(RemoteRegistryOutcome::Loaded {
                    entries: portable_registry_entries(&doc),
                })
            }
            Ok(crate::skills::install::RegistryFetchResult::NeedsApproval(host)) => {
                Ok(RemoteRegistryOutcome::NeedsApproval(host))
            }
            Ok(crate::skills::install::RegistryFetchResult::Denied(host)) => {
                Ok(RemoteRegistryOutcome::Denied(host))
            }
            Err(err) => Err(format_registry_error("Failed to fetch registry", &err)),
        }
    }

    fn recommend_skills(&mut self, task: &str) -> Result<Vec<SkillRecommendation>, String> {
        let (network, _max_size, registry_url) = installer_settings();
        let registry = run_async(async move {
            crate::skills::install::fetch_registry(&network, &registry_url).await
        });
        match registry {
            Ok(crate::skills::install::RegistryFetchResult::Loaded(doc)) => {
                let recommendations =
                    crate::skills::recommend::recommend_remote_skills(task, &doc, 3);
                Ok(recommendations
                    .into_iter()
                    .map(|recommendation| SkillRecommendation {
                        name: recommendation.name.to_string(),
                        description: recommendation.entry.description.clone(),
                        matched_terms: recommendation.matched_terms.clone(),
                    })
                    .collect())
            }
            Ok(crate::skills::install::RegistryFetchResult::NeedsApproval(host)) => {
                Err(needs_approval_message(&host))
            }
            Ok(crate::skills::install::RegistryFetchResult::Denied(host)) => {
                Err(network_denied_message(&host))
            }
            Err(err) => Err(format_registry_error("Failed to fetch registry", &err)),
        }
    }

    fn sync_registry(&mut self) -> Result<SkillSyncOutcome, String> {
        use crate::skills::install::{SkillSyncOutcome as TuiSyncOutcome, SyncResult};
        let (network, max_size, registry_url) = installer_settings();
        let cache_dir = crate::skills::install::default_cache_skills_dir();
        let result = run_async(async move {
            crate::skills::install::sync_registry(&network, &registry_url, &cache_dir, max_size)
                .await
        });
        match result {
            Ok(SyncResult::RegistryDenied(host)) => Ok(SkillSyncOutcome::RegistryDenied(host)),
            Ok(SyncResult::RegistryNeedsApproval(host)) => {
                Ok(SkillSyncOutcome::RegistryNeedsApproval(host))
            }
            Ok(SyncResult::Done { outcomes }) => {
                let total = outcomes.len();
                let mut downloaded = 0usize;
                let mut fresh = 0usize;
                let mut failed = 0usize;
                let entries = outcomes
                    .into_iter()
                    .map(|outcome| match outcome {
                        TuiSyncOutcome::Downloaded { name, path } => {
                            downloaded += 1;
                            SkillSyncEntry::Downloaded {
                                name,
                                path: path.display().to_string(),
                            }
                        }
                        TuiSyncOutcome::Fresh { name } => {
                            fresh += 1;
                            SkillSyncEntry::Fresh { name }
                        }
                        TuiSyncOutcome::Failed { name, reason } => {
                            failed += 1;
                            SkillSyncEntry::Failed { name, reason }
                        }
                        TuiSyncOutcome::Denied { name, host } => {
                            failed += 1;
                            SkillSyncEntry::Denied { name, host }
                        }
                        TuiSyncOutcome::NeedsApproval { name, host } => {
                            failed += 1;
                            SkillSyncEntry::NeedsApproval { name, host }
                        }
                    })
                    .collect();
                Ok(SkillSyncOutcome::Done {
                    total,
                    downloaded,
                    fresh,
                    failed,
                    entries,
                })
            }
            Err(err) => Err(format_registry_error("Sync failed", &err)),
        }
    }

    fn run_review(&mut self) -> Result<ReviewOutcome, String> {
        let skills_dir = self.host.app.borrow().skills_dir.clone();
        let registry = crate::skills::SkillRegistry::discover(&skills_dir).into_enabled();
        let mut warnings: Vec<String> = registry.warnings().to_vec();
        let mut skill = registry.get("review").cloned();

        let global_dir = crate::skills::default_skills_dir();
        if skill.is_none() && global_dir != skills_dir {
            let registry = crate::skills::SkillRegistry::discover(&global_dir).into_enabled();
            if warnings.is_empty() {
                warnings = registry.warnings().to_vec();
            } else if !registry.warnings().is_empty() {
                warnings.extend(registry.warnings().iter().cloned());
            }
            skill = registry.get("review").cloned();
        }

        match skill {
            Some(skill) => {
                // Host-side side effects (D2): session-message insertion and
                // active-skill mutation are authoritative App operations; the
                // portable handler renders no success message (baseline emits
                // only the SendMessage action) and never touches App.
                let instruction = format!(
                    "You are now using a skill. Follow these instructions:\n\n# Skill: {}\n\n{}\n\n---\n\nNow respond to the user's request following the above skill instructions.",
                    skill.name, skill.body
                );
                let mut app = self.host.app.borrow_mut();
                app.add_message(HistoryCell::System {
                    content: format!("Activated skill: {}\n\n{}", skill.name, skill.description),
                });
                app.active_skill = Some(instruction);
                app.active_skill_provenance = None;
                Ok(ReviewOutcome::Ready)
            }
            None => Ok(ReviewOutcome::NotFound {
                skills_dir: skills_dir.display().to_string(),
                global_dir: global_dir.display().to_string(),
                warnings,
            }),
        }
    }

    fn snapshot_list(&mut self, limit: usize) -> Result<Vec<SnapshotEntry>, String> {
        let workspace = self.host.app.borrow().workspace.clone();
        let repo = match crate::snapshot::SnapshotRepo::open_or_init(&workspace) {
            Ok(repo) => repo,
            Err(err) => {
                return Err(format!(
                    "Snapshot repo unavailable for {}: {err}",
                    workspace.display(),
                ));
            }
        };
        let snapshots = match repo.list(limit) {
            Ok(snapshots) => snapshots,
            Err(err) => return Err(format!("Failed to list snapshots: {err}")),
        };
        Ok(snapshots
            .into_iter()
            .map(|snapshot| SnapshotEntry {
                id: snapshot.id.0,
                label: snapshot.label,
                timestamp: snapshot.timestamp,
            })
            .collect())
    }

    fn restore_snapshot(&mut self, id: &str) -> Result<(), String> {
        let workspace = self.host.app.borrow().workspace.clone();
        let repo = match crate::snapshot::SnapshotRepo::open_or_init(&workspace) {
            Ok(repo) => repo,
            Err(err) => {
                return Err(format!(
                    "Snapshot repo unavailable for {}: {err}",
                    workspace.display(),
                ));
            }
        };
        repo.restore(&crate::snapshot::SnapshotId(id.to_string()))
            .map_err(|err| format!("Restore failed: {err}"))
    }

    fn approval_state(&self) -> CommandApprovalState {
        let app = self.host.app.borrow();
        CommandApprovalState {
            yolo: app.yolo,
            trust_mode: app.trust_mode,
        }
    }
}

// ---------------------------------------------------------------------------
// Envelope construction (D1)
// ---------------------------------------------------------------------------

/// Owns eleven facet objects sharing one synchronous TUI host proxy.
///
/// Handlers borrow only these adapters. Every method delegates to the real App
/// authority and releases its `RefCell` borrow before returning, so facets can
/// be called sequentially without exposing TUI types across the boundary.
pub(crate) struct CommandContextBundle<'a> {
    session: SessionAdapter<'a>,
    skill_group: SkillGroupAdapter<'a>,
    model: ModelAdapter<'a>,
    cost: CostAdapter<'a>,
    mode_policy: ModePolicyAdapter<'a>,
    system_prompt: SystemPromptAdapter<'a>,
    skills: SkillsAdapter<'a>,
    workspace: WorkspaceAdapter<'a>,
    presentation: PresentationAdapter<'a>,
    media: MediaAdapter<'a>,
    project: ProjectAdapter<'a>,
}

impl<'a> CommandContextBundle<'a> {
    pub(crate) fn contexts(&mut self) -> CommandContexts<'_> {
        CommandContexts::empty()
            .with_session(&mut self.session)
            .with_model(&mut self.model)
            .with_cost(&mut self.cost)
            .with_mode_policy(&mut self.mode_policy)
            .with_system_prompt(&mut self.system_prompt)
            .with_skills(&mut self.skills)
            .with_workspace(&mut self.workspace)
            .with_presentation(&mut self.presentation)
            .with_media(&mut self.media)
            .with_project(&mut self.project)
            .with_skill_group(&mut self.skill_group)
    }

    /// Test-only: consume the bundle into independent facet parts.
    #[cfg(test)]
    pub(crate) fn parts(&mut self) -> ContextParts<'_> {
        self.contexts().into_parts()
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
            project: ProjectAdapter { host: host.clone() },
            media: MediaAdapter { host: host.clone() },
            skill_group: SkillGroupAdapter { host },
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
        }
        assert_eq!(app.input, input_before, "no eager composer mutation");
    }

    // ---------------------------------------------------------------------
    // FEAT-021 project adapter tests
    // ---------------------------------------------------------------------

    #[test]
    fn key_to_project_message_id_resolves_goal_runtime_keys_and_rejects_unknown() {
        // FEAT-021 D5: only /goal uses runtime translations via the project
        // key map; unknown keys fail safely.
        assert_eq!(
            key_to_project_message_id("goal_control_accepted"),
            Some(MessageId::GoalControlAccepted)
        );
        assert_eq!(
            key_to_project_message_id("goal_status_idle_hint"),
            Some(MessageId::GoalStatusIdleHint)
        );
        assert_eq!(key_to_project_message_id("goal_bogus_key"), None);
        assert_eq!(key_to_project_message_id(""), None);
    }

    #[test]
    fn presentation_translate_resolves_project_keys_with_locale_and_fallback() {
        // The presentation facet resolves the project runtime keys through the
        // current catalog (authoritative English fallback preserved).
        let mut app = test_app();
        let mut bundle = app.command_contexts();
        let mut parts = bundle.parts();
        let presentation = parts.presentation.as_mut().expect("presentation facet");
        let accepted = presentation
            .translate("goal_control_accepted", &[])
            .expect("goal_control_accepted must resolve");
        assert!(
            accepted.contains("Goal control saved"),
            "English fallback text expected: {accepted}"
        );
        let hint = presentation
            .translate("goal_status_idle_hint", &[])
            .expect("goal_status_idle_hint must resolve");
        assert!(hint.contains("not running now"), "hint: {hint}");
        assert!(
            presentation.translate("goal_bogus", &[]).is_err(),
            "unknown key must fail safely"
        );
    }

    #[test]
    fn project_adapter_maps_lsp_state() {
        let mut app = test_app();
        app.lsp_enabled = false;
        assert!(!app.lsp_enabled);
        {
            let mut bundle = app.command_contexts();
            let project = bundle
                .parts()
                .project
                .expect("project facet must be present");
            assert!(!project.lsp_enabled());

            project.lsp_set(true).unwrap();
            assert!(project.lsp_enabled());
            project.lsp_set(false).unwrap();
            assert!(!project.lsp_enabled());
        }
        assert!(!app.lsp_enabled);
    }

    #[test]
    fn project_adapter_share_projection_maps_history_model_and_mode() {
        let mut app = test_app();
        app.model = "deepseek-v4-pro".to_string();
        app.mode = crate::tui::app::AppMode::Agent;
        let mut bundle = app.command_contexts();
        let project = bundle
            .parts()
            .project
            .expect("project facet must be present");

        // Empty history → empty share branch.
        let share = project.share_projection();
        assert!(share.history_is_empty);
        assert_eq!(share.history_len, 0);

        // Populated history → length and labels match host exactly.
        app.history.push(crate::tui::history::HistoryCell::User {
            content: "hello".to_string(),
        });
        app.history
            .push(crate::tui::history::HistoryCell::Assistant {
                content: "world".to_string(),
                streaming: false,
            });
        let mut bundle = app.command_contexts();
        let project = bundle
            .parts()
            .project
            .expect("project facet must be present");
        let share = project.share_projection();
        assert!(!share.history_is_empty);
        assert_eq!(share.history_len, 2);
        assert_eq!(share.model, "deepseek-v4-pro");
        assert_eq!(share.mode_label, crate::tui::app::AppMode::Agent.label());
    }

    #[test]
    fn project_adapter_goal_projection_preserves_visible_and_effective_state() {
        let mut app = test_app();
        app.goal.objective = Some("Ship FEAT-021".to_string());
        app.goal.status = crate::tools::goal::GoalStatus::Active;
        app.goal.time_used_seconds = 42;
        app.goal.token_budget = Some(50_000);
        app.goal.tokens_used = 1_000;
        app.goal.continuation_count = 3;
        app.session.total_conversation_tokens = 2_000;
        app.goal_continuation_waiting = true;
        app.is_loading = false;
        app.api_messages.push(crate::models::Message {
            role: crate::models::Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "work".to_string(),
                cache_control: None,
            }],
        });

        let mut bundle = app.command_contexts();
        let project = bundle
            .parts()
            .project
            .expect("project facet must be present");
        let goal = project.goal_state();
        assert_eq!(goal.objective.as_deref(), Some("Ship FEAT-021"));
        assert_eq!(goal.status, ProjectGoalStatus::Active);
        assert_eq!(goal.time_used_seconds, 42);
        assert_eq!(goal.token_budget, Some(50_000));
        assert_eq!(goal.tokens_used, 1_000);
        assert_eq!(goal.session_total_tokens, 2_000);
        assert_eq!(goal.continuation_count, 3);
        assert!(!goal.pending_controls);
        assert!(goal.goal_continuation_waiting);
        assert!(goal.conversation_present);

        // Pending controls flip the effective source to the durable state.
        app.pending_goal_controls
            .push_back(crate::tui::app::PendingGoalControl {
                intent: crate::tui::app::GoalControlIntent::SetStatus {
                    status: crate::tools::goal::GoalStatus::Paused,
                    clear: false,
                },
                dispatched: false,
            });
        app.last_known_goal_state = Some(crate::session_manager::SessionGoalState {
            schema_version: 1,
            objective: "Durable objective".to_string(),
            status: crate::session_manager::SessionGoalStatus::Paused,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            continuation_count: 0,
            elapsed_seconds: 0,
            pause_reason: None,
        });
        let mut bundle = app.command_contexts();
        let project = bundle
            .parts()
            .project
            .expect("project facet must be present");
        let goal = project.goal_state();
        assert!(goal.pending_controls);
        assert_eq!(
            goal.last_known_objective.as_deref(),
            Some("Durable objective")
        );
        assert_eq!(goal.last_known_status, Some(ProjectGoalStatus::Paused));
    }

    #[test]
    fn project_adapter_exposure_matches_main_envelope_model() {
        // main's envelope always populates every adapter (no capability
        // bitmask yet); the project facet is present and usable, and the
        // handlers destructure only the facets they need.
        let mut app = test_app();
        let mut bundle = app.command_contexts();
        let parts = bundle.parts();
        assert!(parts.project.is_some());
        assert!(parts.workspace.is_some());
        assert!(parts.presentation.is_some());
    }

    // ─── FEAT-022 skill-group adapter tests ───────────────────────────────────

    /// Pins HOME to a tempdir for the duration of the test under the
    /// crate-wide env mutex (keeps global skill/snapshot discovery hermetic).
    struct ScopedHome {
        prev: Option<std::ffi::OsString>,
        _home: TempDir,
        _guard: crate::test_support::TestEnvLock,
    }
    impl Drop for ScopedHome {
        fn drop(&mut self) {
            // SAFETY: process-wide lock still held.
            unsafe {
                match self.prev.take() {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
        }
    }
    fn scoped_home(_workspace: &TempDir) -> ScopedHome {
        let guard = crate::test_support::lock_test_env();
        let prev = std::env::var_os("HOME");
        let home = TempDir::new().expect("home tempdir");
        // SAFETY: serialised by the global env lock.
        unsafe {
            std::env::set_var("HOME", home.path());
        }
        ScopedHome {
            prev,
            _home: home,
            _guard: guard,
        }
    }

    fn skill_test_app(tmp: &TempDir, skills_dir: &Path) -> App {
        let mut options = crate::test_support::test_tui_options(tmp.path());
        options.skills_dir = skills_dir.to_path_buf();
        crate::test_support::test_app_with_options(options)
    }

    fn write_skill(dir: &Path, name: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n{name} instructions"),
        )
        .unwrap();
    }

    #[test]
    fn skill_group_projection_maps_native_skills_and_dirs() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "demo");
        let mut app = skill_test_app(&tmp, &skills_dir);
        let mut bundle = app.command_contexts();
        let group = bundle
            .parts()
            .skill_group
            .expect("skill_group facet must be present");
        let projection = group.skill_registry_projection();
        assert_eq!(projection.total, 1);
        assert_eq!(projection.entries.len(), 1);
        assert_eq!(projection.entries[0].name, "demo");
        assert_eq!(projection.entries[0].description, "demo skill");
        assert_eq!(projection.entries[0].source, SkillSourceKind::Native);
        assert!(projection.entries[0].path.is_some());
        assert_eq!(projection.skills_dir, skills_dir.display().to_string());
        assert!(!projection.dirs.is_empty());
        assert!(projection.warnings.is_empty());
    }

    #[test]
    fn skill_group_projection_reports_empty_registry() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let mut app = skill_test_app(&tmp, &skills_dir);
        let mut bundle = app.command_contexts();
        let group = bundle
            .parts()
            .skill_group
            .expect("skill_group facet must be present");
        let projection = group.skill_registry_projection();
        assert_eq!(projection.total, 0);
        assert!(projection.entries.is_empty());
    }

    #[test]
    fn skill_group_activation_sets_active_skill_and_history() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "demo");
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let outcome = group.activate_skill("demo").unwrap();
            assert_eq!(outcome.name, "demo");
            assert_eq!(outcome.description, "demo skill");
        }
        assert!(app.active_skill.is_some());
        assert!(
            app.active_skill
                .as_deref()
                .unwrap()
                .contains("# Skill: demo")
        );
        assert!(app.active_skill_provenance.is_none());
        assert!(!app.history.is_empty());
    }

    #[test]
    fn skill_group_activation_looks_up_exact_name() {
        // The `/skill new` -> skill-creator alias is handler-side parsing
        // (Phase 4); the delegate performs an exact host lookup.
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "skill-creator");
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let outcome = group.activate_skill("skill-creator").unwrap();
            assert_eq!(outcome.name, "skill-creator");
        }
        assert!(app.active_skill.is_some());
    }

    #[test]
    fn skill_group_activation_not_found_lists_available() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "demo");
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let err = group.activate_skill("missing").unwrap_err();
            match err {
                SkillActivationError::NotFound {
                    requested,
                    available,
                    ..
                } => {
                    assert_eq!(requested, "missing");
                    assert!(available.contains(&"demo".to_string()));
                }
                _ => panic!("expected NotFound"),
            }
        }
        assert!(app.active_skill.is_none());
    }

    #[test]
    fn skill_group_install_invalid_source_returns_safe_error() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let err = group.install_skill(None, "   ").unwrap_err();
            assert!(err.contains("Invalid install source"), "{err}");
        }
    }

    #[test]
    fn skill_group_review_ready_sets_side_effects() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        write_skill(&skills_dir, "review");
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let outcome = group.run_review().unwrap();
            assert_eq!(outcome, ReviewOutcome::Ready);
        }
        assert!(app.active_skill.is_some());
        assert!(app.active_skill_provenance.is_none());
        assert!(!app.history.is_empty());
    }

    #[test]
    fn skill_group_review_not_found_reports_searched_dirs() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let outcome = group.run_review().unwrap();
            match outcome {
                ReviewOutcome::NotFound {
                    skills_dir: found_dir,
                    global_dir,
                    warnings,
                } => {
                    assert_eq!(found_dir, skills_dir.display().to_string());
                    assert_eq!(
                        global_dir,
                        crate::skills::default_skills_dir().display().to_string()
                    );
                    assert!(warnings.is_empty());
                }
                _ => panic!("expected NotFound"),
            }
        }
        assert!(app.active_skill.is_none());
    }

    #[test]
    fn skill_group_snapshot_list_and_restore_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        let file = tmp.path().join("a.txt");
        let repo = crate::snapshot::SnapshotRepo::open_or_init(tmp.path()).unwrap();
        std::fs::write(&file, b"v1").unwrap();
        repo.snapshot("pre-turn:1").unwrap();
        std::fs::write(&file, b"v2").unwrap();
        let mut app = skill_test_app(&tmp, &skills_dir);
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let entries = group.snapshot_list(20).unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].label, "pre-turn:1");
            assert!(!entries[0].id.is_empty());
            group.restore_snapshot(&entries[0].id).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
    }

    #[test]
    fn skill_group_approval_state_reflects_app_posture() {
        let tmp = TempDir::new().unwrap();
        let _home = scoped_home(&tmp);
        let skills_dir = tmp.path().join("skills");
        let mut app = skill_test_app(&tmp, &skills_dir);
        app.yolo = true;
        app.trust_mode = false;
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let state = group.approval_state();
            assert!(state.yolo);
            assert!(!state.trust_mode);
        }
        app.yolo = false;
        app.trust_mode = true;
        {
            let mut bundle = app.command_contexts();
            let group = bundle
                .parts()
                .skill_group
                .expect("skill_group facet must be present");
            let state = group.approval_state();
            assert!(!state.yolo);
            assert!(state.trust_mode);
        }
    }

    #[test]
    fn portable_scope_maps_both_scopes_and_none() {
        use crate::skills::mutation::SkillTargetScope as TuiScope;
        assert_eq!(
            portable_scope(Some(SkillTargetScope::Project)),
            Some(TuiScope::Project)
        );
        assert_eq!(
            portable_scope(Some(SkillTargetScope::Global)),
            Some(TuiScope::Global)
        );
        assert_eq!(portable_scope(None), None);
    }

    #[test]
    fn portable_mutation_receipt_maps_distinct_outcomes() {
        use crate::skills::audit::SkillActionKind;
        use crate::skills::mutation::{
            SkillMutationOutcome as TuiOutcome, SkillMutationReceipt as TuiReceipt,
        };
        use crate::skills::roots::SkillScope;
        let make = |outcome: TuiOutcome| TuiReceipt {
            action: SkillActionKind::Install,
            name: "demo".to_string(),
            scope: SkillScope::Global,
            safe_target_path: "/tmp/demo".to_string(),
            before_digest: None,
            after_digest: None,
            outcome,
        };
        let installed = portable_mutation_receipt(&make(TuiOutcome::Installed));
        assert_eq!(installed.outcome, SkillMutationOutcome::Installed);
        assert_eq!(installed.name, "demo");
        assert_eq!(installed.safe_target_path, "/tmp/demo");

        let approval =
            portable_mutation_receipt(&make(TuiOutcome::NeedsApproval("acme.com".to_string())));
        assert_eq!(
            approval.outcome,
            SkillMutationOutcome::NeedsApproval("acme.com".to_string())
        );

        let denied =
            portable_mutation_receipt(&make(TuiOutcome::NetworkDenied("acme.com".to_string())));
        assert_eq!(
            denied.outcome,
            SkillMutationOutcome::NetworkDenied("acme.com".to_string())
        );
        assert_ne!(installed.outcome, denied.outcome);
    }

    #[test]
    fn skill_group_adapter_exposure_matches_main_envelope_model() {
        // The envelope populates the skill_group slot alongside the other
        // adapters; handlers destructure only their declared facets (D4).
        let mut app = test_app();
        let mut bundle = app.command_contexts();
        let parts = bundle.parts();
        assert!(parts.skill_group.is_some());
        assert!(parts.project.is_some());
        assert!(parts.skills.is_some());
    }
}
