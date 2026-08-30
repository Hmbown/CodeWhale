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
    CommandCostContext, CommandMediaContext, CommandModePolicyContext, CommandModelContext,
    CommandPresentationContext, CommandProjectContext, CommandSessionContext, CommandSkillsContext,
    CommandSystemPromptContext, CommandWorkspaceContext, MediaAttachmentReceipt, ProjectGoalState,
    ProjectGoalStatus, ProjectShareProjection,
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
pub(crate) const PENDING_GROUPS: &[&str] = &[
    "config", "core", "debug", "memory", "plugins", "session", "skills",
];

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
// Envelope construction (D1)
// ---------------------------------------------------------------------------

/// Owns ten facet objects sharing one synchronous TUI host proxy.
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
            media: MediaAdapter { host },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::Locale;
    use crate::models::Role;

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
}
