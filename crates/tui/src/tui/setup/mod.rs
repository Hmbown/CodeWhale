//! Constitution-first setup wizard shell (#3404/#3794).
//!
//! This module owns the reusable setup shell: step ordering, navigation,
//! per-step status projection, and the v0.8.67 constitution checkpoint action.
//! Individual step contents can grow behind [`SetupWizardStep`] without
//! changing the navigation or commit contract.

use std::borrow::Cow;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap},
};

use crate::config::{Config, has_api_key, has_api_key_for};
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::prompts::{
    BASE_PROMPT_OVERRIDE_OPT_IN_ENV, CONSTITUTION_OVERRIDE_FILE, base_prompt_override_opt_in,
};
use crate::tui::app::App;
use crate::tui::onboarding;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer, render_modal_surface,
};

use codewhale_config::{
    AutonomyPreference, ConstitutionAuthoring, ConstitutionChoice, ConstitutionSource,
    ConstitutionValidity, InheritedConfigFacts, RuntimePostureSource, SetupState, SetupStep,
    StepEntry, StepStatus, UserConstitution, UserConstitutionLoad,
    user_constitution::MAX_NOTES_LEN,
};

mod fleet_draft;
mod model_draft;
mod operate;
mod persistence;
mod provider;
mod remote;
mod tools_mcp;

pub(crate) use fleet_draft::{draft_fleet_profile_with_model, workspace_fingerprint};
pub(crate) use model_draft::draft_constitution_with_model;
use persistence::SetupPersistenceFacts;
use remote::SetupRemoteFacts;

/// Target lane for the once-per-version constitution checkpoint. The workspace
/// package remains 0.8.66 until release approval, so this cannot read
/// `CARGO_PKG_VERSION` yet.
pub const CONSTITUTION_CHECKPOINT_VERSION: &str = "0.8.67";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupCommitKind {
    BundledConstitution,
    DeferredConstitution,
}

pub trait SetupWizardStep {
    fn id(&self) -> SetupStep;
    fn title_id(&self) -> MessageId;
    fn why_id(&self) -> MessageId;
    fn required(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StaticSetupStep {
    id: SetupStep,
    title_id: MessageId,
    why_id: MessageId,
    required: bool,
}

impl SetupWizardStep for StaticSetupStep {
    fn id(&self) -> SetupStep {
        self.id
    }

    fn title_id(&self) -> MessageId {
        self.title_id
    }

    fn why_id(&self) -> MessageId {
        self.why_id
    }

    fn required(&self) -> bool {
        self.required
    }
}

const STEP_SPECS: [StaticSetupStep; 10] = [
    StaticSetupStep {
        id: SetupStep::Language,
        title_id: MessageId::SetupStepLanguageTitle,
        why_id: MessageId::SetupStepLanguageWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::ProviderModel,
        title_id: MessageId::SetupStepProviderModelTitle,
        why_id: MessageId::SetupStepProviderModelWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::TrustSandbox,
        title_id: MessageId::SetupStepTrustSandboxTitle,
        why_id: MessageId::SetupStepTrustSandboxWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::Constitution,
        title_id: MessageId::SetupStepConstitutionTitle,
        why_id: MessageId::SetupStepConstitutionWhy,
        required: true,
    },
    StaticSetupStep {
        id: SetupStep::OperateFleet,
        title_id: MessageId::SetupStepOperateFleetTitle,
        why_id: MessageId::SetupStepOperateFleetWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Hotbar,
        title_id: MessageId::SetupStepHotbarTitle,
        why_id: MessageId::SetupStepHotbarWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::ToolsMcp,
        title_id: MessageId::SetupStepToolsMcpTitle,
        why_id: MessageId::SetupStepToolsMcpWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::RemoteRuntime,
        title_id: MessageId::SetupStepRemoteRuntimeTitle,
        why_id: MessageId::SetupStepRemoteRuntimeWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Persistence,
        title_id: MessageId::SetupStepPersistenceTitle,
        why_id: MessageId::SetupStepPersistenceWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Verification,
        title_id: MessageId::SetupStepVerificationTitle,
        why_id: MessageId::SetupStepVerificationWhy,
        required: false,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupWizardView {
    state: SetupState,
    selected: usize,
    locale: Locale,
    facts: SetupRuntimeFacts,
    guided_draft: GuidedConstitutionDraft,
    freeform_note: String,
    editing_freeform_note: bool,
    guided_preview_seen: bool,
    /// The keep-existing path mirrors the guided two-step: the first `K`
    /// opens the rendered preview of the existing file, the second completes
    /// the checkpoint without touching it.
    existing_preview_seen: bool,
    /// A model-drafted constitution awaiting ratification, installed by the
    /// host after a successful one-shot draft (already sanitized + bounded).
    /// Cleared whenever a guided answer changes so a stale draft can never be
    /// ratified against fresh answers.
    model_draft: Option<Box<UserConstitution>>,
    /// Display label of the model that authored `model_draft` (safe metadata,
    /// e.g. "GLM-5.2"), for provenance copy only.
    model_draft_label: Option<String>,
    runtime_preset: SetupRuntimePreset,
    runtime_preset_preview_seen: bool,
    body_scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SetupRuntimeFacts {
    provider: String,
    model: String,
    auth: String,
    health: String,
    provider_ready: bool,
    provider_result: String,
    work_intent: String,
    approval: String,
    shell: String,
    allow_shell_enabled: bool,
    trust: String,
    sandbox: String,
    sandbox_mode_value: String,
    network: String,
    network_default_value: String,
    runtime_result: String,
    operate_runtime_ready: bool,
    operate_runtime_result: String,
    fleet_roster_ready: bool,
    fleet_roster_result: String,
    operate_concurrency_result: String,
    operate_result: String,
    hotbar_bindings_result: String,
    hotbar_actions_result: String,
    hotbar_result: String,
    tools_mcp_servers_result: String,
    tools_mcp_skills_result: String,
    tools_mcp_tools_result: String,
    tools_mcp_plugins_result: String,
    tools_mcp_hotbar_result: String,
    tools_mcp_result: String,
    tools_mcp_needs_action: bool,
    tools_mcp_path_display: String,
    tools_mcp_skills_path_display: String,
    tools_mcp_plugins_path_display: String,
    remote_clouds_result: String,
    remote_bridges_result: String,
    remote_providers_result: String,
    remote_mode_result: String,
    remote_command_provider: String,
    remote_result: String,
    persistence: SetupPersistenceFacts,
    default_mode: String,
    approval_policy_value: String,
    project_override_warning: Option<String>,
    constitution_autonomy: String,
    constitution_file: SetupConstitutionFileState,
    expert_override: SetupExpertOverrideState,
}

impl Default for SetupRuntimeFacts {
    fn default() -> Self {
        Self {
            provider: "not loaded".to_string(),
            model: "not loaded".to_string(),
            auth: "not checked".to_string(),
            health: "not checked".to_string(),
            provider_ready: false,
            provider_result: "provider/model not loaded".to_string(),
            work_intent: "not loaded".to_string(),
            approval: "not loaded".to_string(),
            shell: "not loaded".to_string(),
            allow_shell_enabled: false,
            trust: "not loaded".to_string(),
            sandbox: "not configured".to_string(),
            sandbox_mode_value: "default".to_string(),
            network: "not configured".to_string(),
            network_default_value: "prompt".to_string(),
            runtime_result: "runtime posture not loaded".to_string(),
            operate_runtime_ready: false,
            operate_runtime_result: "worker runtime not loaded".to_string(),
            fleet_roster_ready: false,
            fleet_roster_result: "Fleet roster not loaded".to_string(),
            operate_concurrency_result: "concurrency not loaded".to_string(),
            operate_result: "operate readiness not loaded".to_string(),
            hotbar_bindings_result: "Hotbar config not loaded".to_string(),
            hotbar_actions_result: "Hotbar actions not loaded".to_string(),
            hotbar_result: "hotbar not loaded".to_string(),
            tools_mcp_servers_result: "MCP config not loaded".to_string(),
            tools_mcp_skills_result: "skills dir not loaded".to_string(),
            tools_mcp_tools_result: "tools dir not loaded".to_string(),
            tools_mcp_plugins_result: "plugins dir not loaded".to_string(),
            tools_mcp_hotbar_result: "hotbar source metadata not loaded".to_string(),
            tools_mcp_result: "tools/MCP not loaded".to_string(),
            tools_mcp_needs_action: false,
            tools_mcp_path_display: String::new(),
            tools_mcp_skills_path_display: String::new(),
            tools_mcp_plugins_path_display: String::new(),
            remote_clouds_result: "remote cloud registry not loaded".to_string(),
            remote_bridges_result: "remote bridge registry not loaded".to_string(),
            remote_providers_result: "provider registry not loaded".to_string(),
            remote_mode_result: "remote setup mode not loaded".to_string(),
            remote_command_provider: "deepseek".to_string(),
            remote_result: "remote runtime not loaded".to_string(),
            persistence: SetupPersistenceFacts::default(),
            default_mode: "agent".to_string(),
            approval_policy_value: "on-request".to_string(),
            project_override_warning: None,
            constitution_autonomy: "not loaded".to_string(),
            constitution_file: SetupConstitutionFileState::NotChecked,
            expert_override: SetupExpertOverrideState::NotChecked,
        }
    }
}

impl SetupRuntimeFacts {
    fn from_app_config(app: &App, config: &Config) -> Self {
        let expert_override = SetupExpertOverrideState::load();
        let provider_ready = has_api_key_for(config, app.api_provider);
        let model = app.model_display_label();
        let provider = app.api_provider.display_name().to_string();
        let auth = if provider_ready {
            "present or local runtime".to_string()
        } else if app.api_provider == crate::config::ApiProvider::OpenaiCodex {
            "missing Codex OAuth login".to_string()
        } else {
            "missing for active provider".to_string()
        };
        let health = if provider_ready {
            "ready for first turn; live validation remains with /provider".to_string()
        } else if app.api_provider == crate::config::ApiProvider::OpenaiCodex {
            "run codex login or set OPENAI_CODEX_ACCESS_TOKEN before first turn".to_string()
        } else if let Some(url) = app.api_provider.credential_url() {
            format!("needs key or local runtime before first turn; credentials: {url}")
        } else {
            "needs key or local runtime before first turn".to_string()
        };
        let provider_result = format!(
            "provider={}, model={}, auth={}, health={}",
            app.api_provider.as_str(),
            model,
            if provider_ready {
                "present/local"
            } else {
                "missing"
            },
            if provider_ready {
                "not checked"
            } else {
                "needs action"
            }
        );
        let shell = if app.allow_shell { "enabled" } else { "hidden" }.to_string();
        let trust = if app.trust_mode {
            "trusted workspace / writes allowed by posture"
        } else {
            "workspace trust not elevated"
        }
        .to_string();
        let sandbox = config
            .sandbox_mode
            .as_deref()
            .filter(|mode| !mode.trim().is_empty())
            .unwrap_or("default")
            .to_string();
        let sandbox_mode_value = sandbox.clone();
        let network_default_value = config
            .network
            .as_ref()
            .map_or("prompt".to_string(), |policy| policy.default.clone());
        let network = config
            .network
            .as_ref()
            .map_or("prompt by default".to_string(), |policy| {
                format!("default {}", policy.default)
            });
        let runtime_result = format!(
            "intent={}, approval={}, shell={}, trust={}, sandbox={}, network={}",
            app.mode.as_setting(),
            app.approval_mode.label().to_ascii_lowercase(),
            if app.allow_shell { "enabled" } else { "hidden" },
            if app.trust_mode {
                "trusted"
            } else {
                "workspace"
            },
            sandbox,
            network
        );
        let operate = operate::SetupOperateFacts::from_app_config(app, config, provider_ready);
        let known_hotbar_action_ids = app
            .hotbar_actions
            .iter()
            .map(|action| action.id())
            .collect::<Vec<_>>();
        let hotbar_resolution = config.resolve_hotbar_bindings(&known_hotbar_action_ids);
        let configured_hotbar_slots = config.hotbar.as_ref().map_or(0, Vec::len);
        let hotbar_state = match config.hotbar.as_ref() {
            None => "hidden",
            Some(bindings) if bindings.is_empty() => "disabled",
            Some(_) => "customized",
        };
        let active_hotbar_slots = hotbar_resolution.bindings.len();
        let hotbar_warning_count = hotbar_resolution.warnings.len();
        let hotbar_bindings_result = format!(
            "{hotbar_state}; configured_slots={configured_hotbar_slots}; active_slots={active_hotbar_slots}; warnings={hotbar_warning_count}"
        );
        let hotbar_actions_result =
            format!("{} bindable actions registered", app.hotbar_actions.len());
        let hotbar_result = format!(
            "state={hotbar_state}, configured_slots={configured_hotbar_slots}, active_slots={active_hotbar_slots}, actions={}, warnings={hotbar_warning_count}",
            app.hotbar_actions.len()
        );
        let codewhale_home = setup_codewhale_home_dir();
        let persistence = SetupPersistenceFacts::from_app_config(app, config, &codewhale_home);
        let tools_mcp =
            tools_mcp::SetupToolsMcpFacts::from_app_config(app, config, &codewhale_home);
        let tools_mcp_servers_result = tools_mcp.servers_result;
        let tools_mcp_skills_result = tools_mcp.skills_result;
        let tools_mcp_tools_result = tools_mcp.tools_result;
        let tools_mcp_plugins_result = tools_mcp.plugins_result;
        let tools_mcp_hotbar_result = tools_mcp.hotbar_result;
        let tools_mcp_result = tools_mcp.result;
        let tools_mcp_needs_action = tools_mcp.needs_action;
        let tools_mcp_path_display = tools_mcp.mcp_path_display;
        let tools_mcp_skills_path_display = tools_mcp.skills_path_display;
        let tools_mcp_plugins_path_display = tools_mcp.plugins_path_display;
        let remote = SetupRemoteFacts::from_app(app);
        let constitution_autonomy = UserConstitution::load()
            .ok()
            .and_then(|load| {
                load.constitution().map(|constitution| {
                    autonomy_label(constitution.autonomy_preference, app.ui_locale).to_string()
                })
            })
            .unwrap_or_else(|| tr(app.ui_locale, MessageId::SetupAutonomyUnspecified).to_string());
        Self {
            provider,
            model,
            auth,
            health,
            provider_ready,
            provider_result,
            work_intent: app.mode.display_name().to_string(),
            approval: app.approval_mode.label().to_ascii_lowercase(),
            shell,
            allow_shell_enabled: app.allow_shell,
            trust,
            sandbox,
            sandbox_mode_value,
            network,
            network_default_value,
            runtime_result,
            operate_runtime_ready: operate.runtime_ready,
            operate_runtime_result: operate.runtime_result,
            fleet_roster_ready: operate.roster_ready,
            fleet_roster_result: operate.roster_result,
            operate_concurrency_result: operate.concurrency_result,
            operate_result: operate.result,
            hotbar_bindings_result,
            hotbar_actions_result,
            hotbar_result,
            tools_mcp_servers_result,
            tools_mcp_skills_result,
            tools_mcp_tools_result,
            tools_mcp_plugins_result,
            tools_mcp_hotbar_result,
            tools_mcp_result,
            tools_mcp_needs_action,
            tools_mcp_path_display,
            tools_mcp_skills_path_display,
            tools_mcp_plugins_path_display,
            remote_clouds_result: remote.clouds_result,
            remote_bridges_result: remote.bridges_result,
            remote_providers_result: remote.providers_result,
            remote_mode_result: remote.mode_result,
            remote_command_provider: remote.command_provider,
            remote_result: remote.result,
            persistence,
            default_mode: app.mode.as_setting().to_string(),
            approval_policy_value: config
                .approval_policy
                .as_deref()
                .filter(|policy| !policy.trim().is_empty())
                .unwrap_or("on-request")
                .to_string(),
            project_override_warning: project_runtime_override_warning(
                &app.workspace,
                app.ui_locale,
            ),
            constitution_autonomy,
            constitution_file: SetupConstitutionFileState::load(),
            expert_override,
        }
    }
}

fn setup_codewhale_home_dir() -> std::path::PathBuf {
    codewhale_config::codewhale_home().unwrap_or_else(|_| {
        dirs::home_dir().map_or_else(
            || std::path::PathBuf::from(".codewhale"),
            |home| home.join(".codewhale"),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetupRuntimePreset {
    AskFirst,
    #[default]
    NormalAgent,
    HighTrustLocal,
}

impl SetupRuntimePreset {
    const ALL: [Self; 3] = [Self::AskFirst, Self::NormalAgent, Self::HighTrustLocal];

    fn from_key(key: char) -> Option<Self> {
        match key {
            '1' => Some(Self::AskFirst),
            '2' => Some(Self::NormalAgent),
            '3' => Some(Self::HighTrustLocal),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::AskFirst => "ask-first",
            Self::NormalAgent => "normal-agent",
            Self::HighTrustLocal => "high-trust-local",
        }
    }

    fn title_id(self) -> MessageId {
        match self {
            Self::AskFirst => MessageId::SetupRuntimePresetAskFirstTitle,
            Self::NormalAgent => MessageId::SetupRuntimePresetNormalAgentTitle,
            Self::HighTrustLocal => MessageId::SetupRuntimePresetHighTrustTitle,
        }
    }

    fn description_id(self) -> MessageId {
        match self {
            Self::AskFirst => MessageId::SetupRuntimePresetAskFirstDescription,
            Self::NormalAgent => MessageId::SetupRuntimePresetNormalAgentDescription,
            Self::HighTrustLocal => MessageId::SetupRuntimePresetHighTrustDescription,
        }
    }

    pub fn default_mode(self) -> &'static str {
        match self {
            Self::AskFirst => "plan",
            Self::NormalAgent => "agent",
            Self::HighTrustLocal => "yolo",
        }
    }

    pub fn approval_policy(self) -> Option<&'static str> {
        match self {
            Self::AskFirst | Self::NormalAgent => Some("on-request"),
            // YOLO derives bypass approval from `default_mode = "yolo"`.
            // `approval_policy = "bypass"` is intentionally not a persisted
            // config value in v0.8.67.
            Self::HighTrustLocal => None,
        }
    }

    pub fn allow_shell(self) -> bool {
        match self {
            Self::AskFirst => false,
            Self::NormalAgent | Self::HighTrustLocal => true,
        }
    }

    pub fn sandbox_mode(self) -> &'static str {
        match self {
            Self::AskFirst => "read-only",
            Self::NormalAgent | Self::HighTrustLocal => "workspace-write",
        }
    }

    pub fn result_summary(self) -> String {
        let approval = self.approval_policy().unwrap_or("mode-derived-yolo-bypass");
        format!(
            "preset={}, default_mode={}, approval_policy={}, allow_shell={}, sandbox_mode={}, network=unchanged, trust=unchanged",
            self.id(),
            self.default_mode(),
            approval,
            self.allow_shell(),
            self.sandbox_mode()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupConstitutionFileState {
    NotChecked,
    Missing,
    Loaded,
    Empty,
    Invalid,
    Unreadable,
    PathError,
}

impl SetupConstitutionFileState {
    fn load() -> Self {
        match UserConstitution::path() {
            Ok(path) => Self::from_load(&UserConstitution::load_from(&path)),
            Err(_) => Self::PathError,
        }
    }

    fn from_load(load: &UserConstitutionLoad) -> Self {
        match load {
            UserConstitutionLoad::Missing => Self::Missing,
            UserConstitutionLoad::Empty => Self::Empty,
            UserConstitutionLoad::Invalid(_) => Self::Invalid,
            UserConstitutionLoad::Unreadable(_) => Self::Unreadable,
            UserConstitutionLoad::Loaded(_) => Self::Loaded,
        }
    }

    fn label(self, choice: ConstitutionChoice, locale: Locale) -> Cow<'static, str> {
        let id = match self {
            Self::NotChecked => MessageId::SetupConstitutionFileNotChecked,
            Self::Missing => MessageId::SetupConstitutionFileMissing,
            Self::Loaded if choice == ConstitutionChoice::GuidedCustom => {
                MessageId::SetupConstitutionFileLoadedSelected
            }
            Self::Loaded if choice.is_explicit() => MessageId::SetupConstitutionFileLoadedInactive,
            Self::Loaded => MessageId::SetupConstitutionFileLoadedUnselected,
            Self::Empty => MessageId::SetupConstitutionFileEmpty,
            Self::Invalid => MessageId::SetupConstitutionFileInvalid,
            Self::Unreadable => MessageId::SetupConstitutionFileUnreadable,
            Self::PathError => MessageId::SetupConstitutionFilePathError,
        };
        tr(locale, id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupExpertOverrideState {
    NotChecked,
    Missing,
    Active,
    Disabled,
    Empty,
    Unreadable,
    PathError,
}

impl SetupExpertOverrideState {
    fn load() -> Self {
        let Some(path) = expert_override_path() else {
            return Self::PathError;
        };
        match std::fs::read_to_string(&path) {
            Ok(raw) if raw.trim().is_empty() => Self::Empty,
            Ok(_) if base_prompt_override_opt_in() => Self::Active,
            Ok(_) => Self::Disabled,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Err(_) => Self::Unreadable,
        }
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }

    fn label(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::NotChecked => tr(locale, MessageId::SetupExpertOverrideNotChecked),
            Self::Missing => tr(locale, MessageId::SetupExpertOverrideMissing),
            Self::Active => tr(locale, MessageId::SetupExpertOverrideActive),
            Self::Disabled => tr(locale, MessageId::SetupExpertOverrideDisabled)
                .replace("{env}", BASE_PROMPT_OVERRIDE_OPT_IN_ENV)
                .into(),
            Self::Empty => tr(locale, MessageId::SetupExpertOverrideEmpty),
            Self::Unreadable => tr(locale, MessageId::SetupExpertOverrideUnreadable),
            Self::PathError => tr(locale, MessageId::SetupExpertOverridePathError),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuidedConstitutionDraft {
    purpose: GuidedPurpose,
    autonomy: AutonomyPreference,
    evidence: GuidedEvidence,
    communication: GuidedCommunication,
    privacy: GuidedPrivacy,
    principles: GuidedPrinciples,
}

impl Default for GuidedConstitutionDraft {
    fn default() -> Self {
        Self {
            purpose: GuidedPurpose::Coding,
            autonomy: AutonomyPreference::Balanced,
            evidence: GuidedEvidence::TestsAndReceipts,
            communication: GuidedCommunication::Concise,
            privacy: GuidedPrivacy::StandardCare,
            principles: GuidedPrinciples::ScopedChanges,
        }
    }
}

impl GuidedConstitutionDraft {
    fn cycle(&mut self, key: char) -> bool {
        match key {
            '1' => self.purpose = self.purpose.next(),
            '2' => self.autonomy = next_guided_autonomy(self.autonomy),
            '3' => self.evidence = self.evidence.next(),
            '4' => self.communication = self.communication.next(),
            '5' => self.privacy = self.privacy.next(),
            '6' => self.principles = self.principles.next(),
            _ => return false,
        }
        true
    }

    #[cfg(test)]
    fn to_constitution(self, locale: Locale) -> UserConstitution {
        self.to_constitution_with_freeform(locale, None)
    }

    fn to_constitution_with_freeform(
        self,
        locale: Locale,
        freeform_note: Option<&str>,
    ) -> UserConstitution {
        let mut notes = self.notes(locale);
        if let Some(note) = freeform_note.map(str::trim).filter(|note| !note.is_empty()) {
            let own_words = match locale {
                Locale::Ja => format!(
                    "\nユーザー自由原則：{}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
                Locale::ZhHans => format!(
                    "\n用户自由原则：{}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
                Locale::ZhHant => format!(
                    "\n使用者自由原則：{}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
                Locale::PtBr => format!(
                    "\nPrincípio livre do usuário: {}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
                Locale::Es419 => format!(
                    "\nPrincipio libre del usuario: {}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
                Locale::Vi => format!(
                    "\nNguyên tắc tự do của người dùng: {}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
                _ => format!(
                    "\nUser freeform principle: {}",
                    bounded_freeform_note(note, MAX_NOTES_LEN)
                ),
            };
            notes.push_str(&own_words);
        }
        UserConstitution {
            language: Some(locale.tag().to_string()),
            about: Some(self.purpose.about(locale).to_string()),
            working_style: vec![
                self.purpose.working_style(locale).to_string(),
                self.communication.working_style(locale).to_string(),
                self.evidence.working_style(locale).to_string(),
                self.privacy.working_style(locale).to_string(),
            ],
            priorities: vec![
                authority_priority(locale).to_string(),
                autonomy_priority(self.autonomy, locale).to_string(),
                self.privacy.escalation_rule(locale).to_string(),
            ],
            autonomy_preference: self.autonomy,
            notes: Some(notes),
            ..UserConstitution::default()
        }
    }

    fn notes(self, locale: Locale) -> String {
        let notes = tr(locale, MessageId::SetupGuidedNotes);
        notes
            .replace("{purpose}", &self.purpose.label(locale))
            .replace("{initiative}", autonomy_label(self.autonomy, locale))
            .replace("{evidence}", &self.evidence.label(locale))
            .replace("{communication}", self.communication.label(locale))
            .replace("{privacy}", self.privacy.label(locale))
            .replace("{principles}", self.principles.label(locale))
            .replace("{notes}", self.principles.note(locale))
            .to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidedPurpose {
    Coding,
    Research,
    Operations,
    Mixed,
}

impl GuidedPurpose {
    fn next(self) -> Self {
        match self {
            Self::Coding => Self::Research,
            Self::Research => Self::Operations,
            Self::Operations => Self::Mixed,
            Self::Mixed => Self::Coding,
        }
    }

    fn label(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Coding => tr(locale, MessageId::SetupGuidedPurposeCoding),
            Self::Research => tr(locale, MessageId::SetupGuidedPurposeResearch),
            Self::Operations => tr(locale, MessageId::SetupGuidedPurposeOperations),
            Self::Mixed => tr(locale, MessageId::SetupGuidedPurposeMixed),
        }
    }

    fn about(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Coding => tr(locale, MessageId::SetupGuidedPurposeAboutCoding),
            Self::Research => tr(locale, MessageId::SetupGuidedPurposeAboutResearch),
            Self::Operations => tr(locale, MessageId::SetupGuidedPurposeAboutOperations),
            Self::Mixed => tr(locale, MessageId::SetupGuidedPurposeAboutMixed),
        }
    }

    fn working_style(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Coding => tr(locale, MessageId::SetupGuidedStyleCoding),
            Self::Research => tr(locale, MessageId::SetupGuidedStyleResearch),
            Self::Operations => tr(locale, MessageId::SetupGuidedStyleOperations),
            Self::Mixed => tr(locale, MessageId::SetupGuidedStyleMixed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidedEvidence {
    Assumptions,
    TestsAndReceipts,
    ReleaseReceipts,
}

impl GuidedEvidence {
    fn next(self) -> Self {
        match self {
            Self::Assumptions => Self::TestsAndReceipts,
            Self::TestsAndReceipts => Self::ReleaseReceipts,
            Self::ReleaseReceipts => Self::Assumptions,
        }
    }

    fn label(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Assumptions => tr(locale, MessageId::SetupGuidedEvidenceAssumptions),
            Self::TestsAndReceipts => tr(locale, MessageId::SetupGuidedEvidenceTestsAndReceipts),
            Self::ReleaseReceipts => tr(locale, MessageId::SetupGuidedEvidenceReleaseReceipts),
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::Assumptions) => {
                "完了を主張する前に、前提、不明点、残るリスクを要約する。"
            }
            (Locale::Ja, Self::TestsAndReceipts) => {
                "不確実性を減らせるときは、コマンド、テスト、スクリーンショット、引用で具体的に検証する。"
            }
            (Locale::Ja, Self::ReleaseReceipts) => {
                "重要な主張とリリース証拠には、ファイル、コマンド、スクリーンショット、CI、出典を示す。"
            }
            (Locale::ZhHans, Self::Assumptions) => "在宣称完成前总结假设、未知和剩余风险。",
            (Locale::ZhHans, Self::TestsAndReceipts) => {
                "在能降低不确定性时，用命令、测试、截图或引用给出具体验证。"
            }
            (Locale::ZhHans, Self::ReleaseReceipts) => {
                "对重要结论和发布证据标注文件、命令、截图、CI 或来源。"
            }
            (Locale::ZhHant, Self::Assumptions) => "在宣稱完成前總結假設、未知和剩餘風險。",
            (Locale::ZhHant, Self::TestsAndReceipts) => {
                "在能降低不確定性時，用命令、測試、截圖或引用給出具體驗證。"
            }
            (Locale::ZhHant, Self::ReleaseReceipts) => {
                "對重要結論和發布證據標註檔案、命令、截圖、CI 或來源。"
            }
            (Locale::PtBr, Self::Assumptions) => {
                "Resuma premissas, desconhecidos e risco restante antes de dizer que concluiu."
            }
            (Locale::PtBr, Self::TestsAndReceipts) => {
                "Use comandos, testes, screenshots ou citações quando reduzirem a incerteza."
            }
            (Locale::PtBr, Self::ReleaseReceipts) => {
                "Cite arquivos, comandos, screenshots, CI ou fontes para afirmações materiais e evidência de release."
            }
            (Locale::Es419, Self::Assumptions) => {
                "Resume supuestos, incógnitas y riesgo restante antes de afirmar que terminaste."
            }
            (Locale::Es419, Self::TestsAndReceipts) => {
                "Usa comandos, pruebas, capturas o citas cuando reduzcan materialmente la incertidumbre."
            }
            (Locale::Es419, Self::ReleaseReceipts) => {
                "Cita archivos, comandos, capturas, CI o fuentes para afirmaciones materiales y evidencia de release."
            }
            (Locale::Vi, Self::Assumptions) => {
                "Tóm tắt giả định, điều chưa biết và rủi ro còn lại trước khi tuyên bố hoàn tất."
            }
            (Locale::Vi, Self::TestsAndReceipts) => {
                "Dùng lệnh, kiểm thử, ảnh chụp hoặc trích dẫn khi chúng giảm đáng kể bất định."
            }
            (Locale::Vi, Self::ReleaseReceipts) => {
                "Trích dẫn tệp, lệnh, ảnh chụp, CI hoặc nguồn cho tuyên bố quan trọng và bằng chứng phát hành."
            }
            (_, Self::Assumptions) => {
                "Summarize assumptions, unknowns, and remaining risk before claiming completion."
            }
            (_, Self::TestsAndReceipts) => {
                "Use commands, tests, screenshots, or citations when they materially reduce uncertainty."
            }
            (_, Self::ReleaseReceipts) => {
                "Cite file paths, commands, screenshots, CI, or sources for material claims and release evidence."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidedCommunication {
    Concise,
    Teaching,
    Direct,
}

impl GuidedCommunication {
    fn next(self) -> Self {
        match self {
            Self::Concise => Self::Teaching,
            Self::Teaching => Self::Direct,
            Self::Direct => Self::Concise,
        }
    }

    fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::Concise) => "簡潔",
            (Locale::Ja, Self::Teaching) => "説明重視",
            (Locale::Ja, Self::Direct) => "直接的",
            (Locale::ZhHans, Self::Concise) => "简洁",
            (Locale::ZhHans, Self::Teaching) => "教学式",
            (Locale::ZhHans, Self::Direct) => "直接",
            (Locale::ZhHant, Self::Concise) => "簡潔",
            (Locale::ZhHant, Self::Teaching) => "教學式",
            (Locale::ZhHant, Self::Direct) => "直接",
            (Locale::PtBr, Self::Concise) => "conciso",
            (Locale::PtBr, Self::Teaching) => "didático",
            (Locale::PtBr, Self::Direct) => "direto",
            (Locale::Es419, Self::Concise) => "conciso",
            (Locale::Es419, Self::Teaching) => "didáctico",
            (Locale::Es419, Self::Direct) => "directo",
            (Locale::Vi, Self::Concise) => "ngắn gọn",
            (Locale::Vi, Self::Teaching) => "giảng giải",
            (Locale::Vi, Self::Direct) => "trực tiếp",
            (_, Self::Concise) => "concise",
            (_, Self::Teaching) => "teaching",
            (_, Self::Direct) => "direct",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::Concise) => "更新は簡潔にし、重要なトレードオフだけ短く説明する。",
            (Locale::Ja, Self::Teaching) => {
                "重要な推論とトレードオフを、ユーザーが仕組みを理解できる程度に説明する。"
            }
            (Locale::Ja, Self::Direct) => {
                "阻塞、リスク、不確実性を直接述べ、装飾的な文案を避ける。"
            }
            (Locale::ZhHans, Self::Concise) => "保持更新简洁，并只解释重要取舍。",
            (Locale::ZhHans, Self::Teaching) => "解释关键推理和取舍，让用户能理解系统。",
            (Locale::ZhHans, Self::Direct) => "直接说明阻塞、风险和不确定性，避免装饰性文案。",
            (Locale::ZhHant, Self::Concise) => "保持更新簡潔，並只解釋重要取捨。",
            (Locale::ZhHant, Self::Teaching) => "解釋關鍵推理和取捨，讓使用者能理解系統。",
            (Locale::ZhHant, Self::Direct) => "直接說明阻塞、風險和不確定性，避免裝飾性文案。",
            (Locale::PtBr, Self::Concise) => {
                "Mantenha atualizações concisas e explique brevemente só os tradeoffs importantes."
            }
            (Locale::PtBr, Self::Teaching) => {
                "Explique raciocínio e tradeoffs principais o bastante para o usuário entender o sistema."
            }
            (Locale::PtBr, Self::Direct) => {
                "Seja direto sobre bloqueios, risco e incerteza; evite texto ornamental."
            }
            (Locale::Es419, Self::Concise) => {
                "Mantén las actualizaciones concisas y explica brevemente solo los tradeoffs importantes."
            }
            (Locale::Es419, Self::Teaching) => {
                "Explica el razonamiento y los tradeoffs clave lo suficiente para que el usuario entienda el sistema."
            }
            (Locale::Es419, Self::Direct) => {
                "Sé directo sobre bloqueos, riesgo e incertidumbre; evita texto ornamental."
            }
            (Locale::Vi, Self::Concise) => {
                "Giữ cập nhật ngắn gọn và chỉ giải thích ngắn các đánh đổi quan trọng."
            }
            (Locale::Vi, Self::Teaching) => {
                "Giải thích suy luận và đánh đổi chính đủ để người dùng hiểu hệ thống."
            }
            (Locale::Vi, Self::Direct) => {
                "Nói thẳng về điểm chặn, rủi ro và bất định; tránh câu chữ trang trí."
            }
            (_, Self::Concise) => "Keep updates concise and explain important tradeoffs briefly.",
            (_, Self::Teaching) => {
                "Explain key reasoning and tradeoffs enough that the user can learn the system."
            }
            (_, Self::Direct) => {
                "Be direct about blockers, risk, and uncertainty; avoid ornamental copy."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidedPrivacy {
    StandardCare,
    StrictBoundaries,
    ProjectLocal,
}

impl GuidedPrivacy {
    fn next(self) -> Self {
        match self {
            Self::StandardCare => Self::StrictBoundaries,
            Self::StrictBoundaries => Self::ProjectLocal,
            Self::ProjectLocal => Self::StandardCare,
        }
    }

    fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::StandardCare) => "標準保護",
            (Locale::Ja, Self::StrictBoundaries) => "厳格な境界",
            (Locale::Ja, Self::ProjectLocal) => "プロジェクト内メモリ",
            (Locale::ZhHans, Self::StandardCare) => "标准保护",
            (Locale::ZhHans, Self::StrictBoundaries) => "严格边界",
            (Locale::ZhHans, Self::ProjectLocal) => "项目内记忆",
            (Locale::ZhHant, Self::StandardCare) => "標準保護",
            (Locale::ZhHant, Self::StrictBoundaries) => "嚴格邊界",
            (Locale::ZhHant, Self::ProjectLocal) => "專案內記憶",
            (Locale::PtBr, Self::StandardCare) => "cuidado padrão",
            (Locale::PtBr, Self::StrictBoundaries) => "limites estritos",
            (Locale::PtBr, Self::ProjectLocal) => "memória local do projeto",
            (Locale::Es419, Self::StandardCare) => "cuidado estándar",
            (Locale::Es419, Self::StrictBoundaries) => "límites estrictos",
            (Locale::Es419, Self::ProjectLocal) => "memoria local del proyecto",
            (Locale::Vi, Self::StandardCare) => "bảo vệ tiêu chuẩn",
            (Locale::Vi, Self::StrictBoundaries) => "ranh giới nghiêm ngặt",
            (Locale::Vi, Self::ProjectLocal) => "bộ nhớ trong dự án",
            (_, Self::StandardCare) => "standard care",
            (_, Self::StrictBoundaries) => "strict boundaries",
            (_, Self::ProjectLocal) => "project-local memory",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::StandardCare) => {
                "秘密情報、ユーザーファイル、Git 履歴、本番システム、コスト、プライバシー、時間を保護する。"
            }
            (Locale::Ja, Self::StrictBoundaries) => {
                "秘密、個人データ、認証情報、本番状態、資金、公開操作は、先に確認する境界として扱う。"
            }
            (Locale::Ja, Self::ProjectLocal) => {
                "プロジェクト固有の文脈はプロジェクト内に留め、明示要求がない限りメモリへ書かない。"
            }
            (Locale::ZhHans, Self::StandardCare) => {
                "保护密钥、用户文件、Git 历史、生产系统、成本、隐私和时间。"
            }
            (Locale::ZhHans, Self::StrictBoundaries) => {
                "把密钥、个人数据、凭据、生产状态、资金和发布动作视为先确认边界。"
            }
            (Locale::ZhHans, Self::ProjectLocal) => {
                "项目特定上下文留在项目内，除非明确要求，否则不要写入记忆。"
            }
            (Locale::ZhHant, Self::StandardCare) => {
                "保護密鑰、使用者檔案、Git 歷史、生產系統、成本、隱私和時間。"
            }
            (Locale::ZhHant, Self::StrictBoundaries) => {
                "把密鑰、個人資料、憑據、生產狀態、資金和發布動作視為先確認邊界。"
            }
            (Locale::ZhHant, Self::ProjectLocal) => {
                "專案特定上下文留在專案內，除非明確要求，否則不要寫入記憶。"
            }
            (Locale::PtBr, Self::StandardCare) => {
                "Proteja segredos, arquivos do usuário, histórico git, produção, custo, privacidade e tempo."
            }
            (Locale::PtBr, Self::StrictBoundaries) => {
                "Trate segredos, dados pessoais, credenciais, estado de produção, dinheiro e publicações como limites de confirmação."
            }
            (Locale::PtBr, Self::ProjectLocal) => {
                "Mantenha contexto específico do projeto no projeto; evite gravar na memória sem pedido explícito."
            }
            (Locale::Es419, Self::StandardCare) => {
                "Protege secretos, archivos del usuario, historial git, producción, costo, privacidad y tiempo."
            }
            (Locale::Es419, Self::StrictBoundaries) => {
                "Trata secretos, datos personales, credenciales, estado de producción, dinero y publicaciones como límites de confirmación."
            }
            (Locale::Es419, Self::ProjectLocal) => {
                "Mantén el contexto específico del proyecto en el proyecto; evita llevarlo a memoria sin pedido explícito."
            }
            (Locale::Vi, Self::StandardCare) => {
                "Bảo vệ bí mật, tệp người dùng, lịch sử git, hệ thống sản xuất, chi phí, riêng tư và thời gian."
            }
            (Locale::Vi, Self::StrictBoundaries) => {
                "Xem bí mật, dữ liệu cá nhân, thông tin xác thực, trạng thái sản xuất, tiền và xuất bản là ranh giới cần xác nhận."
            }
            (Locale::Vi, Self::ProjectLocal) => {
                "Giữ ngữ cảnh riêng của dự án trong dự án; tránh ghi vào bộ nhớ nếu không được yêu cầu rõ."
            }
            (_, Self::StandardCare) => {
                "Protect secrets, user files, git history, production systems, cost, privacy, and time."
            }
            (_, Self::StrictBoundaries) => {
                "Treat secrets, personal data, credentials, production state, money, and publish actions as stop-and-confirm boundaries."
            }
            (_, Self::ProjectLocal) => {
                "Keep project-specific context local; avoid carrying sensitive details into memory unless explicitly asked."
            }
        }
    }

    fn escalation_rule(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::StandardCare) => {
                "破壊的、高コスト、認証情報、公開、法務、セキュリティリスクのある操作の前に尋ねる。"
            }
            (Locale::Ja, Self::StrictBoundaries) => {
                "機微情報の読み取りや拡散、本番システム操作、支出、公開の前に停止して尋ねる。"
            }
            (Locale::Ja, Self::ProjectLocal) => {
                "プロジェクト詳細をメモリ、ワークスペース、古い引き継ぎへ持ち出す前に確認する。"
            }
            (Locale::ZhHans, Self::StandardCare) => {
                "遇到破坏性、高成本、凭据、发布、法律或安全风险操作时先询问。"
            }
            (Locale::ZhHans, Self::StrictBoundaries) => {
                "在读取或传播敏感信息、触碰生产系统、花费资金或发布内容前停止并询问。"
            }
            (Locale::ZhHans, Self::ProjectLocal) => {
                "需要跨项目记忆、复制项目细节或引用旧交接时，先确认这些上下文仍适用。"
            }
            (Locale::ZhHant, Self::StandardCare) => {
                "遇到破壞性、高成本、憑據、發布、法律或安全風險操作時先詢問。"
            }
            (Locale::ZhHant, Self::StrictBoundaries) => {
                "在讀取或傳播敏感資訊、觸碰生產系統、花費資金或發布內容前停止並詢問。"
            }
            (Locale::ZhHant, Self::ProjectLocal) => {
                "需要跨專案記憶、複製專案細節或引用舊交接時，先確認這些上下文仍適用。"
            }
            (Locale::PtBr, Self::StandardCare) => {
                "Pergunte antes de ações destrutivas, caras, com credenciais, publicação, risco legal ou de segurança."
            }
            (Locale::PtBr, Self::StrictBoundaries) => {
                "Pare e pergunte antes de ler ou espalhar dados sensíveis, tocar produção, gastar dinheiro ou publicar."
            }
            (Locale::PtBr, Self::ProjectLocal) => {
                "Confirme antes de levar detalhes do projeto para memória, workspaces ou handoffs antigos."
            }
            (Locale::Es419, Self::StandardCare) => {
                "Pregunta antes de acciones destructivas, costosas, con credenciales, publicación o riesgo legal/de seguridad."
            }
            (Locale::Es419, Self::StrictBoundaries) => {
                "Detente y pregunta antes de leer o difundir datos sensibles, tocar producción, gastar dinero o publicar."
            }
            (Locale::Es419, Self::ProjectLocal) => {
                "Confirma antes de llevar detalles del proyecto a memoria, workspaces o handoffs viejos."
            }
            (Locale::Vi, Self::StandardCare) => {
                "Hỏi trước các thao tác phá hủy, tốn kém, liên quan thông tin xác thực, xuất bản, pháp lý hoặc bảo mật."
            }
            (Locale::Vi, Self::StrictBoundaries) => {
                "Dừng và hỏi trước khi đọc/phát tán dữ liệu nhạy cảm, chạm sản xuất, chi tiền hoặc xuất bản."
            }
            (Locale::Vi, Self::ProjectLocal) => {
                "Xác nhận trước khi mang chi tiết dự án sang bộ nhớ, workspace khác hoặc handoff cũ."
            }
            (_, Self::StandardCare) => {
                "Ask before destructive, high-cost, credential, publishing, legal, or security-risk actions."
            }
            (_, Self::StrictBoundaries) => {
                "Stop and ask before reading or spreading sensitive data, touching production systems, spending money, or publishing."
            }
            (_, Self::ProjectLocal) => {
                "Confirm before carrying project details across memory, workspaces, or stale handoffs."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuidedPrinciples {
    ScopedChanges,
    UserVoice,
    ReversibleOps,
}

impl GuidedPrinciples {
    fn next(self) -> Self {
        match self {
            Self::ScopedChanges => Self::UserVoice,
            Self::UserVoice => Self::ReversibleOps,
            Self::ReversibleOps => Self::ScopedChanges,
        }
    }

    fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::ScopedChanges) => "小さく絞った変更",
            (Locale::Ja, Self::UserVoice) => "ユーザーの声を保つ",
            (Locale::Ja, Self::ReversibleOps) => "可逆手順",
            (Locale::ZhHans, Self::ScopedChanges) => "小范围改动",
            (Locale::ZhHans, Self::UserVoice) => "保留用户语气",
            (Locale::ZhHans, Self::ReversibleOps) => "可逆步骤",
            (Locale::ZhHant, Self::ScopedChanges) => "小範圍改動",
            (Locale::ZhHant, Self::UserVoice) => "保留使用者語氣",
            (Locale::ZhHant, Self::ReversibleOps) => "可逆步驟",
            (Locale::PtBr, Self::ScopedChanges) => "mudanças focadas",
            (Locale::PtBr, Self::UserVoice) => "preservar voz do usuário",
            (Locale::PtBr, Self::ReversibleOps) => "passos reversíveis",
            (Locale::Es419, Self::ScopedChanges) => "cambios acotados",
            (Locale::Es419, Self::UserVoice) => "preservar voz del usuario",
            (Locale::Es419, Self::ReversibleOps) => "pasos reversibles",
            (Locale::Vi, Self::ScopedChanges) => "thay đổi có phạm vi",
            (Locale::Vi, Self::UserVoice) => "giữ giọng người dùng",
            (Locale::Vi, Self::ReversibleOps) => "bước có thể đảo ngược",
            (_, Self::ScopedChanges) => "scoped changes",
            (_, Self::UserVoice) => "user voice",
            (_, Self::ReversibleOps) => "reversible steps",
        }
    }

    fn note(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::Ja, Self::ScopedChanges) => {
                "自由原則：小さくレビューしやすい変更を優先し、明示要求がない限り無関係なリファクタを避ける。"
            }
            (Locale::Ja, Self::UserVoice) => {
                "自由原則：ユーザーの語調、ブランド、制約を保ち、好みを権限拡大として扱わない。"
            }
            (Locale::Ja, Self::ReversibleOps) => {
                "自由原則：影響の大きい操作の前に、可逆手順、チェックポイント、ロールバック説明を選ぶ。"
            }
            (Locale::ZhHans, Self::ScopedChanges) => {
                "自由原则：优先采用小范围、可审查的改动；除非明确要求，不做无关重构。"
            }
            (Locale::ZhHans, Self::UserVoice) => {
                "自由原则：保留用户的语气、品牌和约束；不把偏好推断成权限扩大。"
            }
            (Locale::ZhHans, Self::ReversibleOps) => {
                "自由原则：先选择可逆步骤、检查点和回滚说明，再进行高影响操作。"
            }
            (Locale::ZhHant, Self::ScopedChanges) => {
                "自由原則：優先採用小範圍、可審查的改動；除非明確要求，不做無關重構。"
            }
            (Locale::ZhHant, Self::UserVoice) => {
                "自由原則：保留使用者的語氣、品牌和約束；不把偏好推斷成權限擴大。"
            }
            (Locale::ZhHant, Self::ReversibleOps) => {
                "自由原則：先選擇可逆步驟、檢查點和回復說明，再進行高影響操作。"
            }
            (Locale::PtBr, Self::ScopedChanges) => {
                "Princípio livre: prefira mudanças pequenas e revisáveis; evite refactors não relacionados sem pedido explícito."
            }
            (Locale::PtBr, Self::UserVoice) => {
                "Princípio livre: preserve a voz, marca e restrições do usuário sem tratar preferências como expansão de permissão."
            }
            (Locale::PtBr, Self::ReversibleOps) => {
                "Princípio livre: favoreça passos reversíveis, checkpoints e notas de rollback antes de ações de alto impacto."
            }
            (Locale::Es419, Self::ScopedChanges) => {
                "Principio libre: prefiere cambios pequeños y revisables; evita refactors no relacionados sin pedido explícito."
            }
            (Locale::Es419, Self::UserVoice) => {
                "Principio libre: preserva la voz, marca y restricciones del usuario sin tratar preferencias como expansión de permisos."
            }
            (Locale::Es419, Self::ReversibleOps) => {
                "Principio libre: favorece pasos reversibles, checkpoints y notas de rollback antes de acciones de alto impacto."
            }
            (Locale::Vi, Self::ScopedChanges) => {
                "Nguyên tắc tự do: ưu tiên thay đổi nhỏ, dễ review; tránh refactor không liên quan nếu không được yêu cầu rõ."
            }
            (Locale::Vi, Self::UserVoice) => {
                "Nguyên tắc tự do: giữ giọng, thương hiệu và ràng buộc của người dùng, không xem sở thích là mở rộng quyền."
            }
            (Locale::Vi, Self::ReversibleOps) => {
                "Nguyên tắc tự do: ưu tiên bước có thể đảo ngược, checkpoint và ghi chú rollback trước thao tác tác động cao."
            }
            (_, Self::ScopedChanges) => {
                "Freeform principle: prefer small, reviewable changes and avoid unrelated refactors unless explicitly requested."
            }
            (_, Self::UserVoice) => {
                "Freeform principle: preserve the user's voice, brand, and constraints without treating preferences as permission expansion."
            }
            (_, Self::ReversibleOps) => {
                "Freeform principle: favor reversible steps, checkpoints, and rollback notes before high-impact operations."
            }
        }
    }
}

fn next_guided_autonomy(preference: AutonomyPreference) -> AutonomyPreference {
    match preference {
        AutonomyPreference::Unspecified | AutonomyPreference::Cautious => {
            AutonomyPreference::Balanced
        }
        AutonomyPreference::Balanced => AutonomyPreference::Autonomous,
        AutonomyPreference::Autonomous => AutonomyPreference::Cautious,
    }
}

fn autonomy_label(preference: AutonomyPreference, locale: Locale) -> &'static str {
    match (locale, preference) {
        (Locale::Ja, AutonomyPreference::Cautious) => "慎重",
        (Locale::Ja, AutonomyPreference::Balanced) => "バランス",
        (Locale::Ja, AutonomyPreference::Autonomous) => "積極的",
        (Locale::ZhHans, AutonomyPreference::Cautious) => "谨慎",
        (Locale::ZhHans, AutonomyPreference::Balanced) => "平衡",
        (Locale::ZhHans, AutonomyPreference::Autonomous) => "积极主动",
        (Locale::ZhHant, AutonomyPreference::Cautious) => "謹慎",
        (Locale::ZhHant, AutonomyPreference::Balanced) => "平衡",
        (Locale::ZhHant, AutonomyPreference::Autonomous) => "積極主動",
        (Locale::PtBr, AutonomyPreference::Cautious) => "cauteloso",
        (Locale::PtBr, AutonomyPreference::Balanced) => "equilibrado",
        (Locale::PtBr, AutonomyPreference::Autonomous) => "ambicioso",
        (Locale::Es419, AutonomyPreference::Cautious) => "cauteloso",
        (Locale::Es419, AutonomyPreference::Balanced) => "equilibrado",
        (Locale::Es419, AutonomyPreference::Autonomous) => "ambicioso",
        (Locale::Vi, AutonomyPreference::Cautious) => "thận trọng",
        (Locale::Vi, AutonomyPreference::Balanced) => "cân bằng",
        (Locale::Vi, AutonomyPreference::Autonomous) => "chủ động",
        (_, AutonomyPreference::Cautious) => "cautious",
        (_, AutonomyPreference::Balanced) => "balanced",
        (_, AutonomyPreference::Autonomous) => "ambitious",
        (_, AutonomyPreference::Unspecified) => "unspecified",
    }
}

fn autonomy_priority(preference: AutonomyPreference, locale: Locale) -> &'static str {
    match (locale, preference) {
        (Locale::Ja, AutonomyPreference::Cautious) => {
            "ファイル編集、コマンド実行、あいまいな製品判断の前に停止して尋ねる。"
        }
        (Locale::Ja, AutonomyPreference::Balanced) => {
            "明確で低リスクな作業は直接進め、危険、破壊的、あいまいな操作では先に確認する。"
        }
        (Locale::Ja, AutonomyPreference::Autonomous) => {
            "安全な定型作業はまとめて進めるが、破壊的、認証情報、公開、高コスト、法務、セキュリティリスクでは停止して尋ねる。"
        }
        (Locale::ZhHans, AutonomyPreference::Cautious) => {
            "在编辑文件、运行命令或产品选择不明确前，倾向先停下询问。"
        }
        (Locale::ZhHans, AutonomyPreference::Balanced) => {
            "清晰低风险任务可直接行动；遇到风险、破坏性或歧义时先确认。"
        }
        (Locale::ZhHans, AutonomyPreference::Autonomous) => {
            "可批量处理安全的常规工作，但遇到破坏性、凭据、发布、高成本、法律或安全风险时停止询问。"
        }
        (Locale::ZhHant, AutonomyPreference::Cautious) => {
            "在編輯檔案、執行命令或產品選擇不明確前，傾向先停下詢問。"
        }
        (Locale::ZhHant, AutonomyPreference::Balanced) => {
            "清晰低風險任務可直接行動；遇到風險、破壞性或歧義時先確認。"
        }
        (Locale::ZhHant, AutonomyPreference::Autonomous) => {
            "可批量處理安全的常規工作，但遇到破壞性、憑據、發布、高成本、法律或安全風險時停止詢問。"
        }
        (Locale::PtBr, AutonomyPreference::Cautious) => {
            "Pare e pergunte antes de editar arquivos, rodar comandos ou escolher entre caminhos ambíguos de produto."
        }
        (Locale::PtBr, AutonomyPreference::Balanced) => {
            "Aja diretamente em tarefas claras e de baixo risco; confirme antes de ações arriscadas, destrutivas ou ambíguas."
        }
        (Locale::PtBr, AutonomyPreference::Autonomous) => {
            "Agrupe trabalho seguro de rotina, mas pare para ações destrutivas, credenciais, publicação, alto custo, legais ou de segurança."
        }
        (Locale::Es419, AutonomyPreference::Cautious) => {
            "Detente y pregunta antes de editar archivos, ejecutar comandos o elegir entre caminos ambiguos de producto."
        }
        (Locale::Es419, AutonomyPreference::Balanced) => {
            "Actúa directamente en tareas claras y de bajo riesgo; confirma antes de acciones riesgosas, destructivas o ambiguas."
        }
        (Locale::Es419, AutonomyPreference::Autonomous) => {
            "Agrupa trabajo seguro de rutina, pero detente ante acciones destructivas, credenciales, publicación, alto costo, legales o de seguridad."
        }
        (Locale::Vi, AutonomyPreference::Cautious) => {
            "Dừng và hỏi trước khi sửa tệp, chạy lệnh hoặc chọn giữa đường sản phẩm mơ hồ."
        }
        (Locale::Vi, AutonomyPreference::Balanced) => {
            "Hành động trực tiếp với việc rõ, rủi ro thấp; xác nhận trước việc rủi ro, phá hủy hoặc mơ hồ."
        }
        (Locale::Vi, AutonomyPreference::Autonomous) => {
            "Gộp việc thường lệ an toàn, nhưng dừng với thao tác phá hủy, thông tin xác thực, xuất bản, chi phí cao, pháp lý hoặc bảo mật."
        }
        (_, AutonomyPreference::Cautious) => {
            "Stop and ask before editing files, running commands, or choosing between ambiguous product paths."
        }
        (_, AutonomyPreference::Balanced) => {
            "Act directly on clear low-risk tasks; confirm before risky, destructive, or ambiguous actions."
        }
        (_, AutonomyPreference::Autonomous) => {
            "Batch routine safe work, then stop for destructive, credential, publishing, high-cost, legal, or security-risk actions."
        }
        (_, AutonomyPreference::Unspecified) => "No standing initiative preference was selected.",
    }
}

fn authority_priority(locale: Locale) -> &'static str {
    match locale {
        Locale::Ja => {
            "現在のユーザー要求とライブツール証拠は、メモリ、古い引き継ぎ、推測より優先される。"
        }
        Locale::ZhHans => "当前用户请求和实时工具证据优先于记忆、陈旧交接和猜测。",
        Locale::ZhHant => "目前使用者請求和即時工具證據優先於記憶、陳舊交接和猜測。",
        Locale::PtBr => {
            "Pedidos atuais do usuário e evidência viva das ferramentas superam memória, handoffs antigos e palpites."
        }
        Locale::Es419 => {
            "Las solicitudes actuales del usuario y la evidencia viva de herramientas superan memoria, handoffs viejos y suposiciones."
        }
        Locale::Vi => {
            "Yêu cầu hiện tại của người dùng và bằng chứng trực tiếp từ công cụ ưu tiên hơn bộ nhớ, handoff cũ và phỏng đoán."
        }
        _ => {
            "Current user requests and live tool evidence outrank memory, stale handoffs, and guesses."
        }
    }
}

fn bounded_freeform_note(input: &str, max_chars: usize) -> String {
    input
        .chars()
        .filter_map(|ch| {
            if ch == '\t' {
                Some(' ')
            } else if ch == '\n' || !ch.is_control() {
                Some(ch)
            } else {
                None
            }
        })
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

fn compact_freeform_preview(note: &str) -> String {
    let compact = note.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = compact.chars().take(96).collect::<String>();
    if compact.chars().count() > 96 {
        preview.push_str("...");
    }
    preview
}

fn freeform_note_line(locale: Locale, note: &str, editing: bool) -> Line<'static> {
    let preview = compact_freeform_preview(note);
    let text = match (locale, editing, preview.is_empty()) {
        (Locale::Ja, true, true) => {
            "F 自由原則：編集中 - 有界の原則を入力または貼り付け、Enter で完了".to_string()
        }
        (Locale::Ja, true, false) => format!("F 自由原則：編集中 - {preview}"),
        (Locale::Ja, false, true) => "F 自由原則：F で有界の原則を入力または貼り付け".to_string(),
        (Locale::Ja, false, false) => format!("F 自由原則：{preview}"),
        (Locale::ZhHans, true, true) => {
            "F 自由原则：正在编辑 - 输入或粘贴有界原则，Enter 完成".to_string()
        }
        (Locale::ZhHans, true, false) => format!("F 自由原则：正在编辑 - {preview}"),
        (Locale::ZhHans, false, true) => "F 自由原则：按 F 输入或粘贴自己的有界原则".to_string(),
        (Locale::ZhHans, false, false) => format!("F 自由原则：{preview}"),
        (Locale::ZhHant, true, true) => {
            "F 自由原則：正在編輯 - 輸入或貼上有界原則，Enter 完成".to_string()
        }
        (Locale::ZhHant, true, false) => format!("F 自由原則：正在編輯 - {preview}"),
        (Locale::ZhHant, false, true) => "F 自由原則：按 F 輸入或貼上自己的有界原則".to_string(),
        (Locale::ZhHant, false, false) => format!("F 自由原則：{preview}"),
        (Locale::PtBr, true, true) => {
            "F Princípio livre: editando - digite ou cole um princípio limitado, Enter para concluir".to_string()
        }
        (Locale::PtBr, true, false) => format!("F Princípio livre: editando - {preview}"),
        (Locale::PtBr, false, true) => {
            "F Princípio livre: pressione F para digitar ou colar um princípio limitado".to_string()
        }
        (Locale::PtBr, false, false) => format!("F Princípio livre: {preview}"),
        (Locale::Es419, true, true) => {
            "F Principio libre: editando - escribe o pega un principio acotado, Enter para terminar".to_string()
        }
        (Locale::Es419, true, false) => format!("F Principio libre: editando - {preview}"),
        (Locale::Es419, false, true) => {
            "F Principio libre: presiona F para escribir o pegar un principio acotado".to_string()
        }
        (Locale::Es419, false, false) => format!("F Principio libre: {preview}"),
        (Locale::Vi, true, true) => {
            "F Nguyên tắc tự do: đang sửa - nhập hoặc dán nguyên tắc có giới hạn, Enter để xong".to_string()
        }
        (Locale::Vi, true, false) => format!("F Nguyên tắc tự do: đang sửa - {preview}"),
        (Locale::Vi, false, true) => {
            "F Nguyên tắc tự do: nhấn F để nhập hoặc dán nguyên tắc có giới hạn".to_string()
        }
        (Locale::Vi, false, false) => format!("F Nguyên tắc tự do: {preview}"),
        (_, true, true) => {
            "F Own words: editing - type or paste a bounded principle, Enter to finish".to_string()
        }
        (_, true, false) => format!("F Own words: editing - {preview}"),
        (_, false, true) => "F Own words: press F to type or paste a bounded principle".to_string(),
        (_, false, false) => format!("F Own words: {preview}"),
    };
    let style = if editing || !preview.is_empty() {
        Style::default().fg(palette::WHALE_ACCENT_PRIMARY)
    } else {
        Style::default().fg(palette::TEXT_MUTED)
    };
    Line::from(Span::styled(text, style))
}

impl SetupWizardView {
    #[cfg(test)]
    #[must_use]
    pub fn new(state: SetupState, locale: Locale) -> Self {
        let selected = initial_step_index(&state);
        Self {
            state,
            selected,
            locale,
            facts: SetupRuntimeFacts::default(),
            guided_draft: GuidedConstitutionDraft::default(),
            freeform_note: String::new(),
            editing_freeform_note: false,
            guided_preview_seen: false,
            existing_preview_seen: false,
            model_draft: None,
            model_draft_label: None,
            runtime_preset: SetupRuntimePreset::default(),
            runtime_preset_preview_seen: false,
            body_scroll: 0,
        }
    }

    #[must_use]
    pub fn new_for_app(app: &App, config: &Config) -> Self {
        Self::new_with_facts(
            load_setup_state_for_app(app, config),
            app.ui_locale,
            SetupRuntimeFacts::from_app_config(app, config),
        )
    }

    #[must_use]
    pub fn new_for_app_at(app: &App, config: &Config, step: SetupStep) -> Self {
        Self::new_at_with_facts(
            load_setup_state_for_app(app, config),
            app.ui_locale,
            step,
            SetupRuntimeFacts::from_app_config(app, config),
        )
    }

    #[cfg(test)]
    #[must_use]
    pub fn state(&self) -> &SetupState {
        &self.state
    }

    #[must_use]
    pub fn selected_step(&self) -> SetupStep {
        STEP_SPECS[self.selected].id()
    }

    fn selected_spec(&self) -> &'static dyn SetupWizardStep {
        &STEP_SPECS[self.selected]
    }

    fn new_with_facts(state: SetupState, locale: Locale, facts: SetupRuntimeFacts) -> Self {
        let selected = initial_step_index(&state);
        Self {
            state,
            selected,
            locale,
            facts,
            guided_draft: GuidedConstitutionDraft::default(),
            freeform_note: String::new(),
            editing_freeform_note: false,
            guided_preview_seen: false,
            existing_preview_seen: false,
            model_draft: None,
            model_draft_label: None,
            runtime_preset: SetupRuntimePreset::default(),
            runtime_preset_preview_seen: false,
            body_scroll: 0,
        }
    }

    fn new_at_with_facts(
        state: SetupState,
        locale: Locale,
        step: SetupStep,
        facts: SetupRuntimeFacts,
    ) -> Self {
        Self {
            state,
            selected: visible_step_index(step),
            locale,
            facts,
            guided_draft: GuidedConstitutionDraft::default(),
            freeform_note: String::new(),
            editing_freeform_note: false,
            guided_preview_seen: false,
            existing_preview_seen: false,
            model_draft: None,
            model_draft_label: None,
            runtime_preset: SetupRuntimePreset::default(),
            runtime_preset_preview_seen: false,
            body_scroll: 0,
        }
    }

    fn move_next(&mut self) {
        self.selected = (self.selected + 1).min(STEP_SPECS.len().saturating_sub(1));
        self.body_scroll = 0;
    }

    fn move_back(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.body_scroll = 0;
    }

    fn commit_selected_status(
        &mut self,
        status: StepStatus,
        message_id: MessageId,
        advance: bool,
    ) -> ViewAction {
        let spec = self.selected_spec();
        let result = match status {
            StepStatus::Skipped => Some("skipped by user"),
            StepStatus::NeedsAction => Some("retry requested; needs action"),
            _ => None,
        };
        let mut entry = StepEntry::new(status, spec.required(), CONSTITUTION_CHECKPOINT_VERSION);
        if let Some(result) = result {
            entry = entry.with_result(result);
        }
        let mut state = self.state.clone();
        state.set_step(spec.id(), entry);
        self.state = state.clone();
        if advance {
            self.move_next();
        }
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, message_id).to_string(),
        })
    }

    fn commit_language_review(&mut self) -> ViewAction {
        let mut state = self.state.clone();
        state.constitution_language = Some(self.locale.tag().to_string());
        state.set_step(
            SetupStep::Language,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(format!("setup locale {}", self.locale.tag())),
        );
        self.state = state.clone();
        self.move_next();
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupLanguageReviewed).to_string(),
        })
    }

    fn commit_provider_model_review(&mut self) -> ViewAction {
        let status = provider::step_status(self.facts.provider_ready);
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::ProviderModel,
            provider::step_entry(
                self.facts.provider_ready,
                CONSTITUTION_CHECKPOINT_VERSION,
                self.facts.provider_result.clone(),
            ),
        );
        self.state = state.clone();
        self.move_next();
        let message_id = if status == StepStatus::Verified {
            MessageId::SetupProviderModelReviewed
        } else {
            MessageId::SetupProviderModelNeedsActionSaved
        };
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, message_id).to_string(),
        })
    }

    fn commit_runtime_posture_review(&mut self) -> ViewAction {
        let mut state = self.state.clone();
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.set_step(
            SetupStep::TrustSandbox,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.runtime_result.clone()),
        );
        self.state = state.clone();
        self.move_next();
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupRuntimePostureReviewed).to_string(),
        })
    }

    fn operate_fleet_facts_ready(&self) -> bool {
        self.state.first_run_ready()
            && self.facts.provider_ready
            && self.facts.operate_runtime_ready
            && self.facts.fleet_roster_ready
    }

    fn commit_operate_fleet_review(&mut self) -> ViewAction {
        let status = if self.operate_fleet_facts_ready() {
            StepStatus::Verified
        } else {
            StepStatus::NeedsAction
        };
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::OperateFleet,
            StepEntry::new(status, false, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.operate_result.clone()),
        );
        self.state = state.clone();
        self.move_next();
        let message_id = if status == StepStatus::Verified {
            MessageId::SetupOperateReviewed
        } else {
            MessageId::SetupOperateNeedsActionSaved
        };
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, message_id).to_string(),
        })
    }

    fn commit_hotbar_review(&mut self) -> ViewAction {
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::Hotbar,
            StepEntry::new(StepStatus::Verified, false, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.hotbar_result.clone()),
        );
        self.state = state.clone();
        self.move_next();
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupHotbarReviewed).to_string(),
        })
    }

    fn commit_tools_mcp_review(&mut self) -> ViewAction {
        // Optional step: empty/off inventories settle as Optional; broken
        // configured tools record NeedsAction without blocking first-run.
        let status = if self.facts.tools_mcp_needs_action {
            StepStatus::NeedsAction
        } else if self.facts.tools_mcp_result.contains("overall=off") {
            StepStatus::Optional
        } else {
            StepStatus::Verified
        };
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::ToolsMcp,
            StepEntry::new(status, false, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.tools_mcp_result.clone()),
        );
        self.state = state.clone();
        self.move_next();
        let message_id = if status == StepStatus::NeedsAction {
            MessageId::SetupToolsMcpNeedsActionSaved
        } else {
            MessageId::SetupToolsMcpReviewed
        };
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, message_id).to_string(),
        })
    }

    fn preview_tools_mcp_on_ramp(&self) -> ViewAction {
        ViewAction::Emit(ViewEvent::OpenTextPager {
            title: tr(self.locale, MessageId::SetupToolsMcpPreviewTitle).to_string(),
            content: tools_mcp_on_ramp_text(self.locale, &self.facts),
        })
    }

    fn preview_remote_runtime_on_ramp(&self) -> ViewAction {
        ViewAction::Emit(ViewEvent::OpenTextPager {
            title: tr(self.locale, MessageId::SetupRemotePreviewTitle).to_string(),
            content: remote_runtime_on_ramp_text(self.locale, &self.facts),
        })
    }

    fn commit_remote_runtime_review(&mut self) -> ViewAction {
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::RemoteRuntime,
            StepEntry::new(StepStatus::Verified, false, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.remote_result.clone()),
        );
        self.state = state.clone();
        self.move_next();
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupRemoteReviewed).to_string(),
        })
    }

    fn commit_persistence_review(&mut self) -> ViewAction {
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::Persistence,
            StepEntry::new(StepStatus::Verified, false, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.persistence.result.clone()),
        );
        self.state = state.clone();
        self.move_next();
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupPersistenceReviewed).to_string(),
        })
    }

    fn select_runtime_preset(&mut self, key: char) -> ViewAction {
        if let Some(preset) = SetupRuntimePreset::from_key(key)
            && preset != self.runtime_preset
        {
            self.runtime_preset = preset;
            self.runtime_preset_preview_seen = false;
        }
        ViewAction::None
    }

    fn preview_runtime_preset(&mut self) -> ViewAction {
        self.runtime_preset_preview_seen = true;
        ViewAction::Emit(ViewEvent::OpenTextPager {
            title: tr(self.locale, MessageId::SetupRuntimePresetPreviewTitle).to_string(),
            content: runtime_preset_preview_text(self.locale, self.runtime_preset, &self.facts),
        })
    }

    fn commit_runtime_preset(&mut self) -> ViewAction {
        if !self.runtime_preset_preview_seen {
            return self.preview_runtime_preset();
        }

        let mut state = self.state.clone();
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.set_step(
            SetupStep::TrustSandbox,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.runtime_preset.result_summary()),
        );
        self.state = state.clone();
        self.move_next();
        ViewAction::Emit(ViewEvent::SetupRuntimePresetApplyRequested {
            preset: self.runtime_preset,
            state,
            message: tr(self.locale, MessageId::SetupRuntimePresetApplied).to_string(),
        })
    }

    fn commit_setup_report(&mut self) -> ViewAction {
        let mut state = self.state.clone();
        let status = if setup_report_ready(&state) {
            StepStatus::Verified
        } else {
            StepStatus::NeedsAction
        };
        state.set_step(
            SetupStep::Verification,
            StepEntry::new(status, false, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(setup_report_result(&state, &self.facts)),
        );
        self.state = state.clone();
        ViewAction::Emit(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupReportRecorded).to_string(),
        })
    }

    fn commit_guided_constitution(&mut self) -> ViewAction {
        if !self.guided_preview_seen {
            return self.preview_guided_constitution();
        }

        let (constitution, authoring) = match self.model_draft.as_deref() {
            // Model drafts arrive sanitized + bounded from the untrusted-JSON
            // gate; ratify exactly what was previewed.
            Some(draft) => (draft.clone(), ConstitutionAuthoring::ModelDrafted),
            None => (
                self.guided_draft
                    .to_constitution_with_freeform(self.locale, self.freeform_note_for_draft()),
                ConstitutionAuthoring::Guided,
            ),
        };
        let mut state = self.state.clone();
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::GuidedCustom,
        );
        state.constitution_language = constitution.language.clone();
        state.constitution_source = ConstitutionSource::UserGlobal;
        state.constitution_validity = ConstitutionValidity::Valid;
        state.constitution_authoring = Some(authoring);
        state.constitution_preview_hash = Some(constitution.preview_hash());
        state.constitution_preview_version =
            state.constitution_preview_version.saturating_add(1).max(1);
        let hash = state
            .constitution_preview_hash
            .as_deref()
            .unwrap_or("unknown");
        let result = match authoring {
            ConstitutionAuthoring::ModelDrafted => format!(
                "model-drafted constitution ratified ({}) preview_hash={hash}",
                self.model_draft_label.as_deref().unwrap_or("model")
            ),
            ConstitutionAuthoring::Guided => {
                format!("guided custom constitution preview_hash={hash}")
            }
        };
        state.set_step(
            SetupStep::Constitution,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(result),
        );
        self.state = state.clone();
        ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            state,
            message: tr(self.locale, MessageId::SetupCheckpointDoneGuided).to_string(),
        })
    }

    fn preview_guided_constitution(&mut self) -> ViewAction {
        self.guided_preview_seen = true;
        let (constitution, provenance) = match self.model_draft.as_deref() {
            Some(draft) => (
                draft.clone(),
                DraftProvenance::Model(
                    self.model_draft_label
                        .clone()
                        .unwrap_or_else(|| "model".to_string()),
                ),
            ),
            None => (
                self.guided_draft
                    .to_constitution_with_freeform(self.locale, self.freeform_note_for_draft()),
                DraftProvenance::Guided,
            ),
        };
        ViewAction::Emit(ViewEvent::OpenTextPager {
            title: ratification_preview_title(self.locale).to_string(),
            content: constitution_ratification_text(self.locale, &constitution, &provenance),
        })
    }

    fn cycle_guided_answer(&mut self, key: char) -> ViewAction {
        if self.guided_draft.cycle(key) {
            self.guided_preview_seen = false;
            // Answers changed under the draft: the model draft is stale law
            // and must be re-drafted or replaced by the guided rendering.
            self.model_draft = None;
            self.model_draft_label = None;
        }
        ViewAction::None
    }

    /// `A` on the constitution step: ask the first configured model to draft.
    /// Requires a ready provider route; otherwise the key is inert and the
    /// deterministic guided flow stands untouched.
    fn request_model_draft(&self) -> ViewAction {
        if !self.facts.provider_ready {
            return ViewAction::None;
        }
        ViewAction::Emit(ViewEvent::SetupConstitutionModelDraftRequested {
            draft: self.guided_draft,
            freeform_note: self.freeform_note_for_draft().map(str::to_string),
            locale: self.locale,
        })
    }

    fn toggle_freeform_edit(&mut self) -> ViewAction {
        if self.selected_step() == SetupStep::Constitution {
            self.editing_freeform_note = !self.editing_freeform_note;
        }
        ViewAction::None
    }

    fn freeform_note_for_draft(&self) -> Option<&str> {
        let note = self.freeform_note.trim();
        (!note.is_empty()).then_some(note)
    }

    fn append_freeform_note_text(&mut self, text: &str) {
        let mut next = self.freeform_note.clone();
        next.push_str(text);
        self.freeform_note = bounded_freeform_note(&next, MAX_NOTES_LEN);
        self.guided_preview_seen = false;
        self.model_draft = None;
        self.model_draft_label = None;
    }

    fn handle_freeform_note_key(&mut self, key: KeyEvent) -> Option<ViewAction> {
        if self.selected_step() != SetupStep::Constitution || !self.editing_freeform_note {
            return None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.editing_freeform_note = false;
                Some(ViewAction::None)
            }
            KeyCode::Backspace => {
                self.freeform_note.pop();
                self.guided_preview_seen = false;
                self.model_draft = None;
                self.model_draft_label = None;
                Some(ViewAction::None)
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                let mut buf = [0; 4];
                self.append_freeform_note_text(c.encode_utf8(&mut buf));
                Some(ViewAction::None)
            }
            _ => Some(ViewAction::None),
        }
    }

    /// Install a model-drafted constitution (already sanitized + bounded by
    /// the untrusted-JSON gate) and return the `(title, content)` of the
    /// ratification preview the host must open in the same breath — that is
    /// what satisfies the preview gate. Ratifying still takes the explicit
    /// `G` keypress afterwards.
    #[must_use]
    pub(crate) fn install_model_draft(
        &mut self,
        constitution: Box<UserConstitution>,
        model_label: String,
    ) -> (String, String) {
        let content = constitution_ratification_text(
            self.locale,
            &constitution,
            &DraftProvenance::Model(model_label.clone()),
        );
        self.model_draft = Some(constitution);
        self.model_draft_label = Some(model_label);
        self.guided_preview_seen = true;
        (ratification_preview_title(self.locale).to_string(), content)
    }

    fn commit_constitution(&self, kind: SetupCommitKind) -> ViewAction {
        let choice = match kind {
            SetupCommitKind::BundledConstitution => ConstitutionChoice::Bundled,
            SetupCommitKind::DeferredConstitution => ConstitutionChoice::Deferred,
        };
        let mut state = self.state.clone();
        state.complete_constitution_checkpoint(CONSTITUTION_CHECKPOINT_VERSION, choice);
        state.constitution_source = ConstitutionSource::Bundled;
        state.constitution_validity = ConstitutionValidity::Unknown;
        state.constitution_authoring = None;
        state.constitution_preview_hash = None;
        state.set_step(
            SetupStep::Constitution,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(match kind {
                    SetupCommitKind::BundledConstitution => "bundled/default constitution",
                    SetupCommitKind::DeferredConstitution => "checkpoint deferred; bundled applies",
                }),
        );
        let message_id = match kind {
            SetupCommitKind::BundledConstitution => MessageId::SetupCheckpointDoneBundled,
            SetupCommitKind::DeferredConstitution => MessageId::SetupCheckpointDeferred,
        };
        ViewAction::EmitAndClose(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, message_id).to_string(),
        })
    }

    /// Complete the checkpoint by keeping the existing valid
    /// `constitution.json` exactly as it stands (#3794). First `K` previews
    /// the rendered law; second `K` records the choice. The file is never
    /// rewritten — only `setup_state.json` changes, through the same commit
    /// event as every other completion.
    fn commit_keep_existing_constitution(&mut self) -> ViewAction {
        if self.facts.constitution_file != SetupConstitutionFileState::Loaded {
            return ViewAction::None;
        }
        // Re-read the live file so a stale card cannot ratify a file that
        // has since become invalid; any non-loaded state leaves the key inert.
        let Ok(load) = UserConstitution::load() else {
            return ViewAction::None;
        };
        let Some(constitution) = load.constitution() else {
            return ViewAction::None;
        };
        if !self.existing_preview_seen {
            self.existing_preview_seen = true;
            let content = constitution_ratification_text(
                self.locale,
                constitution,
                &DraftProvenance::Existing,
            );
            return ViewAction::Emit(ViewEvent::OpenTextPager {
                title: ratification_preview_title(self.locale).to_string(),
                content,
            });
        }
        let mut state = self.state.clone();
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::GuidedCustom,
        );
        state.constitution_source = ConstitutionSource::UserGlobal;
        state.constitution_validity = ConstitutionValidity::Valid;
        state.constitution_preview_hash = Some(constitution.preview_hash());
        state.set_step(
            SetupStep::Constitution,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result("existing constitution kept unchanged"),
        );
        ViewAction::EmitAndClose(ViewEvent::SetupStateCommitRequested {
            state,
            message: tr(self.locale, MessageId::SetupCheckpointDoneKept).to_string(),
        })
    }

    fn status_label(&self, status: StepStatus) -> Cow<'static, str> {
        tr(
            self.locale,
            match status {
                StepStatus::NotStarted => MessageId::SetupStatusNotStarted,
                StepStatus::Recommended => MessageId::SetupStatusRecommended,
                StepStatus::Optional => MessageId::SetupStatusOptional,
                StepStatus::Deferred => MessageId::SetupStatusDeferred,
                StepStatus::InProgress => MessageId::SetupStatusInProgress,
                StepStatus::NeedsAction => MessageId::SetupStatusNeedsAction,
                StepStatus::Verified => MessageId::SetupStatusVerified,
                StepStatus::Skipped => MessageId::SetupStatusSkipped,
                StepStatus::Failed => MessageId::SetupStatusFailed,
            },
        )
    }
}

impl ModalView for SetupWizardView {
    fn kind(&self) -> ModalKind {
        ModalKind::SetupWizard
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if let Some(action) = self.handle_freeform_note_key(key) {
            return action;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Left | KeyCode::Char('b') => {
                self.move_back();
                ViewAction::None
            }
            KeyCode::Right | KeyCode::Char('n') => {
                self.move_next();
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.body_scroll = self.body_scroll.saturating_sub(8);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.body_scroll = self.body_scroll.saturating_add(8);
                ViewAction::None
            }
            KeyCode::Up => {
                self.move_back();
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_next();
                ViewAction::None
            }
            KeyCode::Char('s') => {
                self.commit_selected_status(StepStatus::Skipped, MessageId::SetupStepSkipped, true)
            }
            KeyCode::Char('r') if self.selected_step() == SetupStep::ToolsMcp => {
                self.preview_tools_mcp_on_ramp()
            }
            KeyCode::Char('r') if self.selected_step() == SetupStep::RemoteRuntime => {
                self.preview_remote_runtime_on_ramp()
            }
            KeyCode::Char('r') => self.commit_selected_status(
                StepStatus::NeedsAction,
                MessageId::SetupStepRetryRecorded,
                false,
            ),
            KeyCode::Char('g') if self.selected_step() == SetupStep::Constitution => {
                self.commit_guided_constitution()
            }
            KeyCode::Char('p') if self.selected_step() == SetupStep::ProviderModel => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenProviderRequested)
            }
            KeyCode::Char('m') if self.selected_step() == SetupStep::ProviderModel => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenModelRequested)
            }
            KeyCode::Char('p') if self.selected_step() == SetupStep::OperateFleet => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenProviderRequested)
            }
            KeyCode::Char('f') if self.selected_step() == SetupStep::OperateFleet => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenFleetRequested)
            }
            KeyCode::Char('h') if self.selected_step() == SetupStep::Hotbar => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenHotbarRequested)
            }
            KeyCode::Char('m') if self.selected_step() == SetupStep::TrustSandbox => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenModeRequested)
            }
            KeyCode::Char('c') if self.selected_step() == SetupStep::TrustSandbox => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenConfigRequested)
            }
            KeyCode::Char(key @ ('1' | '2' | '3'))
                if self.selected_step() == SetupStep::TrustSandbox =>
            {
                self.select_runtime_preset(key)
            }
            KeyCode::Char('a') if self.selected_step() == SetupStep::TrustSandbox => {
                self.commit_runtime_preset()
            }
            KeyCode::Char(key @ ('1' | '2' | '3' | '4' | '5' | '6'))
                if self.selected_step() == SetupStep::Constitution =>
            {
                self.cycle_guided_answer(key)
            }
            KeyCode::Char('a') if self.selected_step() == SetupStep::Constitution => {
                self.request_model_draft()
            }
            KeyCode::Char('f') if self.selected_step() == SetupStep::Constitution => {
                self.toggle_freeform_edit()
            }
            KeyCode::Char('k') if self.selected_step() == SetupStep::Constitution => {
                self.commit_keep_existing_constitution()
            }
            KeyCode::Char('u') => self.commit_constitution(SetupCommitKind::BundledConstitution),
            KeyCode::Char('d') => self.commit_constitution(SetupCommitKind::DeferredConstitution),
            KeyCode::Enter if self.selected_step() == SetupStep::Constitution => {
                self.commit_constitution(SetupCommitKind::BundledConstitution)
            }
            KeyCode::Enter if self.selected_step() == SetupStep::Language => {
                self.commit_language_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::ProviderModel => {
                self.commit_provider_model_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::TrustSandbox => {
                self.commit_runtime_posture_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::OperateFleet => {
                self.commit_operate_fleet_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::Hotbar => {
                self.commit_hotbar_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::ToolsMcp => {
                self.commit_tools_mcp_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::RemoteRuntime => {
                self.commit_remote_runtime_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::Persistence => {
                self.commit_persistence_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::Verification => {
                self.commit_setup_report()
            }
            KeyCode::Enter => {
                self.move_next();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if self.selected_step() != SetupStep::Constitution {
            return false;
        }
        self.append_freeform_note_text(text);
        true
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_area = centered_modal_area(area, 92, 30, 56, 16);
        render_modal_surface(area, popup_area, buf);
        let progress = format!(
            "{} {}/{}",
            tr(self.locale, MessageId::SetupWizardProgress),
            self.selected + 1,
            STEP_SPECS.len()
        );
        let block = Block::default()
            .title(Line::from(Span::styled(
                format!(" {} ", tr(self.locale, MessageId::SetupWizardTitle)),
                Style::default()
                    .fg(palette::WHALE_ACCENT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(Span::styled(
                format!(" {progress} "),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_PANEL))
            .padding(Padding::new(2, 2, 1, 1));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);
        let mut hints = vec![
            ActionHint::new("B", tr(self.locale, MessageId::SetupActionBack).to_string()),
            ActionHint::new(
                "N",
                tr(self.locale, MessageId::SetupActionContinue).to_string(),
            ),
            ActionHint::new("S", tr(self.locale, MessageId::SetupActionSkip).to_string()),
            ActionHint::new(
                "R",
                tr(self.locale, MessageId::SetupActionRetry).to_string(),
            ),
            ActionHint::new(
                "PgUp/Dn",
                tr(self.locale, MessageId::SetupActionScrollBody).to_string(),
            ),
        ];
        if self.selected_step() == SetupStep::Constitution {
            hints.push(ActionHint::new(
                "1-6",
                tr(self.locale, MessageId::SetupActionTuneGuided).to_string(),
            ));
            if self.facts.provider_ready {
                hints.push(ActionHint::new(
                    "A",
                    tr(self.locale, MessageId::SetupActionModelDraft).to_string(),
                ));
            }
            hints.push(ActionHint::new(
                "G",
                tr(self.locale, MessageId::SetupActionGuided).to_string(),
            ));
            hints.push(ActionHint::new(
                "F",
                tr(self.locale, MessageId::SetupActionFreeform).to_string(),
            ));
            if self.facts.constitution_file == SetupConstitutionFileState::Loaded {
                hints.push(ActionHint::new(
                    "K",
                    tr(self.locale, MessageId::SetupActionKeepExisting).to_string(),
                ));
            }
        } else if self.selected_step() == SetupStep::ProviderModel {
            hints.push(ActionHint::new(
                "P",
                tr(self.locale, MessageId::SetupActionProvider).to_string(),
            ));
            hints.push(ActionHint::new(
                "M",
                tr(self.locale, MessageId::SetupActionModel).to_string(),
            ));
        } else if self.selected_step() == SetupStep::OperateFleet {
            hints.push(ActionHint::new(
                "P",
                tr(self.locale, MessageId::SetupActionProvider).to_string(),
            ));
            hints.push(ActionHint::new(
                "F",
                tr(self.locale, MessageId::SetupActionFleet).to_string(),
            ));
        } else if self.selected_step() == SetupStep::Hotbar {
            hints.push(ActionHint::new(
                "H",
                tr(self.locale, MessageId::SetupActionHotbar).to_string(),
            ));
        } else if self.selected_step() == SetupStep::RemoteRuntime {
            hints.push(ActionHint::new(
                "R",
                tr(self.locale, MessageId::SetupActionRemote).to_string(),
            ));
        } else if self.selected_step() == SetupStep::TrustSandbox {
            hints.push(ActionHint::new(
                "1-3",
                tr(self.locale, MessageId::SetupActionRuntimePreset).to_string(),
            ));
            hints.push(ActionHint::new(
                "A",
                tr(self.locale, MessageId::SetupActionApplyRuntimePreset).to_string(),
            ));
            hints.push(ActionHint::new(
                "M",
                tr(self.locale, MessageId::SetupActionMode).to_string(),
            ));
            hints.push(ActionHint::new(
                "C",
                tr(self.locale, MessageId::SetupActionConfig).to_string(),
            ));
        }
        hints.extend([
            ActionHint::new(
                "U",
                tr(self.locale, MessageId::SetupActionUseBundled).to_string(),
            ),
            ActionHint::new(
                "D",
                tr(self.locale, MessageId::SetupActionDefer).to_string(),
            ),
            ActionHint::new(
                "Esc",
                tr(self.locale, MessageId::SetupActionCancel).to_string(),
            ),
        ]);
        let content_area = render_modal_footer(inner, buf, &hints);
        let spec = self.selected_spec();
        let mut lines = vec![
            Line::from(Span::styled(
                tr(self.locale, spec.title_id()).to_string(),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::raw(tr(self.locale, spec.why_id()).to_string())),
            Line::from(""),
        ];
        lines.extend(self.selected_step_detail_lines());
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::SetupWizardWhy).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(""));
        for (idx, step) in STEP_SPECS.iter().enumerate() {
            let selected = idx == self.selected;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(tr(self.locale, step.title_id()).to_string(), style),
                Span::raw("  "),
                Span::styled(
                    self.status_label(self.state.status(step.id())).to_string(),
                    Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::raw(
            tr(self.locale, MessageId::SetupCheckpointLayerOrder).to_string(),
        )));
        let wrap_width = usize::from(content_area.width).max(1);
        let visual_rows: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(wrap_width).max(1))
            .sum();
        let visible_rows = usize::from(content_area.height).max(1);
        let max_scroll = visual_rows.saturating_sub(visible_rows);
        let scroll = self.body_scroll.min(max_scroll);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0))
            .render(content_area, buf);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

impl SetupWizardView {
    fn selected_step_detail_lines(&self) -> Vec<Line<'static>> {
        match self.selected_step() {
            SetupStep::ProviderModel => self.provider_model_detail_lines(),
            SetupStep::TrustSandbox => self.runtime_posture_detail_lines(),
            SetupStep::Constitution => self.constitution_detail_lines(),
            SetupStep::OperateFleet => self.operate_fleet_detail_lines(),
            SetupStep::Hotbar => self.hotbar_detail_lines(),
            SetupStep::ToolsMcp => self.tools_mcp_detail_lines(),
            SetupStep::RemoteRuntime => self.remote_runtime_detail_lines(),
            SetupStep::Persistence => self.persistence_detail_lines(),
            SetupStep::Verification => self.verification_detail_lines(),
            _ => Vec::new(),
        }
    }

    fn provider_model_detail_lines(&self) -> Vec<Line<'static>> {
        vec![
            self.detail_row(MessageId::SetupCardRouteLabel, &self.facts.provider),
            self.detail_row(MessageId::SetupCardModelLabel, &self.facts.model),
            self.detail_row(MessageId::SetupCardAuthLabel, &self.facts.auth),
            self.detail_row(MessageId::SetupCardHealthLabel, &self.facts.health),
            Line::from(Span::styled(
                tr(
                    self.locale,
                    if self.facts.provider_ready {
                        MessageId::SetupProviderModelReadyHint
                    } else {
                        MessageId::SetupProviderModelNeedsActionHint
                    },
                )
                .to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ]
    }

    fn constitution_detail_lines(&self) -> Vec<Line<'static>> {
        let choice = constitution_choice_label(self.state.constitution_choice);
        let source = constitution_source_label(self.state.constitution_source);
        let validity = constitution_validity_label(self.state.constitution_validity);
        let source_state = format!("{source}; validity {validity}");
        let existing_file = self
            .facts
            .constitution_file
            .label(self.state.constitution_choice, self.locale);
        let expert_override = self.facts.expert_override.label(self.locale);
        let preview = self
            .state
            .constitution_preview_hash
            .as_deref()
            .unwrap_or("not accepted yet")
            .to_string();
        let mut lines = vec![
            self.detail_row(MessageId::SetupConstitutionChoiceLabel, choice),
            self.detail_row(MessageId::SetupConstitutionSourceLabel, &source_state),
            self.detail_row(MessageId::SetupConstitutionPreviewLabel, &preview),
            self.detail_row(MessageId::SetupConstitutionExistingLabel, &existing_file),
            self.detail_row(
                MessageId::SetupConstitutionExpertOverrideLabel,
                &expert_override,
            ),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupConstitutionGuidedAnswersHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            self.guided_answer_pair(
                (
                    "1",
                    MessageId::SetupConstitutionPurposeLabel,
                    &self.guided_draft.purpose.label(self.locale),
                ),
                (
                    "2",
                    MessageId::SetupConstitutionAutonomyLabel,
                    autonomy_label(self.guided_draft.autonomy, self.locale),
                ),
            ),
            self.guided_answer_pair(
                (
                    "3",
                    MessageId::SetupConstitutionEvidenceLabel,
                    &self.guided_draft.evidence.label(self.locale),
                ),
                (
                    "4",
                    MessageId::SetupConstitutionCommunicationLabel,
                    self.guided_draft.communication.label(self.locale),
                ),
            ),
            self.guided_answer_single(
                "5",
                MessageId::SetupConstitutionPrivacyLabel,
                self.guided_draft.privacy.label(self.locale),
            ),
            self.guided_answer_single(
                "6",
                MessageId::SetupConstitutionPrinciplesLabel,
                self.guided_draft.principles.label(self.locale),
            ),
            freeform_note_line(self.locale, &self.freeform_note, self.editing_freeform_note),
        ];
        if self.facts.constitution_file == SetupConstitutionFileState::Loaded {
            lines.push(Line::from(Span::styled(
                keep_existing_invitation_line(self.locale),
                Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
            )));
        }
        if let Some(label) = self
            .model_draft_label
            .as_deref()
            .filter(|_| self.model_draft.is_some())
        {
            lines.push(Line::from(Span::styled(
                model_draft_ready_line(self.locale, label),
                Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
            )));
        } else if self.facts.provider_ready {
            lines.push(Line::from(Span::styled(
                model_draft_invitation_line(self.locale, &self.facts.model),
                Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
            )));
        }
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::SetupConstitutionGuidedHint).to_string(),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines
    }

    fn runtime_posture_detail_lines(&self) -> Vec<Line<'static>> {
        let project_override = self
            .facts
            .project_override_warning
            .clone()
            .unwrap_or_else(|| {
                tr(self.locale, MessageId::SetupRuntimeProjectOverrideNone).to_string()
            });
        let mut lines = vec![
            self.detail_row(MessageId::SetupCardIntentLabel, &self.facts.work_intent),
            self.detail_row(MessageId::SetupCardApprovalLabel, &self.facts.approval),
            self.detail_row(MessageId::SetupCardShellLabel, &self.facts.shell),
            self.detail_row(MessageId::SetupCardTrustLabel, &self.facts.trust),
            self.detail_row(MessageId::SetupCardSandboxLabel, &self.facts.sandbox),
            self.detail_row(MessageId::SetupCardNetworkLabel, &self.facts.network),
            self.detail_row(
                MessageId::SetupRuntimePresetSelectedLabel,
                &runtime_preset_summary(self.locale, self.runtime_preset),
            ),
            self.detail_row(
                MessageId::SetupRuntimePresetDiffLabel,
                &runtime_preset_inline_diff(self.runtime_preset, &self.facts),
            ),
            self.detail_row(
                MessageId::SetupRuntimeProjectOverrideLabel,
                &project_override,
            ),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupRuntimePostureBoundary).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupRuntimePresetSafetyFloor).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            self.setup_review_hint_line(
                MessageId::SetupRuntimePostureReviewHint,
                Some("Press M for work mode or C for config."),
            ),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupRuntimePresetApplyHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ];
        for (idx, preset) in SetupRuntimePreset::ALL.iter().enumerate() {
            let marker = if *preset == self.runtime_preset {
                ">"
            } else {
                " "
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker} {}. {}",
                    idx + 1,
                    runtime_preset_summary(self.locale, *preset)
                ),
                Style::default().fg(if *preset == self.runtime_preset {
                    palette::TEXT_PRIMARY
                } else {
                    palette::TEXT_MUTED
                }),
            )));
        }
        lines
    }

    fn operate_fleet_detail_lines(&self) -> Vec<Line<'static>> {
        let route = format!("{} / {}", self.facts.provider, self.facts.model);
        let readiness = self.ready_label(self.operate_fleet_facts_ready());
        vec![
            self.detail_row(MessageId::SetupCardRouteLabel, &route),
            self.detail_row(MessageId::SetupCardAuthLabel, &self.facts.auth),
            self.detail_row(
                MessageId::SetupOperateRuntimeLabel,
                &self.facts.operate_runtime_result,
            ),
            self.detail_row(
                MessageId::SetupOperateRosterLabel,
                &self.facts.fleet_roster_result,
            ),
            self.detail_row(
                MessageId::SetupOperateConcurrencyLabel,
                &self.facts.operate_concurrency_result,
            ),
            self.detail_row(MessageId::SetupOperateReadinessLabel, &readiness),
            self.setup_review_hint_line(MessageId::SetupOperateReviewHint, None),
        ]
    }

    fn hotbar_detail_lines(&self) -> Vec<Line<'static>> {
        vec![
            self.detail_row(
                MessageId::SetupHotbarBindingsLabel,
                &self.facts.hotbar_bindings_result,
            ),
            self.detail_row(
                MessageId::SetupHotbarActionsLabel,
                &self.facts.hotbar_actions_result,
            ),
            self.setup_review_hint_line(
                MessageId::SetupHotbarReviewHint,
                Some("Press H to customize slots."),
            ),
        ]
    }

    fn tools_mcp_detail_lines(&self) -> Vec<Line<'static>> {
        vec![
            self.detail_row(
                MessageId::SetupToolsMcpServersLabel,
                &self.facts.tools_mcp_servers_result,
            ),
            self.detail_row(
                MessageId::SetupToolsMcpSkillsLabel,
                &self.facts.tools_mcp_skills_result,
            ),
            self.detail_row(
                MessageId::SetupToolsMcpToolsLabel,
                &self.facts.tools_mcp_tools_result,
            ),
            self.detail_row(
                MessageId::SetupToolsMcpPluginsLabel,
                &self.facts.tools_mcp_plugins_result,
            ),
            self.detail_row(
                MessageId::SetupToolsMcpHotbarLabel,
                &self.facts.tools_mcp_hotbar_result,
            ),
            self.setup_review_hint_line(
                MessageId::SetupToolsMcpReviewHint,
                Some("Press R for safe on-ramps (no auto-run)."),
            ),
        ]
    }

    fn remote_runtime_detail_lines(&self) -> Vec<Line<'static>> {
        vec![
            self.detail_row(
                MessageId::SetupRemoteCloudsLabel,
                &self.facts.remote_clouds_result,
            ),
            self.detail_row(
                MessageId::SetupRemoteBridgesLabel,
                &self.facts.remote_bridges_result,
            ),
            self.detail_row(
                MessageId::SetupRemoteProvidersLabel,
                &self.facts.remote_providers_result,
            ),
            self.detail_row(
                MessageId::SetupRemoteModeLabel,
                &self.facts.remote_mode_result,
            ),
            self.setup_review_hint_line(
                MessageId::SetupRemoteReviewHint,
                Some("Press R to preview."),
            ),
        ]
    }

    fn persistence_detail_lines(&self) -> Vec<Line<'static>> {
        vec![
            self.detail_row(
                MessageId::SetupPersistenceHomeLabel,
                &self.facts.persistence.home_result,
            ),
            self.detail_row(
                MessageId::SetupPersistenceConfigLabel,
                &self.facts.persistence.config_result,
            ),
            self.detail_row(
                MessageId::SetupPersistenceStateLabel,
                &self.facts.persistence.state_result,
            ),
            self.detail_row(
                MessageId::SetupPersistenceConstitutionLabel,
                &self.facts.persistence.constitution_result,
            ),
            self.detail_row(
                MessageId::SetupPersistenceMemoryLabel,
                &self.facts.persistence.memory_result,
            ),
            self.detail_row(
                MessageId::SetupPersistenceNotesLabel,
                &self.facts.persistence.notes_result,
            ),
            self.setup_review_hint_line(MessageId::SetupPersistenceReviewHint, None),
        ]
    }

    fn verification_detail_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            self.detail_row(
                MessageId::SetupReportFirstRunLabel,
                &self.ready_label(self.state.first_run_ready()),
            ),
            self.detail_row(
                MessageId::SetupReportUpdateLabel,
                &self.ready_label(self.state.update_ready(CONSTITUTION_CHECKPOINT_VERSION)),
            ),
            self.detail_row(
                MessageId::SetupReportOperateLabel,
                &self.ready_label(self.state.operate_ready()),
            ),
            self.detail_row(
                MessageId::SetupReportSourceLabel,
                &self.state_source_label(),
            ),
            self.detail_row(
                MessageId::SetupReportAutonomyLabel,
                &self.facts.constitution_autonomy,
            ),
            self.detail_row(
                MessageId::SetupReportRuntimePostureLabel,
                &self.facts.runtime_result,
            ),
            Line::from(""),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupReportRowsLabel).to_string(),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            )),
        ];

        for spec in STEP_SPECS {
            let step = spec.id();
            let entry = self.state.steps.get(&step);
            let required = entry.map_or(spec.required(), |entry| entry.required);
            let required_label = if required {
                tr(self.locale, MessageId::SetupReportRequired)
            } else {
                tr(self.locale, MessageId::SetupReportOptional)
            };
            let mut value = format!(
                "{} ({})",
                self.status_label(self.state.status(step)),
                required_label
            );
            if let Some(version) = entry.and_then(|entry| entry.version.as_deref()) {
                value.push_str(&format!(" · {version}"));
            }
            if let Some(result) = entry.and_then(|entry| entry.result.as_deref()) {
                value.push_str(&format!(" · {result}"));
            }
            lines.push(self.detail_row(spec.title_id(), &value));
        }

        lines.push(Line::from(""));
        let next_action = tr(self.locale, self.next_action_id()).to_string();
        lines.push(self.detail_row(MessageId::SetupReportNextActionLabel, &next_action));
        lines
    }

    fn setup_review_hint_line(
        &self,
        hint_id: MessageId,
        english_action: Option<&'static str>,
    ) -> Line<'static> {
        let hint = if self.locale == Locale::En {
            let mut hint = "Enter records this setup snapshot.".to_string();
            if let Some(action) = english_action {
                hint.push(' ');
                hint.push_str(action);
            }
            hint
        } else {
            tr(self.locale, hint_id).to_string()
        };
        Line::from(Span::styled(hint, Style::default().fg(palette::TEXT_MUTED)))
    }

    fn ready_label(&self, ready: bool) -> String {
        if ready {
            tr(self.locale, MessageId::SetupReportReady).to_string()
        } else {
            tr(self.locale, MessageId::SetupStatusNeedsAction).to_string()
        }
    }

    fn state_source_label(&self) -> String {
        if self.state.inherited {
            tr(self.locale, MessageId::SetupReportInherited).to_string()
        } else {
            tr(self.locale, MessageId::SetupReportPersisted).to_string()
        }
    }

    fn next_action_id(&self) -> MessageId {
        if !self.state.update_ready(CONSTITUTION_CHECKPOINT_VERSION) {
            return MessageId::SetupReportNextActionConstitution;
        }
        if !matches!(
            self.state.status(SetupStep::ProviderModel),
            StepStatus::Verified | StepStatus::NeedsAction
        ) {
            return MessageId::SetupReportNextActionProvider;
        }
        if !self.state.runtime_posture_source.is_reviewed() {
            return MessageId::SetupReportNextActionRuntime;
        }
        if !self.state.first_run_ready() {
            return MessageId::SetupReportNextActionRequired;
        }
        if !self.state.operate_ready() {
            return MessageId::SetupReportNextActionOperate;
        }
        MessageId::SetupReportNextActionNone
    }

    fn detail_row(&self, label: MessageId, value: &str) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("{} ", tr(self.locale, label)),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(value.to_string()),
        ])
    }

    fn guided_answer_pair(
        &self,
        left: (&str, MessageId, &str),
        right: (&str, MessageId, &str),
    ) -> Line<'static> {
        let label_style = Style::default()
            .fg(palette::TEXT_MUTED)
            .add_modifier(Modifier::BOLD);
        Line::from(vec![
            Span::styled(
                format!("{} {} ", left.0, tr(self.locale, left.1)),
                label_style,
            ),
            Span::raw(left.2.to_string()),
            Span::styled("  ·  ", Style::default().fg(palette::TEXT_MUTED)),
            Span::styled(
                format!("{} {} ", right.0, tr(self.locale, right.1)),
                label_style,
            ),
            Span::raw(right.2.to_string()),
        ])
    }

    fn guided_answer_single(&self, key: &str, label: MessageId, value: &str) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                format!("{key} {} ", tr(self.locale, label)),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(value.to_string()),
        ])
    }
}

fn setup_report_ready(state: &SetupState) -> bool {
    state.first_run_ready() || state.update_ready(CONSTITUTION_CHECKPOINT_VERSION)
}

fn runtime_preset_summary(locale: Locale, preset: SetupRuntimePreset) -> String {
    format!(
        "{} - {}",
        tr(locale, preset.title_id()),
        tr(locale, preset.description_id())
    )
}

fn runtime_preset_inline_diff(preset: SetupRuntimePreset, facts: &SetupRuntimeFacts) -> String {
    runtime_preset_diff_rows(preset, facts).join("; ")
}

fn runtime_preset_preview_text(
    locale: Locale,
    preset: SetupRuntimePreset,
    facts: &SetupRuntimeFacts,
) -> String {
    let mut lines = vec![
        tr(locale, MessageId::SetupRuntimePresetPreviewTitle).to_string(),
        runtime_preset_summary(locale, preset),
        String::new(),
        tr(locale, MessageId::SetupRuntimePresetDiffLabel).to_string(),
    ];
    lines.extend(
        runtime_preset_diff_rows(preset, facts)
            .into_iter()
            .map(|row| format!("- {row}")),
    );
    lines.extend([
        String::new(),
        tr(locale, MessageId::SetupRuntimePostureBoundary).to_string(),
        tr(locale, MessageId::SetupRuntimePresetSafetyFloor).to_string(),
        tr(locale, MessageId::SetupRuntimePresetApplyHint).to_string(),
    ]);
    lines.join("\n")
}

fn runtime_preset_diff_rows(preset: SetupRuntimePreset, facts: &SetupRuntimeFacts) -> Vec<String> {
    let approval_target = preset.approval_policy().map_or_else(
        || "unchanged; Full Access derives bypass from default_mode".to_string(),
        ToString::to_string,
    );
    let mut rows = vec![
        format!(
            "settings.default_mode: {} -> {}",
            facts.default_mode,
            preset.default_mode()
        ),
        format!(
            "config.approval_policy: {} -> {}",
            facts.approval_policy_value, approval_target
        ),
        format!(
            "config.allow_shell: {} -> {}",
            facts.allow_shell_enabled,
            preset.allow_shell()
        ),
        format!(
            "config.sandbox_mode: {} -> {}",
            facts.sandbox_mode_value,
            preset.sandbox_mode()
        ),
        format!(
            "config.network.default: {} -> unchanged",
            facts.network_default_value
        ),
        format!("workspace trust: {} -> unchanged", facts.trust),
    ];
    if let Some(warning) = facts.project_override_warning.as_deref() {
        rows.push(format!("project override warning: {warning}"));
    }
    rows
}

fn project_runtime_override_warning(workspace: &Path, locale: Locale) -> Option<String> {
    let project = codewhale_config::load_project_config(workspace)?;
    let mut fields = Vec::new();
    if let Some(policy) = project.approval_policy.as_deref() {
        fields.push(format!("approval_policy={policy}"));
    }
    if let Some(mode) = project.sandbox_mode.as_deref() {
        fields.push(format!("sandbox_mode={mode}"));
    }
    if fields.is_empty() {
        return None;
    }
    Some(match locale {
        Locale::ZhHans => format!(
            "此工作区的项目配置包含 {}。预设会保存用户默认值；项目配置仍可在此工作区收紧运行姿态。",
            fields.join(", ")
        ),
        _ => format!(
            "Project config contains {}. Presets save user defaults; project config can still tighten runtime posture in this workspace.",
            fields.join(", ")
        ),
    })
}

fn setup_report_result(state: &SetupState, facts: &SetupRuntimeFacts) -> String {
    format!(
        "first_run={}, update={}, operate={}, constitution={:?}, autonomy={}, posture={:?}, runtime={}, operate_fleet={}",
        if state.first_run_ready() {
            "ready"
        } else {
            "needs_action"
        },
        if state.update_ready(CONSTITUTION_CHECKPOINT_VERSION) {
            "ready"
        } else {
            "needs_action"
        },
        if state.operate_ready() {
            "ready"
        } else {
            "needs_action"
        },
        state.constitution_choice,
        facts.constitution_autonomy,
        state.runtime_posture_source,
        facts.runtime_result,
        facts.operate_result
    )
}

fn remote_runtime_on_ramp_text(locale: Locale, facts: &SetupRuntimeFacts) -> String {
    remote::on_ramp_text(
        locale,
        &facts.remote_clouds_result,
        &facts.remote_bridges_result,
        &facts.remote_providers_result,
        &facts.remote_mode_result,
        &facts.remote_command_provider,
    )
}

fn tools_mcp_on_ramp_text(locale: Locale, facts: &SetupRuntimeFacts) -> String {
    let tools_facts = tools_mcp::SetupToolsMcpFacts {
        servers_result: facts.tools_mcp_servers_result.clone(),
        skills_result: facts.tools_mcp_skills_result.clone(),
        tools_result: facts.tools_mcp_tools_result.clone(),
        plugins_result: facts.tools_mcp_plugins_result.clone(),
        hotbar_result: facts.tools_mcp_hotbar_result.clone(),
        result: facts.tools_mcp_result.clone(),
        overall_status: if facts.tools_mcp_needs_action {
            tools_mcp::InventoryStatus::NeedsConfig
        } else if facts.tools_mcp_result.contains("overall=off") {
            tools_mcp::InventoryStatus::Off
        } else {
            tools_mcp::InventoryStatus::Healthy
        },
        needs_action: facts.tools_mcp_needs_action,
        mcp_path_display: facts.tools_mcp_path_display.clone(),
        skills_path_display: facts.tools_mcp_skills_path_display.clone(),
        plugins_path_display: facts.tools_mcp_plugins_path_display.clone(),
    };
    tools_mcp::on_ramp_text(locale, &tools_facts)
}

#[cfg(test)]
#[must_use]
fn guided_constitution_template(locale: Locale) -> UserConstitution {
    GuidedConstitutionDraft::default().to_constitution(locale)
}

/// Who authored the draft being previewed for ratification.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DraftProvenance {
    /// Rendered deterministically from the guided answers.
    Guided,
    /// Drafted by the named model, then sanitized and bounded by CodeWhale.
    Model(String),
    /// The user's existing `constitution.json`, shown unchanged for the
    /// keep-existing checkpoint completion (#3794).
    Existing,
}

fn ratification_preview_title(locale: Locale) -> &'static str {
    match locale {
        Locale::Ja => "ユーザー憲法 - 批准前の草案",
        Locale::ZhHans => "用户宪法 — 批准前草案",
        Locale::ZhHant => "使用者憲法 - 批准前草案",
        Locale::PtBr => "Constituição do Usuário - Rascunho para Ratificação",
        Locale::Es419 => "Constitución del Usuario - Borrador para Ratificación",
        Locale::Vi => "Hiến pháp Người dùng - Bản nháp để phê chuẩn",
        _ => "User Constitution — Draft for Ratification",
    }
}

/// The ratification artifact shown in the pager: provenance, what a
/// constitution is, the exact block that will be injected (byte-identical to
/// prompt assembly's rendering), its authority boundaries, and how to ratify
/// or amend. Only the scaffold differs between guided and model drafts — the
/// law itself always comes from the same renderer.
fn constitution_ratification_text(
    locale: Locale,
    constitution: &UserConstitution,
    provenance: &DraftProvenance,
) -> String {
    const RULE: &str = "──────────────────────────────────────────────────────";
    let rendered = constitution
        .render_block(None)
        .unwrap_or_else(|| match locale {
            Locale::Ja => "構造化された憲法は空です。".to_string(),
            Locale::ZhHans => "结构化宪法为空。".to_string(),
            Locale::ZhHant => "結構化憲法為空。".to_string(),
            Locale::PtBr => "A constituição estruturada está vazia.".to_string(),
            Locale::Es419 => "La constitución estructurada está vacía.".to_string(),
            Locale::Vi => "Hiến pháp có cấu trúc đang trống.".to_string(),
            _ => "The structured constitution is empty.".to_string(),
        });
    let layer_order = tr(locale, MessageId::SetupCheckpointLayerOrder);

    match locale {
        Locale::Ja => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "{label} があなたのガイド回答から起草し、CodeWhale が構造検証と境界制限を適用しました。"
                ),
                DraftProvenance::Guided => {
                    "あなたのガイド回答から決定的に生成されました。".to_string()
                }
                DraftProvenance::Existing => {
                    "既存の憲法を constitution.json から読み込み、変更せずに表示しています。"
                        .to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "これはすでに有効な基準です。プレビューを閉じて K を押すと、このまま保持してチェックポイントを完了します。\
                     ファイルは変更されません。/constitution または /setup でいつでも修正できます。"
                }
                _ => {
                    "確認するまで、どの内容も基準にはなりません。プレビューを閉じて G を押すと批准して保存します。\
                     /constitution または /setup でいつでも修正できます。"
                }
            };
            format!(
                "CODEWHALE · ユーザー憲法\n{RULE}\n\n{drafted_by}\n\n\
                 これは CodeWhale があなたと協働するための常設の基準です。優れた憲法のように、使えるほど短く、\
                 網羅的な規則ではなく持続する原則で構成され、あなたの変化に合わせて修正できます。\
                 すべての個別判断を裁くのではなく権限と境界を定め、セッションを越えて協働を継続させます。\
                 ただしこれは記憶ではありません。履歴ではなく原則を保持します。\n\n\
                 {rendered}\n\n\
                 権限の階層\n{layer_order}\nあなたの直接の指示は常にこの文書より優先されます。\n\n\
                 これができないこと\n\
                 これは行動を導くものです。承認ポリシー、サンドボックス、Shell、ネットワーク、信頼、MCP 権限、\
                 既定モード、公開、支出の権限を付与または変更することはできません。これらは実行時にあなたが管理します。\n\n\
                 縮小コアと任意モジュール\n\
                 組み込みの 55 行コアは引き続き有効です。この草案はユーザーグローバルの長期設定だけを保存します。\
                 重い実行/オーケストレーション教義はモードプロンプトまたは将来の任意モジュールに属します。このプレビューはモジュールを有効化せず、設定も変更しません。\n\n\
                 批准\n{ratify_how}"
            )
        }
        Locale::ZhHans => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "由 {label} 根据你的引导式答案起草，并已由 CodeWhale 完成结构校验与边界限制。"
                ),
                DraftProvenance::Guided => "由你的引导式答案确定性生成。".to_string(),
                DraftProvenance::Existing => {
                    "你现有的宪法，读取自 constitution.json——原样展示，未做任何修改。".to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "这已是你现行的准则。关闭此预览后按 K 保留并完成检查点——文件不会被修改。\
                     之后可随时用 /constitution 或 /setup 修订。"
                }
                _ => {
                    "未经你确认，任何内容都不会成为准则。关闭此预览后按 G 批准并保存；\
                     之后可随时用 /constitution 或 /setup 修订。"
                }
            };
            format!(
                "CODEWHALE · 用户宪法\n{RULE}\n\n{drafted_by}\n\n\
                 这是 CodeWhale 与你协作的长期准则。像优秀的宪法一样：足够简短因而可用，由持久原则而非详尽规则构成，并且可以随你修订。\
                 它界定权力与边界，而非裁决每个具体决定；它让协作跨会话延续——但它不是记忆，它承载的是原则，而非历史。\n\n\
                 {rendered}\n\n\
                 权限层级\n{layer_order}\n你的直接指令始终高于本文件。\n\n\
                 它不能做什么\n\
                 它只提供行为指导，不能授予或更改审批策略、沙箱、Shell、网络、信任、MCP 权限、默认模式、发布或支出权限——这些始终由你在运行时掌控。\n\n\
                 精简核心与可选模块\n\
                 内置 55 行核心始终生效。本草案只保存你的用户全局长期偏好。执行/编排等重型教义位于模式提示词或未来的可选模块中；此预览不会启用模块或更改其配置。\n\n\
                 批准\n{ratify_how}"
            )
        }
        Locale::ZhHant => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "由 {label} 根據你的引導式答案起草，並已由 CodeWhale 完成結構驗證與邊界限制。"
                ),
                DraftProvenance::Guided => "由你的引導式答案確定性生成。".to_string(),
                DraftProvenance::Existing => {
                    "你現有的憲法，讀取自 constitution.json；原樣展示，未做任何修改。".to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "這已是你現行的準則。關閉此預覽後按 K 保留並完成檢查點；\
                     檔案不會被修改。之後可隨時用 /constitution 或 /setup 修訂。"
                }
                _ => {
                    "未經你確認，任何內容都不會成為準則。關閉此預覽後按 G 批准並保存；\
                     之後可隨時用 /constitution 或 /setup 修訂。"
                }
            };
            format!(
                "CODEWHALE · 使用者憲法\n{RULE}\n\n{drafted_by}\n\n\
                 這是 CodeWhale 與你協作的長期準則。像優秀的憲法一樣：足夠簡短因而可用，由持久原則而非詳盡規則構成，並且可以隨你修訂。\
                 它界定權力與邊界，而非裁決每個具體決定；它讓協作跨會話延續，但它不是記憶，它承載的是原則，而非歷史。\n\n\
                 {rendered}\n\n\
                 權限層級\n{layer_order}\n你的直接指令始終高於本文件。\n\n\
                 它不能做什麼\n\
                 它只提供行為指導，不能授予或更改審批策略、沙箱、Shell、網路、信任、MCP 權限、預設模式、發布或支出權限；這些始終由你在執行時掌控。\n\n\
                 精簡核心與可選模組\n\
                 內建 55 行核心始終生效。本草案只保存你的使用者全域長期偏好。執行/編排等重型教義位於模式提示詞或未來的可選模組中；此預覽不會啟用模組或更改其配置。\n\n\
                 批准\n{ratify_how}"
            )
        }
        Locale::PtBr => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "Rascunhado por {label} a partir das suas respostas guiadas, depois validado por schema e limitado pelo CodeWhale."
                ),
                DraftProvenance::Guided => {
                    "Renderizado deterministicamente a partir das suas respostas guiadas.".to_string()
                }
                DraftProvenance::Existing => {
                    "Sua constituição existente, carregada de constitution.json, é exibida sem alterações."
                        .to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "Esta já é sua regra vigente. Feche a prévia e pressione K para mantê-la e concluir o checkpoint; \
                     o arquivo não será modificado. Edite quando quiser com /constitution ou /setup."
                }
                _ => {
                    "Nada vira regra até você confirmar. Feche a prévia e pressione G para ratificar e salvar. \
                     Edite quando quiser com /constitution ou /setup."
                }
            };
            format!(
                "CODEWHALE · CONSTITUIÇÃO DO USUÁRIO\n{RULE}\n\n{drafted_by}\n\n\
                 Esta é a regra permanente de como o CodeWhale trabalha com você. Como boas constituições, \
                 ela é curta o bastante para ser usada, formada por princípios duráveis em vez de regras exaustivas, \
                 e pode ser emendada conforme você muda. Ela define poderes e limites em vez de decidir cada caso, \
                 e dá continuidade à colaboração entre sessões. Mas ela não é memória: carrega princípios, não histórico.\n\n\
                 {rendered}\n\n\
                 HIERARQUIA DE AUTORIDADE\n{layer_order}\nSeus pedidos diretos sempre superam este documento.\n\n\
                 O QUE ISTO NÃO PODE FAZER\n\
                 Isto orienta comportamento. Não pode conceder nem alterar política de aprovação, sandbox, shell, rede, \
                 confiança, permissões MCP, modo padrão, publicação ou autoridade para gastos; isso continua sob seu controle em tempo de execução.\n\n\
                 NÚCLEO REDUZIDO E MÓDULOS OPT-IN\n\
                 O núcleo embutido de 55 linhas continua ativo. Este rascunho só salva suas preferências permanentes globais de usuário. \
                 Doutrina pesada de execução ou orquestração pertence a prompts de modo ou módulos opt-in futuros; esta prévia não ativa módulos nem muda sua configuração.\n\n\
                 RATIFICAÇÃO\n{ratify_how}"
            )
        }
        Locale::Es419 => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "Redactado por {label} desde tus respuestas guiadas, luego validado por schema y acotado por CodeWhale."
                ),
                DraftProvenance::Guided => {
                    "Renderizado de forma determinística desde tus respuestas guiadas.".to_string()
                }
                DraftProvenance::Existing => {
                    "Tu constitución existente, cargada desde constitution.json, se muestra sin cambios."
                        .to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "Esta ya es tu regla vigente. Cierra la vista previa y presiona K para conservarla y completar el checkpoint; \
                     el archivo no se modifica. Puedes enmendarla cuando quieras con /constitution o /setup."
                }
                _ => {
                    "Nada se vuelve regla hasta que confirmes. Cierra la vista previa y presiona G para ratificar y guardar. \
                     Puedes enmendarla cuando quieras con /constitution o /setup."
                }
            };
            format!(
                "CODEWHALE · CONSTITUCIÓN DEL USUARIO\n{RULE}\n\n{drafted_by}\n\n\
                 Esta es la regla permanente de cómo CodeWhale trabaja contigo. Como las buenas constituciones, \
                 es lo bastante breve para usarse, hecha de principios duraderos en vez de reglas exhaustivas, \
                 y enmendable a medida que cambias. Define poderes y límites en vez de decidir cada caso, \
                 y da continuidad a la colaboración entre sesiones. Pero no es memoria: lleva principios, no historial.\n\n\
                 {rendered}\n\n\
                 JERARQUÍA DE AUTORIDAD\n{layer_order}\nTus pedidos directos siempre superan este documento.\n\n\
                 LO QUE ESTO NO PUEDE HACER\n\
                 Orienta comportamiento. No puede conceder ni cambiar política de aprobación, sandbox, shell, red, \
                 confianza, permisos MCP, modo predeterminado, publicación o autoridad de gasto; eso sigue bajo tu control en tiempo de ejecución.\n\n\
                 NÚCLEO REDUCIDO Y MÓDULOS OPT-IN\n\
                 El núcleo integrado de 55 líneas sigue activo. Este borrador solo guarda tus preferencias permanentes globales de usuario. \
                 La doctrina pesada de ejecución u orquestación pertenece a prompts de modo o módulos opt-in futuros; esta vista previa no activa módulos ni cambia su configuración.\n\n\
                 RATIFICACIÓN\n{ratify_how}"
            )
        }
        Locale::Vi => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "Được {label} soạn từ câu trả lời hướng dẫn của bạn, rồi được CodeWhale kiểm tra schema và giới hạn biên."
                ),
                DraftProvenance::Guided => {
                    "Được kết xuất xác định từ câu trả lời hướng dẫn của bạn.".to_string()
                }
                DraftProvenance::Existing => {
                    "Hiến pháp hiện có của bạn, tải từ constitution.json, được hiển thị nguyên trạng."
                        .to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "Đây đã là luật hiện hành của bạn. Đóng bản xem trước rồi nhấn K để giữ nguyên và hoàn tất checkpoint; \
                     tệp không bị sửa. Có thể chỉnh bất cứ lúc nào bằng /constitution hoặc /setup."
                }
                _ => {
                    "Không có gì trở thành luật cho đến khi bạn xác nhận. Đóng bản xem trước rồi nhấn G để phê chuẩn và lưu. \
                     Có thể chỉnh bất cứ lúc nào bằng /constitution hoặc /setup."
                }
            };
            format!(
                "CODEWHALE · HIẾN PHÁP NGƯỜI DÙNG\n{RULE}\n\n{drafted_by}\n\n\
                 Đây là luật thường trực cho cách CodeWhale làm việc với bạn. Giống các hiến pháp tốt, \
                 nó đủ ngắn để dùng, gồm các nguyên tắc bền vững thay vì luật lệ cạn kiệt, \
                 và có thể sửa khi bạn thay đổi. Nó định khung quyền hạn và giới hạn thay vì quyết định từng trường hợp, \
                 đồng thời giữ sự liên tục giữa các phiên. Nhưng nó không phải bộ nhớ: nó mang nguyên tắc, không mang lịch sử.\n\n\
                 {rendered}\n\n\
                 THỨ BẬC THẨM QUYỀN\n{layer_order}\nYêu cầu trực tiếp của bạn luôn cao hơn tài liệu này.\n\n\
                 ĐIỀU NÀY KHÔNG THỂ LÀM\n\
                 Nó hướng dẫn hành vi. Nó không thể cấp hoặc đổi chính sách phê duyệt, sandbox, shell, mạng, \
                 độ tin cậy, quyền MCP, chế độ mặc định, xuất bản hoặc quyền chi tiêu; những thứ đó vẫn do bạn kiểm soát lúc chạy.\n\n\
                 LÕI RÚT GỌN VÀ MÔ-ĐUN OPT-IN\n\
                 Lõi tích hợp 55 dòng vẫn hoạt động. Bản nháp này chỉ lưu tùy chọn thường trực toàn cục của người dùng. \
                 Giáo điều thực thi hoặc điều phối nặng thuộc về prompt chế độ hoặc mô-đun opt-in trong tương lai; bản xem trước này không bật mô-đun hoặc đổi cấu hình của chúng.\n\n\
                 PHÊ CHUẨN\n{ratify_how}"
            )
        }
        _ => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "Drafted by {label} from your guided answers, then schema-checked and bounded by CodeWhale."
                ),
                DraftProvenance::Guided => {
                    "Rendered deterministically from your guided answers.".to_string()
                }
                DraftProvenance::Existing => {
                    "Your existing constitution, loaded from constitution.json — shown unchanged."
                        .to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "This is already your standing law. Close this preview, then press K to \
                     keep it and complete the checkpoint — the file is not modified. Amend \
                     anytime with /constitution or /setup."
                }
                _ => {
                    "Nothing becomes law until you confirm. Close this preview, then press G to \
                     ratify and save. Amend anytime with /constitution or /setup."
                }
            };
            format!(
                "CODEWHALE · USER CONSTITUTION\n{RULE}\n\n{drafted_by}\n\n\
                 This is the standing law for how CodeWhale works with you. Like the best \
                 constitutions, it is short enough to use, made of durable principles rather \
                 than exhaustive rules, and amendable as you change. It frames powers and \
                 limits rather than deciding every case, and it gives your collaboration \
                 continuity across sessions — but it is not memory: it carries principles, \
                 not history.\n\n\
                 {rendered}\n\n\
                 HIERARCHY OF AUTHORITY\n{layer_order}\nYour direct requests always outrank this document.\n\n\
                 WHAT THIS CANNOT DO\n\
                 It guides behavior. It cannot grant or change approval policy, sandbox, shell, \
                 network, trust, MCP permissions, default mode, publishing, or spending \
                 authority — those stay under your hand at runtime.\n\n\
                 REDUCED CORE AND OPT-IN MODULES\n\
                 The bundled 55-line core stays active. This draft only saves your user-global \
                 standing preferences. Heavy execution or orchestration doctrine belongs in mode \
                 prompts or future opt-in modules; this preview does not enable modules or change \
                 their configuration.\n\n\
                 RATIFICATION\n{ratify_how}"
            )
        }
    }
}

/// Card line inviting the user to let their configured model draft the law.
fn model_draft_invitation_line(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::Ja => {
            format!("A {model_label} が起草し、あなたが批准します。確認するまで保存しません。")
        }
        Locale::ZhHans => {
            format!("A {model_label} 起草，你批准。未经确认不会保存。")
        }
        Locale::ZhHant => {
            format!("A {model_label} 起草，你批准。未經確認不會保存。")
        }
        Locale::PtBr => {
            format!("A {model_label} pode rascunhar. Você ratifica. Nada salva sem você.")
        }
        Locale::Es419 => {
            format!("A {model_label} puede redactarla. Tú ratificas. Nada se guarda sin ti.")
        }
        Locale::Vi => {
            format!("A {model_label} có thể soạn. Bạn phê chuẩn. Không lưu gì nếu chưa có bạn.")
        }
        _ => format!("A {model_label} can draft it. You ratify it. Nothing saves without you."),
    }
}

/// Card line offering to keep an existing valid constitution unchanged.
fn keep_existing_invitation_line(locale: Locale) -> &'static str {
    match locale {
        Locale::Ja => "K 既存の憲法を保持 - 確認して保持、ファイルは変更しません。",
        Locale::ZhHans => "K 保留现有宪法——先查看，再保留，文件不变。",
        Locale::ZhHant => "K 保留現有憲法 - 先查看，再保留，檔案不變。",
        Locale::PtBr => "K Manter constituição existente - revise, mantenha, arquivo inalterado.",
        Locale::Es419 => {
            "K Conservar constitución existente - revisa, conserva, archivo sin cambios."
        }
        Locale::Vi => "K Giữ hiến pháp hiện có - xem lại, giữ nguyên, tệp không đổi.",
        _ => "K Keep your existing constitution — review it, keep it, file unchanged.",
    }
}

/// Card line shown while a model draft awaits ratification.
fn model_draft_ready_line(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::Ja => {
            format!(
                "{model_label} の草案が批准待ちです - G で確認して批准、1-6 で草案を破棄します。"
            )
        }
        Locale::ZhHans => {
            format!("{model_label} 的草案待批准——按 G 查看并批准；按 1-6 会丢弃草案。")
        }
        Locale::ZhHant => {
            format!("{model_label} 的草案待批准 - 按 G 查看並批准；按 1-6 會丟棄草案。")
        }
        Locale::PtBr => {
            format!(
                "Rascunho de {model_label} aguarda ratificação - G para revisar e ratificar; 1-6 descarta."
            )
        }
        Locale::Es419 => {
            format!(
                "El borrador de {model_label} espera ratificación - G para revisar y ratificar; 1-6 lo descarta."
            )
        }
        Locale::Vi => {
            format!(
                "Bản nháp của {model_label} chờ phê chuẩn - G để xem và phê chuẩn; 1-6 sẽ bỏ bản nháp."
            )
        }
        _ => format!(
            "Draft by {model_label} awaits ratification — G to review and ratify; 1-6 discards it."
        ),
    }
}

/// Host-facing status line after a successful model draft.
pub(crate) fn model_draft_ready_message(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::Ja => format!(
            "{model_label} があなたの憲法を起草しました。プレビューを確認してから G で批准してください。"
        ),
        Locale::ZhHans => format!("{model_label} 已起草你的宪法。请查看预览，然后按 G 批准。"),
        Locale::ZhHant => format!("{model_label} 已起草你的憲法。請查看預覽，然後按 G 批准。"),
        Locale::PtBr => format!(
            "{model_label} rascunhou sua constituição. Revise a prévia e pressione G para ratificar."
        ),
        Locale::Es419 => format!(
            "{model_label} redactó tu constitución. Revisa la vista previa y presiona G para ratificar."
        ),
        Locale::Vi => format!(
            "{model_label} đã soạn hiến pháp của bạn. Xem bản xem trước rồi nhấn G để phê chuẩn."
        ),
        _ => format!(
            "{model_label} drafted your constitution. Review the preview, then press G to ratify."
        ),
    }
}

/// Host-facing status line when model drafting fails or is unavailable. The
/// guided deterministic draft always remains the standing fallback.
pub(crate) fn model_draft_failed_message(
    locale: Locale,
    model_label: &str,
    reason: &str,
) -> String {
    match locale {
        Locale::Ja => {
            format!(
                "{model_label} は起草を完了できませんでした（{reason}）。ガイド草案は有効です。G でプレビューして批准できます。"
            )
        }
        Locale::ZhHans => {
            format!("{model_label} 未能完成起草（{reason}）。引导式草案仍然有效——按 G 预览并批准。")
        }
        Locale::ZhHant => {
            format!("{model_label} 未能完成起草（{reason}）。引導式草案仍然有效；按 G 預覽並批准。")
        }
        Locale::PtBr => {
            format!(
                "{model_label} não conseguiu rascunhar sua constituição ({reason}). O rascunho guiado continua válido; pressione G para pré-visualizar e ratificar."
            )
        }
        Locale::Es419 => {
            format!(
                "{model_label} no pudo redactar tu constitución ({reason}). El borrador guiado sigue válido; presiona G para previsualizar y ratificar."
            )
        }
        Locale::Vi => {
            format!(
                "{model_label} không thể soạn hiến pháp của bạn ({reason}). Bản nháp hướng dẫn vẫn hợp lệ; nhấn G để xem trước và phê chuẩn."
            )
        }
        _ => format!(
            "{model_label} could not draft your constitution ({reason}). Your guided draft still \
             stands — press G to preview and ratify."
        ),
    }
}

fn constitution_choice_label(choice: ConstitutionChoice) -> &'static str {
    match choice {
        ConstitutionChoice::Unset => "unset",
        ConstitutionChoice::Bundled => "bundled/default",
        ConstitutionChoice::GuidedCustom => "guided custom",
        ConstitutionChoice::ExpertOverride => "expert override",
        ConstitutionChoice::Deferred => "deferred",
    }
}

fn constitution_source_label(source: ConstitutionSource) -> &'static str {
    match source {
        ConstitutionSource::Bundled => "bundled",
        ConstitutionSource::UserGlobal => "user-global constitution.json",
        ConstitutionSource::ExpertOverride => "expert full Markdown override",
    }
}

fn constitution_validity_label(validity: ConstitutionValidity) -> &'static str {
    match validity {
        ConstitutionValidity::Unknown => "unknown",
        ConstitutionValidity::Valid => "valid",
        ConstitutionValidity::Invalid => "invalid",
        ConstitutionValidity::Empty => "empty",
        ConstitutionValidity::Unreadable => "unreadable",
    }
}

pub fn persist_user_constitution_choice(
    constitution: &UserConstitution,
    state: &SetupState,
) -> anyhow::Result<()> {
    let constitution_path = UserConstitution::path()?;
    let setup_state_path = SetupState::path()?;
    let mut transaction = codewhale_config::persistence::SetupTransaction::new();
    transaction.stage_json(constitution_path, &constitution.bounded())?;
    transaction.stage_json(setup_state_path, state)?;
    transaction.commit()
}

#[must_use]
pub fn should_open_update_checkpoint(app: &App, config: &Config) -> bool {
    let state = load_setup_state_for_app(app, config);
    state.needs_constitution_checkpoint(CONSTITUTION_CHECKPOINT_VERSION)
}

pub fn defer_update_checkpoint_for_app(app: &App, config: &Config) -> anyhow::Result<SetupState> {
    let mut state = load_setup_state_for_app(app, config);
    if !state.needs_constitution_checkpoint(CONSTITUTION_CHECKPOINT_VERSION) {
        return Ok(state);
    }
    state.complete_constitution_checkpoint(
        CONSTITUTION_CHECKPOINT_VERSION,
        ConstitutionChoice::Deferred,
    );
    state.constitution_source = ConstitutionSource::Bundled;
    state.constitution_validity = ConstitutionValidity::Unknown;
    state.constitution_authoring = None;
    state.constitution_preview_hash = None;
    state.set_step(
        SetupStep::Constitution,
        StepEntry::new(StepStatus::Deferred, true, CONSTITUTION_CHECKPOINT_VERSION)
            .with_result("checkpoint deferred; bundled applies"),
    );
    state.save()?;
    Ok(state)
}

#[must_use]
pub fn load_setup_state_for_app(app: &App, config: &Config) -> SetupState {
    if let Ok(Some(state)) = SetupState::load() {
        return state;
    }
    SetupState::derive_inherited(&inherited_facts_for_app(app, config))
}

pub(crate) fn record_provider_model_setup_state_for_app(
    app: &App,
    config: &Config,
) -> anyhow::Result<SetupState> {
    let facts = SetupRuntimeFacts::from_app_config(app, config);
    let mut state = load_setup_state_for_app(app, config);
    state.set_step(
        SetupStep::ProviderModel,
        provider::step_entry(
            facts.provider_ready,
            CONSTITUTION_CHECKPOINT_VERSION,
            facts.provider_result,
        ),
    );
    state.save()?;
    Ok(state)
}

#[must_use]
fn inherited_facts_for_app(app: &App, config: &Config) -> InheritedConfigFacts {
    let user_constitution = UserConstitution::load().ok();
    let user_constitution_validity = user_constitution.as_ref().map_or(
        ConstitutionValidity::Unknown,
        UserConstitutionLoad::validity,
    );
    let has_user_constitution = user_constitution
        .as_ref()
        .is_some_and(|loaded| !matches!(loaded, UserConstitutionLoad::Missing));
    let expert_override = SetupExpertOverrideState::load();
    InheritedConfigFacts {
        language: Some(app.ui_locale.tag().to_string()),
        has_provider_route: !config.default_model().trim().is_empty(),
        has_credentials_or_local_runtime: has_api_key(config),
        trust_chosen: app.trust_mode || !onboarding::needs_trust(&app.workspace),
        has_expert_override: expert_override.is_active(),
        has_user_constitution,
        user_constitution_validity,
    }
}

fn expert_override_path() -> Option<std::path::PathBuf> {
    codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join(Path::new(CONSTITUTION_OVERRIDE_FILE)))
}

#[must_use]
fn initial_step_index(state: &SetupState) -> usize {
    if state.needs_constitution_checkpoint(CONSTITUTION_CHECKPOINT_VERSION) {
        return step_index(SetupStep::Constitution);
    }
    STEP_SPECS
        .iter()
        .position(|step| {
            step.required()
                && !matches!(
                    state.status(step.id()),
                    StepStatus::Verified
                        | StepStatus::NeedsAction
                        | StepStatus::Deferred
                        | StepStatus::Optional
                        | StepStatus::Skipped
                )
        })
        .unwrap_or_else(|| step_index(SetupStep::Verification))
}

#[must_use]
fn step_index(step: SetupStep) -> usize {
    STEP_SPECS
        .iter()
        .position(|spec| spec.id() == step)
        .expect("all setup-state steps should have wizard specs")
}

fn visible_step_index(step: SetupStep) -> usize {
    STEP_SPECS
        .iter()
        .position(|spec| spec.id() == step)
        .unwrap_or_else(|| step_index(SetupStep::Constitution))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn setup_test_options(workspace: std::path::PathBuf) -> crate::tui::app::TuiOptions {
        crate::tui::app::TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
            config_path: None,
            config_profile: None,
            allow_shell: true,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: std::path::PathBuf::from("."),
            memory_path: std::path::PathBuf::from("memory.md"),
            notes_path: std::path::PathBuf::from("notes.txt"),
            mcp_config_path: std::path::PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: false,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        }
    }

    #[test]
    fn visible_release_rail_includes_supported_optional_steps() {
        let steps = STEP_SPECS.iter().map(|step| step.id()).collect::<Vec<_>>();

        assert_eq!(
            steps,
            vec![
                SetupStep::Language,
                SetupStep::ProviderModel,
                SetupStep::TrustSandbox,
                SetupStep::Constitution,
                SetupStep::OperateFleet,
                SetupStep::Hotbar,
                SetupStep::ToolsMcp,
                SetupStep::RemoteRuntime,
                SetupStep::Persistence,
                SetupStep::Verification,
            ]
        );
        assert_eq!(
            SetupWizardView::new_at_with_facts(
                SetupState::default(),
                Locale::En,
                SetupStep::ToolsMcp,
                SetupRuntimeFacts::default(),
            )
            .selected_step(),
            SetupStep::ToolsMcp
        );
    }

    #[test]
    fn wizard_resumes_at_constitution_checkpoint_when_update_incomplete() {
        let state = SetupState::default();

        let view = SetupWizardView::new(state, Locale::En);

        assert_eq!(view.selected_step(), SetupStep::Constitution);
    }

    #[test]
    fn bundled_constitution_commit_marks_checkpoint_complete() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::EmitAndClose(ViewEvent::SetupStateCommitRequested { state, message }) =
            action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(
            state.constitution_checkpoint_completed_for.as_deref(),
            Some(CONSTITUTION_CHECKPOINT_VERSION)
        );
        assert_eq!(state.constitution_choice, ConstitutionChoice::Bundled);
        assert_eq!(state.status(SetupStep::Constitution), StepStatus::Verified);
        assert!(message.contains("Constitution checkpoint complete"));
    }

    #[test]
    fn back_keys_return_to_previous_step_and_clamp_at_first() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);
        assert_eq!(view.selected_step(), SetupStep::Constitution);

        let action = view.handle_key(key(KeyCode::Right));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.selected_step(), SetupStep::OperateFleet);

        let action = view.handle_key(key(KeyCode::Char('b')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.selected_step(), SetupStep::Constitution);

        for _ in 0..STEP_SPECS.len() {
            view.handle_key(key(KeyCode::Left));
        }
        assert_eq!(view.selected_step(), SetupStep::Language);
    }

    #[test]
    fn cancel_closes_without_commit_event() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        let action = view.handle_key(key(KeyCode::Esc));

        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn skip_and_retry_emit_setup_state_commits() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        let action = view.handle_key(key(KeyCode::Char('s')));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected skipped setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::Constitution), StepStatus::Skipped);
        assert!(message.contains("skipped"));
        assert_eq!(view.selected_step(), SetupStep::OperateFleet);

        let action = view.handle_key(key(KeyCode::Char('r')));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected retry setup-state commit event");
        };
        assert_eq!(
            state.status(SetupStep::OperateFleet),
            StepStatus::NeedsAction
        );
        assert!(message.contains("retry"));
    }

    #[test]
    fn completed_checkpoint_resumes_to_first_required_gap() {
        let mut state = SetupState::default();
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::Bundled,
        );

        let view = SetupWizardView::new(state, Locale::En);

        assert_eq!(view.selected_step(), SetupStep::Language);
    }

    #[test]
    fn language_step_records_locale_and_unblocks_first_run_ready() {
        let mut state = SetupState::default();
        state.set_step(
            SetupStep::ProviderModel,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION),
        );
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::Bundled,
        );
        state.set_step(
            SetupStep::Constitution,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION),
        );
        let mut view = SetupWizardView::new(state, Locale::En);
        assert_eq!(view.selected_step(), SetupStep::Language);

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected language setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::Language), StepStatus::Verified);
        assert_eq!(state.constitution_language.as_deref(), Some("en"));
        assert!(state.first_run_ready());
        assert!(message.contains("Setup language recorded"));
        assert_eq!(view.selected_step(), SetupStep::ProviderModel);
    }

    #[test]
    fn zh_hans_checkpoint_copy_is_localized() {
        assert_ne!(
            tr(Locale::ZhHans, MessageId::SetupWizardTitle),
            tr(Locale::En, MessageId::SetupWizardTitle)
        );
        assert_ne!(
            tr(Locale::ZhHans, MessageId::SetupCheckpointDoneBundled),
            tr(Locale::En, MessageId::SetupCheckpointDoneBundled)
        );
    }

    #[test]
    fn guided_constitution_requires_preview_before_save() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        let action = view.handle_key(key(KeyCode::Char('g')));

        let ViewAction::Emit(ViewEvent::OpenTextPager { title, content }) = action else {
            panic!("expected guided constitution preview event");
        };
        assert!(title.contains("Draft for Ratification"));
        assert!(content.contains("<codewhale_user_constitution"));
        assert!(content.contains("press G to ratify and save"));
        assert!(content.contains("REDUCED CORE AND OPT-IN MODULES"));
        assert!(content.contains("The bundled 55-line core stays active"));
        assert!(content.contains("does not enable modules"));
        assert_eq!(view.state().constitution_choice, ConstitutionChoice::Unset);

        let action = view.handle_key(key(KeyCode::Char('g')));

        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            state,
            message,
        }) = action
        else {
            panic!("expected guided constitution commit event");
        };
        assert_eq!(constitution.language.as_deref(), Some("en"));
        assert_eq!(
            constitution.autonomy_preference,
            AutonomyPreference::Balanced
        );
        assert_eq!(state.constitution_choice, ConstitutionChoice::GuidedCustom);
        assert_eq!(state.constitution_source, ConstitutionSource::UserGlobal);
        assert_eq!(state.constitution_validity, ConstitutionValidity::Valid);
        assert_eq!(
            state.constitution_preview_hash.as_deref(),
            Some(constitution.preview_hash().as_str())
        );
        assert_eq!(state.status(SetupStep::Constitution), StepStatus::Verified);
        assert_eq!(state.runtime_posture_source, RuntimePostureSource::Unset);
        assert!(message.contains("Constitution ratified"));
    }

    #[test]
    fn ratification_preview_explains_reduced_core_modules_for_shipped_locales() {
        for locale in Locale::shipped() {
            let constitution = GuidedConstitutionDraft::default().to_constitution(*locale);
            let content =
                constitution_ratification_text(*locale, &constitution, &DraftProvenance::Guided);
            let (heading, module_marker, no_enable_marker, permission_marker, mcp_marker) =
                match locale {
                    Locale::Ja => (
                        "縮小コア",
                        "モジュール",
                        "有効化せず",
                        "承認ポリシー、サンドボックス、Shell、ネットワーク、信頼、MCP 権限",
                        "付与または変更することはできません",
                    ),
                    Locale::ZhHans => (
                        "精简核心",
                        "模块",
                        "不会启用",
                        "不能授予或更改审批策略、沙箱、Shell、网络、信任、MCP 权限",
                        "发布或支出权限",
                    ),
                    Locale::ZhHant => (
                        "精簡核心",
                        "模組",
                        "不會啟用",
                        "不能授予或更改審批策略、沙箱、Shell、網路、信任、MCP 權限",
                        "發布或支出權限",
                    ),
                    Locale::PtBr => (
                        "NÚCLEO REDUZIDO",
                        "módulos",
                        "não ativa",
                        "Não pode conceder nem alterar política de aprovação, sandbox, shell, rede",
                        "permissões MCP",
                    ),
                    Locale::Es419 => (
                        "NÚCLEO REDUCIDO",
                        "módulos",
                        "no activa",
                        "No puede conceder ni cambiar política de aprobación, sandbox, shell, red",
                        "permisos MCP",
                    ),
                    Locale::Vi => (
                        "LÕI RÚT GỌN",
                        "mô-đun",
                        "không bật",
                        "không thể cấp hoặc đổi chính sách phê duyệt, sandbox, shell, mạng",
                        "quyền MCP",
                    ),
                    Locale::En => (
                        "REDUCED CORE",
                        "modules",
                        "does not enable",
                        "cannot grant or change approval policy, sandbox, shell",
                        "MCP permissions",
                    ),
                };

            assert!(content.contains("55"), "{}", locale.tag());
            assert!(content.contains(heading), "{}", locale.tag());
            assert!(content.contains(module_marker), "{}", locale.tag());
            assert!(content.contains(no_enable_marker), "{}", locale.tag());
            assert!(content.contains(permission_marker), "{}", locale.tag());
            assert!(content.contains(mcp_marker), "{}", locale.tag());
        }
    }

    #[test]
    fn guided_constitution_key_is_contextual_to_constitution_step() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            SetupRuntimeFacts::default(),
        );

        let action = view.handle_key(key(KeyCode::Char('g')));

        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.selected_step(), SetupStep::ProviderModel);
        assert_eq!(view.state().constitution_choice, ConstitutionChoice::Unset);
    }

    #[test]
    fn provider_model_step_hands_off_to_existing_route_surfaces() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            SetupRuntimeFacts::default(),
        );

        let provider_action = view.handle_key(key(KeyCode::Char('p')));
        assert!(matches!(
            provider_action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenProviderRequested)
        ));

        let model_action = view.handle_key(key(KeyCode::Char('m')));
        assert!(matches!(
            model_action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenModelRequested)
        ));
    }

    #[test]
    fn provider_model_detail_lines_show_credential_url_for_missing_hosted_provider() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join(".codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _nim_key = crate::test_support::EnvVarGuard::remove("NVIDIA_API_KEY");
        let _nim_alt_key = crate::test_support::EnvVarGuard::remove("NVIDIA_NIM_API_KEY");
        let config = Config {
            provider: Some("nvidia-nim".to_string()),
            ..Config::default()
        };
        let app = App::new(setup_test_options(workspace), &config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            facts,
        );

        let text = lines_to_text(view.provider_model_detail_lines());

        assert!(text.contains("NVIDIA NIM"), "{text}");
        assert!(text.contains("credentials: https://build.nvidia.com/settings/api-keys"));
    }

    #[test]
    fn provider_model_detail_lines_keep_codex_oauth_url_free() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join(".codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _openai_codex_key =
            crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _codex_key = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let config = Config {
            provider: Some("openai-codex".to_string()),
            ..Config::default()
        };
        let app = App::new(setup_test_options(workspace), &config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            facts,
        );

        let text = lines_to_text(view.provider_model_detail_lines());

        assert!(text.contains("codex login"), "{text}");
        assert!(!text.contains("credentials:"), "{text}");
    }

    #[test]
    fn provider_model_detail_lines_cover_deepseek_cn_and_local_boundaries() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join(".codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _deepseek_key = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY");
        let _deepseek_source = crate::test_support::EnvVarGuard::remove("DEEPSEEK_API_KEY_SOURCE");

        let cn_config = Config {
            provider: Some("deepseek-cn".to_string()),
            ..Config::default()
        };
        let cn_app = App::new(setup_test_options(workspace.clone()), &cn_config);
        let cn_view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            SetupRuntimeFacts::from_app_config(&cn_app, &cn_config),
        );
        let cn_text = lines_to_text(cn_view.provider_model_detail_lines());
        assert!(cn_text.contains("DeepSeek (legacy alias)"), "{cn_text}");
        assert!(
            cn_text.contains("credentials: https://platform.deepseek.com/api_keys"),
            "{cn_text}"
        );
        assert!(cn_text.contains("missing for active provider"), "{cn_text}");

        let local_config = Config {
            provider: Some("ollama".to_string()),
            ..Config::default()
        };
        let local_app = App::new(setup_test_options(workspace), &local_config);
        let local_view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            SetupRuntimeFacts::from_app_config(&local_app, &local_config),
        );
        let local_text = lines_to_text(local_view.provider_model_detail_lines());
        assert!(local_text.contains("Ollama"), "{local_text}");
        assert!(
            local_text.contains("present or local runtime"),
            "{local_text}"
        );
        assert!(!local_text.contains("credentials:"), "{local_text}");
    }

    #[test]
    fn runtime_posture_step_hands_off_to_mode_and_config_surfaces() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            SetupRuntimeFacts::default(),
        );

        let mode_action = view.handle_key(key(KeyCode::Char('m')));
        assert!(matches!(
            mode_action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenModeRequested)
        ));

        let config_action = view.handle_key(key(KeyCode::Char('c')));
        assert!(matches!(
            config_action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenConfigRequested)
        ));
    }

    #[test]
    fn operate_fleet_step_hands_off_to_provider_and_fleet_surfaces() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::OperateFleet,
            SetupRuntimeFacts::default(),
        );

        let provider_action = view.handle_key(key(KeyCode::Char('p')));
        assert!(matches!(
            provider_action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenProviderRequested)
        ));

        let fleet_action = view.handle_key(key(KeyCode::Char('f')));
        assert!(matches!(
            fleet_action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenFleetRequested)
        ));
    }

    #[test]
    fn hotbar_step_hands_off_to_existing_hotbar_setup() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            SetupRuntimeFacts::default(),
        );

        let action = view.handle_key(key(KeyCode::Char('h')));

        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::SetupOpenHotbarRequested)
        ));
    }

    #[test]
    fn remote_runtime_step_previews_generate_only_on_ramp() {
        let facts = SetupRuntimeFacts {
            remote_clouds_result: "3 cloud targets: lighthouse, azure, digitalocean".to_string(),
            remote_bridges_result: "2 chat bridges: feishu, telegram".to_string(),
            remote_providers_result:
                "12 providers from the provider registry; active route deepseek / deepseek-chat"
                    .to_string(),
            remote_mode_result:
                "generate-only bundle; --apply not implemented; default port 7878, workers 2"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::RemoteRuntime,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Char('r')));

        let ViewAction::Emit(ViewEvent::OpenTextPager { title, content }) = action else {
            panic!("expected remote on-ramp pager");
        };
        assert_eq!(title, "Remote runtime on-ramp");
        assert!(content.contains("does not generate deploy bundles"));
        assert!(content.contains("codewhale remote-setup --generate-only"));
        assert!(content.contains("`--apply` remains unimplemented"));
    }

    #[test]
    fn remote_runtime_on_ramp_command_uses_active_provider() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join(".codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let config = Config {
            provider: Some("openrouter".to_string()),
            ..Config::default()
        };
        let app = App::new(setup_test_options(workspace), &config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);

        let content = remote_runtime_on_ramp_text(Locale::En, &facts);

        assert!(content.contains("--provider openrouter"), "{content}");
        assert!(!content.contains("--provider deepseek"), "{content}");
        assert!(
            content.contains("does not generate deploy bundles"),
            "{content}"
        );
        assert!(
            content.contains("`--apply` remains unimplemented"),
            "{content}"
        );
    }

    #[test]
    fn remote_runtime_on_ramp_is_localized_for_shipped_locales() {
        let facts = SetupRuntimeFacts {
            remote_clouds_result: "3 cloud targets: lighthouse, azure, digitalocean".to_string(),
            remote_bridges_result: "2 chat bridges: feishu, telegram".to_string(),
            remote_providers_result:
                "12 providers from the provider registry; active route deepseek / deepseek-chat"
                    .to_string(),
            remote_mode_result:
                "generate-only bundle; --apply not implemented; default port 7878, workers 2"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let english = remote_runtime_on_ramp_text(Locale::En, &facts);

        for locale in Locale::shipped() {
            let content = remote_runtime_on_ramp_text(*locale, &facts);
            assert!(
                content.contains("codewhale remote-setup --generate-only"),
                "{}",
                locale.tag()
            );
            assert!(content.contains("`--apply`"), "{}", locale.tag());
            if *locale != Locale::En {
                assert_ne!(content, english, "{}", locale.tag());
            }
        }
    }

    #[test]
    fn guided_constitution_answers_shape_preview_and_saved_payload() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);
        for key_char in ['1', '2', '3', '4', '5', '6'] {
            assert!(matches!(
                view.handle_key(key(KeyCode::Char(key_char))),
                ViewAction::None
            ));
        }

        let action = view.handle_key(key(KeyCode::Char('g')));

        let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = action else {
            panic!("expected tuned guided constitution preview event");
        };
        assert!(content.contains("current, cited research"));
        assert!(content.contains("ambitious initiative"));
        assert!(content.contains("release evidence"));
        assert!(content.contains("learn the system"));
        assert!(content.contains("sensitive data"));
        assert!(content.contains("user voice"));
        assert!(content.contains("preserve the user's voice"));

        let action = view.handle_key(key(KeyCode::Char('g')));

        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            state,
            ..
        }) = action
        else {
            panic!("expected tuned guided constitution commit event");
        };
        assert_eq!(
            constitution.autonomy_preference,
            AutonomyPreference::Autonomous
        );
        let body = constitution.render_body();
        assert!(body.contains("current, cited research"));
        assert!(body.contains("release evidence"));
        assert!(body.contains("learn the system"));
        assert!(body.contains("sensitive data"));
        assert!(body.contains("preserve the user's voice"));
        assert_eq!(
            state.constitution_preview_hash.as_deref(),
            Some(constitution.preview_hash().as_str())
        );
    }

    #[test]
    fn constitution_detail_lines_explain_reduced_core_and_modules_boundary() {
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            SetupRuntimeFacts::default(),
        );

        let text = lines_to_text(view.constitution_detail_lines());

        assert!(text.contains("user-global preferences only"));
        assert!(text.contains("55-line core"));
        assert!(text.contains("mode prompts"));
        assert!(text.contains("future opt-ins"));
    }

    #[test]
    fn freeform_note_previews_saves_and_stays_advisory() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        let first_preview = view.handle_key(key(KeyCode::Char('g')));
        assert!(matches!(
            first_preview,
            ViewAction::Emit(ViewEvent::OpenTextPager { .. })
        ));
        assert!(view.handle_paste(
            "Prefer reversible demos; do not treat shell unrestricted as permission."
        ));

        let second_preview = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = second_preview else {
            panic!("freeform note should force a fresh preview");
        };
        assert!(content.contains("User freeform principle"));
        assert!(content.contains("Prefer reversible demos"));
        assert!(content.contains("do not change approval, sandbox, shell"));

        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            state,
            ..
        }) = action
        else {
            panic!("expected guided constitution commit event");
        };
        let body = constitution.render_body();
        assert!(body.contains("User freeform principle"));
        assert!(body.contains("Prefer reversible demos"));
        assert_eq!(
            state.constitution_authoring,
            Some(ConstitutionAuthoring::Guided)
        );
        assert_eq!(state.runtime_posture_source, RuntimePostureSource::Unset);
    }

    #[test]
    fn changing_guided_answer_requires_fresh_preview() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        let first_preview = view.handle_key(key(KeyCode::Char('g')));
        assert!(matches!(
            first_preview,
            ViewAction::Emit(ViewEvent::OpenTextPager { .. })
        ));

        assert!(matches!(
            view.handle_key(key(KeyCode::Char('6'))),
            ViewAction::None
        ));
        let second_preview = view.handle_key(key(KeyCode::Char('g')));

        let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = second_preview else {
            panic!("changed guided answer should preview again before saving");
        };
        assert!(content.contains("preserve the user's voice"));

        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            ..
        }) = action
        else {
            panic!("expected save after fresh preview");
        };
        assert_eq!(
            constitution.autonomy_preference,
            AutonomyPreference::Balanced
        );
        assert!(
            constitution
                .render_body()
                .contains("preserve the user's voice")
        );
    }

    fn ready_facts(model: &str) -> SetupRuntimeFacts {
        SetupRuntimeFacts {
            provider_ready: true,
            model: model.to_string(),
            ..SetupRuntimeFacts::default()
        }
    }

    fn first_run_ready_state() -> SetupState {
        let mut state = SetupState::default();
        state.set_step(
            SetupStep::Language,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION),
        );
        state.set_step(
            SetupStep::ProviderModel,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION),
        );
        state.runtime_posture_source = RuntimePostureSource::Confirmed;
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::Bundled,
        );
        state.set_step(
            SetupStep::Constitution,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION),
        );
        state
    }

    fn sample_model_draft() -> Box<UserConstitution> {
        Box::new(UserConstitution {
            language: Some("en".to_string()),
            about: Some("A GLM-5.2 user shipping Rust.".to_string()),
            working_style: vec!["Keep diffs scoped.".to_string()],
            priorities: vec!["Evidence over vibes.".to_string()],
            autonomy_preference: AutonomyPreference::Balanced,
            notes: Some("Advisory only.".to_string()),
            ..UserConstitution::default()
        })
    }

    #[test]
    fn model_draft_key_is_inert_without_a_ready_provider() {
        // Fallback contract: no route, no drafting offer — the deterministic
        // guided flow stands untouched.
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);
        assert_eq!(view.selected_step(), SetupStep::Constitution);

        let action = view.handle_key(key(KeyCode::Char('a')));

        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.state().constitution_choice, ConstitutionChoice::Unset);
    }

    #[test]
    fn model_draft_key_requests_drafting_with_current_answers() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            ready_facts("GLM-5.2"),
        );
        // Tune one answer first: the request must carry the tuned draft.
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('2'))),
            ViewAction::None
        ));
        assert!(view.handle_paste("Prefer demos before durable rewrites."));

        let action = view.handle_key(key(KeyCode::Char('a')));

        let ViewAction::Emit(ViewEvent::SetupConstitutionModelDraftRequested {
            draft,
            freeform_note,
            locale,
        }) = action
        else {
            panic!("expected model draft request event");
        };
        assert_eq!(locale, Locale::En);
        assert_eq!(draft.autonomy, AutonomyPreference::Autonomous);
        assert_eq!(
            freeform_note.as_deref(),
            Some("Prefer demos before durable rewrites.")
        );
        // The wizard stays open (Emit, not EmitAndClose) and nothing commits.
        assert_eq!(view.state().constitution_choice, ConstitutionChoice::Unset);
    }

    #[test]
    fn installed_model_draft_previews_then_ratifies_with_provenance() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            ready_facts("GLM-5.2"),
        );

        let (title, content) =
            view.install_model_draft(sample_model_draft(), "GLM-5.2".to_string());
        assert!(title.contains("Draft for Ratification"));
        assert!(content.contains("Drafted by GLM-5.2"));
        assert!(content.contains("A GLM-5.2 user shipping Rust."));
        assert!(content.contains("<codewhale_user_constitution"));

        // The install satisfied the preview gate; G ratifies the model draft.
        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            state,
            message,
        }) = action
        else {
            panic!("expected ratification commit event");
        };
        assert_eq!(constitution, *sample_model_draft());
        assert_eq!(state.constitution_choice, ConstitutionChoice::GuidedCustom);
        assert_eq!(
            state.constitution_authoring,
            Some(ConstitutionAuthoring::ModelDrafted)
        );
        assert_eq!(
            state.constitution_preview_hash.as_deref(),
            Some(constitution.preview_hash().as_str())
        );
        let step = state.steps.get(&SetupStep::Constitution).expect("step");
        let result = step.result.as_deref().expect("result");
        assert!(result.contains("model-drafted constitution ratified (GLM-5.2)"));
        assert!(message.contains("Constitution ratified"));
    }

    #[test]
    fn deterministic_ratification_records_guided_authoring() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);

        view.handle_key(key(KeyCode::Char('g')));
        let action = view.handle_key(key(KeyCode::Char('g')));

        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested { state, .. }) =
            action
        else {
            panic!("expected guided commit event");
        };
        assert_eq!(
            state.constitution_authoring,
            Some(ConstitutionAuthoring::Guided)
        );
    }

    #[test]
    fn cycling_answers_discards_the_model_draft() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            ready_facts("GLM-5.2"),
        );
        let _ = view.install_model_draft(sample_model_draft(), "GLM-5.2".to_string());

        // Changing any answer makes the model draft stale law.
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('1'))),
            ViewAction::None
        ));

        // The next G must preview afresh — and preview the guided rendering,
        // not the discarded model draft.
        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = action else {
            panic!("stale draft should force a fresh preview");
        };
        assert!(content.contains("Rendered deterministically"));
        assert!(!content.contains("Drafted by GLM-5.2"));

        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested { state, .. }) =
            action
        else {
            panic!("expected guided commit after discard");
        };
        assert_eq!(
            state.constitution_authoring,
            Some(ConstitutionAuthoring::Guided)
        );
    }

    #[test]
    fn freeform_note_discards_the_model_draft() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            ready_facts("GLM-5.2"),
        );
        let _ = view.install_model_draft(sample_model_draft(), "GLM-5.2".to_string());

        assert!(view.handle_paste("Prefer local examples before broad rewrites."));

        let action = view.handle_key(key(KeyCode::Char('g')));
        let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = action else {
            panic!("changed freeform note should force a fresh guided preview");
        };
        assert!(content.contains("Rendered deterministically"));
        assert!(content.contains("Prefer local examples"));
        assert!(!content.contains("Drafted by GLM-5.2"));
    }

    #[test]
    fn constitution_card_gates_the_model_draft_invitation() {
        // No ready provider: no invitation (and the blocker-size layout holds).
        let not_ready = SetupWizardView::new(SetupState::default(), Locale::En);
        let text = lines_to_text(not_ready.constitution_detail_lines());
        assert!(!text.contains("can draft it"));
        assert!(!text.contains("awaits ratification"));

        // Ready provider: the invitation names the first configured model.
        let ready = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            ready_facts("GLM-5.2"),
        );
        let text = lines_to_text(ready.constitution_detail_lines());
        assert!(text.contains("GLM-5.2 can draft it. You ratify it."));

        // Installed draft: the card flips to the awaiting-ratification line.
        let mut with_draft = ready.clone();
        let _ = with_draft.install_model_draft(sample_model_draft(), "GLM-5.2".to_string());
        let text = lines_to_text(with_draft.constitution_detail_lines());
        assert!(text.contains("Draft by GLM-5.2 awaits ratification"));
        assert!(!text.contains("GLM-5.2 can draft it"));
    }

    #[test]
    fn model_drafted_commit_round_trips_through_the_setup_transaction() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());

        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            ready_facts("GLM-5.2"),
        );
        let _ = view.install_model_draft(sample_model_draft(), "GLM-5.2".to_string());
        let ViewAction::EmitAndClose(ViewEvent::SetupConstitutionCommitRequested {
            constitution,
            state,
            ..
        }) = view.handle_key(key(KeyCode::Char('g')))
        else {
            panic!("expected ratification commit event");
        };

        persist_user_constitution_choice(&constitution, &state).expect("persist");

        let loaded = UserConstitution::load().expect("load constitution");
        let loaded = loaded.constitution().expect("valid constitution");
        assert_eq!(loaded.render_body(), constitution.render_body());
        let loaded_state = SetupState::load().expect("load state").expect("state");
        assert_eq!(
            loaded_state.constitution_authoring,
            Some(ConstitutionAuthoring::ModelDrafted)
        );
        assert_eq!(
            loaded_state.constitution_preview_hash.as_deref(),
            Some(constitution.preview_hash().as_str())
        );
    }

    #[test]
    fn guided_constitution_template_localizes_content() {
        let english = guided_constitution_template(Locale::En).render_body();
        let zh_hans = guided_constitution_template(Locale::ZhHans).render_body();

        assert!(english.contains("evidence-first coding workbench"));
        assert!(zh_hans.contains("重证据"));
        assert_ne!(english, zh_hans);

        let markers = [
            (Locale::Ja, "証拠重視"),
            (Locale::ZhHans, "重证据"),
            (Locale::ZhHant, "重證據"),
            (Locale::PtBr, "guiada por evidências"),
            (Locale::Es419, "basada en evidencia"),
            (Locale::Vi, "ưu tiên bằng chứng"),
        ];
        for (locale, marker) in markers {
            let body = guided_constitution_template(locale).render_body();
            assert!(
                body.contains(marker),
                "missing localized guided marker for {}",
                locale.tag()
            );
            assert_ne!(
                english,
                body,
                "locale {} fell back to English",
                locale.tag()
            );
            assert!(
                !body.contains("A CodeWhale user who wants"),
                "locale {} reused English purpose copy",
                locale.tag()
            );
            assert!(
                !body.contains("Guided answers:"),
                "locale {} reused English guided-answer notes",
                locale.tag()
            );
            assert!(
                !body.contains("Current user requests and live tool evidence"),
                "locale {} reused English authority priority",
                locale.tag()
            );
        }
    }

    #[test]
    fn ratification_preview_uses_rendered_block_and_layer_order() {
        let draft = GuidedConstitutionDraft::default();
        let english = constitution_ratification_text(
            Locale::En,
            &draft.to_constitution(Locale::En),
            &DraftProvenance::Guided,
        );
        let zh_hans = constitution_ratification_text(
            Locale::ZhHans,
            &draft.to_constitution(Locale::ZhHans),
            &DraftProvenance::Guided,
        );

        assert!(english.contains("<codewhale_user_constitution"));
        assert!(english.contains("Layer order"));
        assert!(english.contains("press G to ratify and save"));
        // Framing: powers and limits, not case-by-case; continuity, not memory.
        assert!(english.contains("powers and limits rather than deciding every case"));
        assert!(english.contains("but it is not memory"));
        assert!(zh_hans.contains("<codewhale_user_constitution"));
        assert!(zh_hans.contains("按 G 批准并保存"));
        assert!(zh_hans.contains("它界定权力与边界"));
        assert!(zh_hans.contains("但它不是记忆"));
        assert_ne!(english, zh_hans);

        let localized_markers = [
            (Locale::Ja, "権限の階層"),
            (Locale::ZhHans, "精简核心与可选模块"),
            (Locale::ZhHant, "精簡核心與可選模組"),
            (Locale::PtBr, "NÚCLEO REDUZIDO E MÓDULOS OPT-IN"),
            (Locale::Es419, "NÚCLEO REDUCIDO Y MÓDULOS OPT-IN"),
            (Locale::Vi, "LÕI RÚT GỌN VÀ MÔ-ĐUN OPT-IN"),
        ];
        for (locale, marker) in localized_markers {
            let content = constitution_ratification_text(
                locale,
                &draft.to_constitution(locale),
                &DraftProvenance::Guided,
            );
            assert!(
                content.contains(marker),
                "missing localized ratification marker for {}",
                locale.tag()
            );
            assert_ne!(
                english,
                content,
                "locale {} ratification preview fell back to English",
                locale.tag()
            );
            for fallback in [
                "CODEWHALE · USER CONSTITUTION",
                "HIERARCHY OF AUTHORITY",
                "WHAT THIS CANNOT DO",
                "REDUCED CORE AND OPT-IN MODULES",
                "Rendered deterministically from your guided answers",
                "Nothing becomes law until you confirm",
            ] {
                assert!(
                    !content.contains(fallback),
                    "locale {} reused English ratification scaffold: {fallback}",
                    locale.tag()
                );
            }
        }
    }

    #[test]
    fn ratification_preview_states_authority_boundaries_and_provenance() {
        let draft = GuidedConstitutionDraft::default();
        let constitution = draft.to_constitution(Locale::En);

        let guided =
            constitution_ratification_text(Locale::En, &constitution, &DraftProvenance::Guided);
        assert!(guided.contains("HIERARCHY OF AUTHORITY"));
        assert!(guided.contains("WHAT THIS CANNOT DO"));
        assert!(guided.contains("cannot grant or change approval policy"));
        assert!(guided.contains("Nothing becomes law until you confirm"));
        assert!(guided.contains("Rendered deterministically"));

        let drafted = constitution_ratification_text(
            Locale::En,
            &constitution,
            &DraftProvenance::Model("GLM-5.2".to_string()),
        );
        assert!(drafted.contains("Drafted by GLM-5.2"));
        assert!(drafted.contains("schema-checked and bounded by CodeWhale"));

        let zh = constitution_ratification_text(
            Locale::ZhHans,
            &draft.to_constitution(Locale::ZhHans),
            &DraftProvenance::Model("GLM-5.2".to_string()),
        );
        assert!(zh.contains("权限层级"));
        assert!(zh.contains("它不能做什么"));
        assert!(zh.contains("由 GLM-5.2 根据你的引导式答案起草"));
    }

    #[test]
    fn guided_constitution_detail_lines_show_localized_answers() {
        let english = SetupWizardView::new(SetupState::default(), Locale::En);
        let english_text = lines_to_text(english.constitution_detail_lines());
        assert!(english_text.contains("Purpose:"));
        assert!(english_text.contains("coding workbench"));
        assert!(english_text.contains("Initiative:"));
        assert!(english_text.contains("balanced"));
        assert!(english_text.contains("Principles:"));
        assert!(english_text.contains("scoped changes"));

        let zh_hans = SetupWizardView::new(SetupState::default(), Locale::ZhHans);
        let zh_hans_text = lines_to_text(zh_hans.constitution_detail_lines());
        assert!(zh_hans_text.contains("用途："));
        assert!(zh_hans_text.contains("编码工作台"));
        assert!(zh_hans_text.contains("主动性："));
        assert!(zh_hans_text.contains("平衡"));
        assert!(zh_hans_text.contains("原则："));
        assert!(zh_hans_text.contains("小范围改动"));

        for locale in Locale::shipped()
            .iter()
            .copied()
            .filter(|locale| *locale != Locale::En)
        {
            let view = SetupWizardView::new(SetupState::default(), locale);
            let text = lines_to_text(view.constitution_detail_lines());
            assert!(
                text.contains(&*GuidedPurpose::Coding.label(locale)),
                "missing localized purpose answer for {}",
                locale.tag()
            );
            assert!(
                text.contains(autonomy_label(AutonomyPreference::Balanced, locale)),
                "missing localized autonomy answer for {}",
                locale.tag()
            );
            assert!(
                text.contains(GuidedPrinciples::ScopedChanges.label(locale)),
                "missing localized principle answer for {}",
                locale.tag()
            );
            assert!(
                !text.contains("Purpose:"),
                "locale {} reused English detail label",
                locale.tag()
            );
            assert!(
                !text.contains("not checked yet"),
                "locale {} reused English file-state detail",
                locale.tag()
            );
        }
    }

    #[test]
    fn constitution_file_state_labels_existing_override_states() {
        assert!(
            SetupConstitutionFileState::Missing
                .label(ConstitutionChoice::Bundled, Locale::En)
                .contains("no constitution.json")
        );
        assert!(
            SetupConstitutionFileState::Loaded
                .label(ConstitutionChoice::GuidedCustom, Locale::En)
                .contains("selected")
        );
        assert!(
            SetupConstitutionFileState::Loaded
                .label(ConstitutionChoice::Bundled, Locale::En)
                .contains("inactive")
        );
        assert!(
            SetupConstitutionFileState::Invalid
                .label(ConstitutionChoice::Unset, Locale::En)
                .contains("invalid")
        );
        assert!(
            SetupConstitutionFileState::Unreadable
                .label(ConstitutionChoice::Unset, Locale::En)
                .contains("unreadable")
        );
        assert!(
            SetupConstitutionFileState::PathError
                .label(ConstitutionChoice::Unset, Locale::ZhHans)
                .contains("CODEWHALE_HOME")
        );
    }

    #[test]
    fn expert_override_state_requires_content_and_opt_in() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
        let _opt_in = crate::test_support::EnvVarGuard::remove(BASE_PROMPT_OVERRIDE_OPT_IN_ENV);

        assert_eq!(
            SetupExpertOverrideState::load(),
            SetupExpertOverrideState::Missing
        );

        let path = tmp.path().join(CONSTITUTION_OVERRIDE_FILE);
        std::fs::create_dir_all(path.parent().expect("override parent")).expect("override parent");
        std::fs::write(&path, "\n  \n").expect("write empty override");
        assert_eq!(
            SetupExpertOverrideState::load(),
            SetupExpertOverrideState::Empty
        );

        std::fs::write(&path, "# Expert override\n").expect("write override");
        assert_eq!(
            SetupExpertOverrideState::load(),
            SetupExpertOverrideState::Disabled
        );
        assert!(!SetupExpertOverrideState::Disabled.is_active());
        assert!(
            SetupExpertOverrideState::Disabled
                .label(Locale::En)
                .contains(BASE_PROMPT_OVERRIDE_OPT_IN_ENV)
        );

        // SAFETY: the process-wide test env mutex is held by `_guard`.
        unsafe { std::env::set_var(BASE_PROMPT_OVERRIDE_OPT_IN_ENV, "1") };
        assert_eq!(
            SetupExpertOverrideState::load(),
            SetupExpertOverrideState::Active
        );
        assert!(SetupExpertOverrideState::Active.is_active());
    }

    #[test]
    fn constitution_detail_lines_show_existing_file_state() {
        let mut state = SetupState {
            constitution_choice: ConstitutionChoice::Bundled,
            constitution_source: ConstitutionSource::Bundled,
            constitution_validity: ConstitutionValidity::Valid,
            ..SetupState::default()
        };
        let facts = SetupRuntimeFacts {
            constitution_file: SetupConstitutionFileState::Loaded,
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            state.clone(),
            Locale::En,
            SetupStep::Constitution,
            facts,
        );

        let text = lines_to_text(view.constitution_detail_lines());
        assert!(text.contains("Source: bundled; validity valid"));
        assert!(text.contains("Existing file:"));
        assert!(text.contains("inactive under the recorded choice"));
        assert!(text.contains("Expert override:"));
        assert!(text.contains("not checked yet"));

        state.constitution_choice = ConstitutionChoice::GuidedCustom;
        state.constitution_source = ConstitutionSource::UserGlobal;
        let view = SetupWizardView::new_at_with_facts(
            state,
            Locale::ZhHans,
            SetupStep::Constitution,
            SetupRuntimeFacts {
                constitution_file: SetupConstitutionFileState::Loaded,
                ..SetupRuntimeFacts::default()
            },
        );
        let text = lines_to_text(view.constitution_detail_lines());
        assert!(text.contains("现有文件："));
        assert!(text.contains("已存在并已选择"));
        assert!(text.contains("专家覆盖："));
    }

    #[test]
    fn setup_wizard_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        use ratatui::{buffer::Buffer, layout::Rect};
        use unicode_width::UnicodeWidthStr;

        const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(SetupWizardView::new_at_with_facts(
                SetupState::default(),
                Locale::En,
                SetupStep::Constitution,
                SetupRuntimeFacts {
                    constitution_file: SetupConstitutionFileState::Loaded,
                    ..SetupRuntimeFacts::default()
                },
            ));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            let text = rows.join("\n");

            for label in [
                "Setup",
                "Choice:",
                "Existing file:",
                "Purpose:",
                "preview/ratify",
                "use bundled",
                "cancel",
            ] {
                assert!(text.contains(label), "{w}x{h}: missing '{label}'");
            }
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into setup modal"
            );
            assert!(
                [palette::WHALE_BG, palette::WHALE_PANEL].contains(&buf[(w / 2, h / 2)].bg),
                "{w}x{h}: modal interior must be opaque"
            );
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= usize::from(w),
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }

    #[test]
    fn persist_user_constitution_choice_writes_constitution_and_state() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());
        let constitution = guided_constitution_template(Locale::En);
        let mut state = SetupState::default();
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::GuidedCustom,
        );
        state.constitution_source = ConstitutionSource::UserGlobal;
        state.constitution_validity = ConstitutionValidity::Valid;
        state.constitution_preview_hash = Some(constitution.preview_hash());
        state.set_step(
            SetupStep::Constitution,
            StepEntry::new(StepStatus::Verified, true, CONSTITUTION_CHECKPOINT_VERSION),
        );

        persist_user_constitution_choice(&constitution, &state).expect("persist constitution");

        let loaded_constitution = UserConstitution::load().expect("load constitution");
        assert!(matches!(
            loaded_constitution,
            UserConstitutionLoad::Loaded(_)
        ));
        let loaded_state = SetupState::load()
            .expect("load setup state")
            .expect("setup state");
        assert_eq!(
            loaded_state.constitution_choice,
            ConstitutionChoice::GuidedCustom
        );
        assert_eq!(
            loaded_state
                .constitution_checkpoint_completed_for
                .as_deref(),
            Some(CONSTITUTION_CHECKPOINT_VERSION)
        );
    }

    #[test]
    fn keep_existing_constitution_previews_then_completes_without_rewriting() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());

        // An existing valid custom constitution from a prior version.
        let existing = guided_constitution_template(Locale::En);
        persist_user_constitution_choice(&existing, &SetupState::default())
            .expect("write existing constitution");
        let path = UserConstitution::path().expect("constitution path");
        let bytes_before = std::fs::read(&path).expect("existing file bytes");

        let facts = SetupRuntimeFacts {
            constitution_file: SetupConstitutionFileState::Loaded,
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            facts,
        );

        // The card offers the keep path.
        let text = lines_to_text(view.constitution_detail_lines());
        assert!(text.contains("K Keep your existing constitution"), "{text}");

        // First K previews the existing law, unchanged, with keep wording.
        let action = view.handle_key(key(KeyCode::Char('k')));
        let ViewAction::Emit(ViewEvent::OpenTextPager { title, content }) = action else {
            panic!("expected keep-existing preview event");
        };
        assert!(title.contains("Draft for Ratification"));
        assert!(content.contains("shown unchanged"), "{content}");
        assert!(content.contains("press K to keep it"), "{content}");
        assert!(
            content.contains("<codewhale_user_constitution"),
            "{content}"
        );

        // Second K completes the checkpoint without touching the file.
        let action = view.handle_key(key(KeyCode::Char('k')));
        let ViewAction::EmitAndClose(ViewEvent::SetupStateCommitRequested { state, message }) =
            action
        else {
            panic!("expected keep-existing commit event");
        };
        assert_eq!(state.constitution_choice, ConstitutionChoice::GuidedCustom);
        assert_eq!(state.constitution_source, ConstitutionSource::UserGlobal);
        assert_eq!(state.constitution_validity, ConstitutionValidity::Valid);
        assert_eq!(
            state.constitution_checkpoint_completed_for.as_deref(),
            Some(CONSTITUTION_CHECKPOINT_VERSION)
        );
        assert_eq!(
            state.constitution_preview_hash.as_deref(),
            Some(existing.preview_hash().as_str())
        );
        assert_eq!(state.status(SetupStep::Constitution), StepStatus::Verified);
        assert!(message.contains("Constitution kept"), "{message}");

        let bytes_after = std::fs::read(&path).expect("file bytes after keep");
        assert_eq!(bytes_before, bytes_after, "keep must not rewrite the file");
    }

    #[test]
    fn keep_key_is_inert_without_a_valid_existing_constitution() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", tmp.path());

        for file_state in [
            SetupConstitutionFileState::Missing,
            SetupConstitutionFileState::Invalid,
            SetupConstitutionFileState::Empty,
        ] {
            let facts = SetupRuntimeFacts {
                constitution_file: file_state,
                ..SetupRuntimeFacts::default()
            };
            let mut view = SetupWizardView::new_at_with_facts(
                SetupState::default(),
                Locale::En,
                SetupStep::Constitution,
                facts,
            );
            let text = lines_to_text(view.constitution_detail_lines());
            assert!(
                !text.contains("K Keep your existing constitution"),
                "{file_state:?} must not offer keep: {text}"
            );
            assert!(
                matches!(view.handle_key(key(KeyCode::Char('k'))), ViewAction::None),
                "{file_state:?} must leave K inert"
            );
        }
    }

    #[test]
    fn provider_model_review_records_ready_route_and_continues() {
        let facts = SetupRuntimeFacts {
            provider: "DeepSeek".to_string(),
            model: "deepseek-v4-pro".to_string(),
            auth: "present".to_string(),
            health: "ready".to_string(),
            provider_ready: true,
            provider_result:
                "provider=deepseek, model=deepseek-v4-pro, auth=present/local, health=not checked"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::ProviderModel), StepStatus::Verified);
        assert_eq!(view.selected_step(), SetupStep::TrustSandbox);
        assert!(message.contains("Provider/model readiness recorded"));
    }

    #[test]
    fn provider_model_review_records_missing_auth_as_needs_action() {
        let facts = SetupRuntimeFacts {
            provider_ready: false,
            provider_result:
                "provider=deepseek, model=deepseek-v4-pro, auth=missing, health=needs action"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ProviderModel,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(
            state.status(SetupStep::ProviderModel),
            StepStatus::NeedsAction
        );
        assert!(message.contains("needs action"));
    }

    #[test]
    fn runtime_posture_review_confirms_without_config_mutation() {
        let facts = SetupRuntimeFacts {
            runtime_result: "intent=agent, approval=suggest, shell=enabled, trust=workspace, sandbox=default, network=prompt by default".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::TrustSandbox), StepStatus::Verified);
        assert_eq!(
            state.runtime_posture_source,
            RuntimePostureSource::Confirmed
        );
        assert!(message.contains("Runtime posture reviewed"));
        assert_eq!(view.selected_step(), SetupStep::Constitution);
    }

    #[test]
    fn runtime_posture_review_result_redacts_secret_config() {
        let _guard = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir");
        let codewhale_home = tmp.path().join(".codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = crate::test_support::EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);

        let mut config = Config {
            api_key: Some("sk-runtime-posture-secret".to_string()),
            sandbox_api_key: Some("sandbox-runtime-secret".to_string()),
            approval_policy: Some("on-request".to_string()),
            sandbox_mode: Some("workspace-write".to_string()),
            ..Config::default()
        };
        config.default_text_model = Some("deepseek-v4-pro".to_string());
        let app = App::new(setup_test_options(workspace), &config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, .. }) = action else {
            panic!("expected runtime posture commit event");
        };
        let result = state
            .steps
            .get(&SetupStep::TrustSandbox)
            .and_then(|entry| entry.result.as_deref())
            .expect("runtime posture result");
        assert!(result.contains("intent=agent"), "{result}");
        assert!(result.contains("sandbox=workspace-write"), "{result}");
        for forbidden in [
            "sk-runtime-posture-secret",
            "sandbox-runtime-secret",
            "api_key",
            "sandbox_api_key",
            "secret",
        ] {
            assert!(
                !result.contains(forbidden),
                "runtime posture result leaked {forbidden}: {result}"
            );
        }
    }

    #[test]
    fn runtime_posture_skip_records_posture_specific_state() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            SetupRuntimeFacts::default(),
        );

        let action = view.handle_key(key(KeyCode::Char('s')));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected runtime posture skip commit event");
        };
        let entry = state
            .steps
            .get(&SetupStep::TrustSandbox)
            .expect("trust/sandbox step entry");
        assert_eq!(entry.status, StepStatus::Skipped);
        assert!(entry.required);
        assert_eq!(entry.result.as_deref(), Some("skipped by user"));
        assert_eq!(state.runtime_posture_source, RuntimePostureSource::Unset);
        assert!(message.contains("skipped"));
        assert_eq!(view.selected_step(), SetupStep::Constitution);
    }

    #[test]
    fn runtime_posture_detail_lines_show_preset_diff() {
        let facts = SetupRuntimeFacts {
            default_mode: "agent".to_string(),
            approval_policy_value: "on-request".to_string(),
            allow_shell_enabled: true,
            sandbox_mode_value: "workspace-write".to_string(),
            network_default_value: "prompt".to_string(),
            trust: "workspace trust not elevated".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            facts,
        );

        let text = lines_to_text(view.runtime_posture_detail_lines());

        assert!(text.contains("Selected preset:"));
        assert!(text.contains("Normal agent"));
        assert!(text.contains("settings.default_mode: agent -> agent"));
        assert!(text.contains("config.allow_shell: true -> true"));
        assert!(text.contains("Safety floor:"));
        assert!(text.contains("Press A to preview"));
    }

    #[test]
    fn runtime_posture_detail_lines_warn_about_project_overrides() {
        let tmp = tempfile::TempDir::new().expect("workspace");
        let project_dir = tmp.path().join(codewhale_config::CODEWHALE_APP_DIR);
        std::fs::create_dir_all(&project_dir).expect("project config dir");
        std::fs::write(
            project_dir.join("config.toml"),
            "approval_policy = \"never\"\nsandbox_mode = \"read-only\"\n",
        )
        .expect("project config");
        let warning =
            project_runtime_override_warning(tmp.path(), Locale::En).expect("project warning");
        let facts = SetupRuntimeFacts {
            project_override_warning: Some(warning),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            facts,
        );

        let text = lines_to_text(view.runtime_posture_detail_lines());

        assert!(text.contains("Project override:"));
        assert!(text.contains("approval_policy=never"));
        assert!(text.contains("sandbox_mode=read-only"));
        assert!(text.contains("project override warning"));
        assert!(text.contains("project config can still tighten"));
    }

    #[test]
    fn operate_fleet_detail_lines_show_read_only_facts() {
        let facts = SetupRuntimeFacts {
            provider: "DeepSeek".to_string(),
            model: "deepseek-v4-pro".to_string(),
            auth: "present".to_string(),
            provider_ready: true,
            operate_runtime_ready: true,
            operate_runtime_result: "worker runtime enabled for deepseek; max_subagents=4, launch_concurrency=2, admission=6".to_string(),
            fleet_roster_ready: true,
            fleet_roster_result: "3 Fleet members (1 config/workspace)".to_string(),
            operate_concurrency_result:
                "configured launch_concurrency=2; max_subagents=4; admission=6; plan limit not probed"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            first_run_ready_state(),
            Locale::En,
            SetupStep::OperateFleet,
            facts,
        );

        let text = lines_to_text(view.operate_fleet_detail_lines());

        assert!(text.contains("Worker runtime:"));
        assert!(text.contains("worker runtime enabled for deepseek"));
        assert!(text.contains("Fleet roster:"));
        assert!(text.contains("3 Fleet members"));
        assert!(text.contains("plan limit not probed"));
        assert!(text.contains("Enter records this setup snapshot."));
    }

    #[test]
    fn operate_fleet_review_records_ready_without_plan_probe() {
        let facts = SetupRuntimeFacts {
            provider_ready: true,
            operate_runtime_ready: true,
            fleet_roster_ready: true,
            operate_result:
                "provider=ready, runtime=ready, roster=ready, concurrency=configured launch_concurrency=2; max_subagents=4; admission=6; plan limit not probed"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            first_run_ready_state(),
            Locale::En,
            SetupStep::OperateFleet,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::OperateFleet), StepStatus::Verified);
        assert!(state.operate_ready());
        let result = state
            .steps
            .get(&SetupStep::OperateFleet)
            .and_then(|entry| entry.result.as_deref())
            .expect("operate result");
        assert!(result.contains("plan limit not probed"), "{result}");
        assert!(message.contains("Operate/Fleet readiness recorded"));
        assert_eq!(view.selected_step(), SetupStep::Hotbar);
    }

    #[test]
    fn hotbar_detail_lines_show_read_only_config_facts() {
        let facts = SetupRuntimeFacts {
            hotbar_bindings_result: "customized; configured_slots=2; active_slots=2; warnings=0"
                .to_string(),
            hotbar_actions_result: "13 bindable actions registered".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let text = lines_to_text(view.hotbar_detail_lines());

        assert!(text.contains("Hotbar bindings:"));
        assert!(text.contains("configured_slots=2"));
        assert!(text.contains("Bindable actions:"));
        assert!(text.contains("13 bindable actions"));
        assert!(text.contains("Enter records this setup snapshot. Press H to customize slots."));
    }

    #[test]
    fn hotbar_review_records_optional_snapshot() {
        let facts = SetupRuntimeFacts {
            hotbar_result:
                "state=customized, configured_slots=2, active_slots=2, actions=13, warnings=0"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::Hotbar), StepStatus::Verified);
        let entry = state
            .steps
            .get(&SetupStep::Hotbar)
            .expect("hotbar setup entry");
        assert!(!entry.required);
        assert!(
            entry
                .result
                .as_deref()
                .is_some_and(|result| result.contains("state=customized"))
        );
        assert!(message.contains("Hotbar setup state recorded"));
        assert_eq!(view.selected_step(), SetupStep::ToolsMcp);
    }

    #[test]
    fn tools_mcp_detail_lines_show_read_only_inventory_facts() {
        let facts = SetupRuntimeFacts {
            tools_mcp_servers_result: "healthy — 2 configured (2 healthy, 0 needs_config, 0 off; global present at /tmp/mcp.json; project missing at /tmp/project/.codewhale/mcp.json); healthy: docs, search".to_string(),
            tools_mcp_skills_result: "healthy — 3 discovered (hotbar skill sources), 3 on disk at /tmp/skills".to_string(),
            tools_mcp_tools_result: "healthy — 1 entries, 0 script-plugin tools at /tmp/tools".to_string(),
            tools_mcp_plugins_result: "off — nothing configured yet (missing at /tmp/plugins); optional".to_string(),
            tools_mcp_hotbar_result: "healthy — shared adapters: mcp_actions=0, skill_actions=3, plugin_actions=0 (deferred), slash_actions=12".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ToolsMcp,
            facts,
        );

        let text = lines_to_text(view.tools_mcp_detail_lines());

        assert!(text.contains("MCP servers:"));
        assert!(text.contains("healthy"));
        assert!(text.contains("/tmp/mcp.json"));
        assert!(text.contains("/tmp/project/.codewhale/mcp.json"));
        assert!(text.contains("Skills:"));
        assert!(text.contains("/tmp/skills"));
        assert!(text.contains("Tools dir:"));
        assert!(text.contains("Plugins:"));
        assert!(text.contains("Hotbar sources:"));
        assert!(text.contains("shared adapters"));
        assert!(text.contains("Enter records this setup snapshot."));
        assert!(text.contains("Press R for safe on-ramps"));
    }

    #[test]
    fn tools_mcp_review_records_optional_snapshot_when_empty() {
        let facts = SetupRuntimeFacts {
            tools_mcp_result:
                "mcp=off, skills=off, tools=off, plugins=off, hotbar_sources=shared adapters: mcp_actions=0, overall=off, mode=read_only_safe_probe"
                    .to_string(),
            tools_mcp_needs_action: false,
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ToolsMcp,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::ToolsMcp), StepStatus::Optional);
        let entry = state
            .steps
            .get(&SetupStep::ToolsMcp)
            .expect("tools/mcp setup entry");
        assert!(!entry.required);
        assert!(
            entry
                .result
                .as_deref()
                .is_some_and(|result| result.contains("mode=read_only_safe_probe"))
        );
        assert!(message.contains("Tools/MCP readiness recorded"));
        assert_eq!(view.selected_step(), SetupStep::RemoteRuntime);
    }

    #[test]
    fn tools_mcp_review_records_needs_action_for_broken_config() {
        let facts = SetupRuntimeFacts {
            tools_mcp_result:
                "mcp=needs_config, skills=off, tools=off, plugins=off, overall=needs_config, mode=read_only_safe_probe"
                    .to_string(),
            tools_mcp_needs_action: true,
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ToolsMcp,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));
        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::ToolsMcp), StepStatus::NeedsAction);
        assert!(
            !state
                .steps
                .get(&SetupStep::ToolsMcp)
                .expect("entry")
                .required
        );
        assert!(message.contains("needs action") || message.contains("Tools/MCP"));
        // Optional step still advances; first-run is not blocked.
        assert_eq!(view.selected_step(), SetupStep::RemoteRuntime);
    }

    #[test]
    fn tools_mcp_on_ramp_preview_is_safe() {
        let facts = SetupRuntimeFacts {
            tools_mcp_servers_result: "off — nothing configured".into(),
            tools_mcp_skills_result: "off — missing".into(),
            tools_mcp_tools_result: "off — missing".into(),
            tools_mcp_plugins_result: "off — missing".into(),
            tools_mcp_hotbar_result: "off — shared adapters".into(),
            tools_mcp_path_display: "~/.codewhale/mcp.json".into(),
            tools_mcp_skills_path_display: "~/.codewhale/skills".into(),
            tools_mcp_plugins_path_display: "~/.codewhale/plugins".into(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::ToolsMcp,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Char('r')));
        let ViewAction::Emit(ViewEvent::OpenTextPager { title, content }) = action else {
            panic!("expected on-ramp pager, got {action:?}");
        };
        assert!(title.to_ascii_lowercase().contains("tool") || title.contains("MCP"));
        assert!(content.contains("/mcp") || content.contains("mcp init"));
        assert!(!content.contains("sk-"));
    }

    #[test]
    fn remote_runtime_detail_lines_show_read_only_registry_facts() {
        let facts = SetupRuntimeFacts {
            remote_clouds_result: "3 cloud targets: lighthouse, azure, digitalocean".to_string(),
            remote_bridges_result: "2 chat bridges: feishu, telegram".to_string(),
            remote_providers_result:
                "12 providers from the provider registry; active route deepseek / deepseek-chat"
                    .to_string(),
            remote_mode_result:
                "generate-only bundle; --apply not implemented; default port 7878, workers 2"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::RemoteRuntime,
            facts,
        );

        let text = lines_to_text(view.remote_runtime_detail_lines());

        assert!(text.contains("Cloud targets:"));
        assert!(text.contains("lighthouse"));
        assert!(text.contains("Chat bridges:"));
        assert!(text.contains("feishu"));
        assert!(text.contains("Remote mode:"));
        assert!(text.contains("--apply not implemented"));
        assert!(text.contains("Enter records this setup snapshot. Press R to preview."));
    }

    #[test]
    fn remote_runtime_review_records_optional_snapshot() {
        let facts = SetupRuntimeFacts {
            remote_result:
                "clouds=3, bridges=2, providers=12, mode=generate_only, apply=not_implemented"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::RemoteRuntime,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::RemoteRuntime), StepStatus::Verified);
        let entry = state
            .steps
            .get(&SetupStep::RemoteRuntime)
            .expect("remote setup entry");
        assert!(!entry.required);
        assert!(
            entry
                .result
                .as_deref()
                .is_some_and(|result| result.contains("mode=generate_only"))
        );
        assert!(message.contains("Remote runtime on-ramp recorded"));
        assert_eq!(view.selected_step(), SetupStep::Persistence);
    }

    #[test]
    fn persistence_detail_lines_show_read_only_path_facts() {
        let facts = SetupRuntimeFacts {
            persistence: SetupPersistenceFacts {
                home_result: "explicit CODEWHALE_HOME at /tmp/cw-home (present)".to_string(),
                config_result: "/tmp/cw-home/config.toml (present)".to_string(),
                state_result: "/tmp/cw-home/setup_state.json (missing)".to_string(),
                constitution_result: "/tmp/cw-home/constitution.json (present)".to_string(),
                memory_result: "/tmp/cw-home/memory.md (missing)".to_string(),
                notes_result: "/tmp/cw-home/notes.md (exists-not-file)".to_string(),
                result: "home_source=explicit, home=present, config=present, setup_state=missing, constitution=present, memory=missing, notes=exists-not-file, mode=read_only_review".to_string(),
            },
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Persistence,
            facts,
        );

        let text = lines_to_text(view.persistence_detail_lines());

        assert!(text.contains("Home:"));
        assert!(text.contains("explicit CODEWHALE_HOME"));
        assert!(text.contains("/tmp/cw-home/config.toml"));
        assert!(text.contains("/tmp/cw-home/setup_state.json (missing)"));
        assert!(text.contains("Constitution:"));
        assert!(text.contains("Memory:"));
        assert!(text.contains("Notes:"));
        assert!(text.contains("Enter records this setup snapshot."));
    }

    #[test]
    fn persistence_review_records_optional_snapshot() {
        let facts = SetupRuntimeFacts {
            persistence: SetupPersistenceFacts {
                result: "home_source=explicit, home=present, config=present, setup_state=missing, constitution=present, memory=missing, notes=missing, mode=read_only_review".to_string(),
                ..SetupPersistenceFacts::default()
            },
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Persistence,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::Persistence), StepStatus::Verified);
        let entry = state
            .steps
            .get(&SetupStep::Persistence)
            .expect("persistence setup entry");
        assert!(!entry.required);
        assert!(
            entry
                .result
                .as_deref()
                .is_some_and(|result| result.contains("mode=read_only_review"))
        );
        assert!(message.contains("Persistence paths recorded"));
        assert_eq!(view.selected_step(), SetupStep::Verification);
    }

    #[test]
    fn operate_fleet_review_records_needs_action_until_first_run_ready() {
        let facts = SetupRuntimeFacts {
            provider_ready: true,
            operate_runtime_ready: true,
            fleet_roster_ready: true,
            operate_result:
                "provider=ready, runtime=ready, roster=ready, concurrency=plan limit not probed"
                    .to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::OperateFleet,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(
            state.status(SetupStep::OperateFleet),
            StepStatus::NeedsAction
        );
        assert!(!state.operate_ready());
        assert!(message.contains("needs action"));
    }

    #[test]
    fn runtime_posture_preset_requires_preview_before_apply() {
        let facts = SetupRuntimeFacts {
            default_mode: "agent".to_string(),
            approval_policy_value: "never".to_string(),
            allow_shell_enabled: false,
            sandbox_mode_value: "read-only".to_string(),
            network_default_value: "deny".to_string(),
            trust: "workspace trust not elevated".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::TrustSandbox,
            facts,
        );

        assert!(matches!(
            view.handle_key(key(KeyCode::Char('3'))),
            ViewAction::None
        ));
        let preview = view.handle_key(key(KeyCode::Char('a')));
        let ViewAction::Emit(ViewEvent::OpenTextPager { content, .. }) = preview else {
            panic!("first apply should preview the exact diff");
        };
        assert!(content.contains("Runtime Posture Preset Preview"));
        assert!(content.contains("settings.default_mode: agent -> yolo"));
        assert!(content.contains(
            "config.approval_policy: never -> unchanged; Full Access derives bypass from default_mode"
        ));
        assert!(content.contains("config.network.default: deny -> unchanged"));

        let action = view.handle_key(key(KeyCode::Char('a')));
        let ViewAction::Emit(ViewEvent::SetupRuntimePresetApplyRequested {
            preset,
            state,
            message,
        }) = action
        else {
            panic!("second apply should request preset persistence");
        };
        assert_eq!(preset, SetupRuntimePreset::HighTrustLocal);
        assert_eq!(state.status(SetupStep::TrustSandbox), StepStatus::Verified);
        assert_eq!(
            state.runtime_posture_source,
            RuntimePostureSource::Confirmed
        );
        assert!(
            state
                .steps
                .get(&SetupStep::TrustSandbox)
                .and_then(|entry| entry.result.as_deref())
                .is_some_and(|result| {
                    result.contains("preset=high-trust-local")
                        && result.contains("default_mode=yolo")
                        && result.contains("network=unchanged")
                })
        );
        assert!(message.contains("Runtime preset applied"));
        assert_eq!(view.selected_step(), SetupStep::Constitution);
    }

    #[test]
    fn verification_report_records_needs_action_until_checkpoint_complete() {
        let facts = SetupRuntimeFacts {
            constitution_autonomy: "balanced".to_string(),
            runtime_result: "intent=agent, approval=suggest".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Verification,
            facts,
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(
            state.status(SetupStep::Verification),
            StepStatus::NeedsAction
        );
        assert!(
            state
                .steps
                .get(&SetupStep::Verification)
                .and_then(|entry| entry.result.as_deref())
                .is_some_and(|result| {
                    result.contains("update=needs_action")
                        && result.contains("operate=needs_action")
                        && result.contains("autonomy=balanced")
                        && result.contains("runtime=intent=agent, approval=suggest")
                })
        );
        assert!(message.contains("Setup report recorded"));
    }

    #[test]
    fn verification_report_records_ready_after_bundled_checkpoint() {
        let mut state = SetupState::default();
        state.complete_constitution_checkpoint(
            CONSTITUTION_CHECKPOINT_VERSION,
            ConstitutionChoice::Bundled,
        );
        let mut view = SetupWizardView::new_at_with_facts(
            state,
            Locale::En,
            SetupStep::Verification,
            SetupRuntimeFacts::default(),
        );

        let action = view.handle_key(key(KeyCode::Enter));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, .. }) = action else {
            panic!("expected setup-state commit event");
        };
        assert_eq!(state.status(SetupStep::Verification), StepStatus::Verified);
        assert!(
            state
                .steps
                .get(&SetupStep::Verification)
                .and_then(|entry| entry.result.as_deref())
                .is_some_and(|result| {
                    result.contains("update=ready") && result.contains("operate=needs_action")
                })
        );
    }

    #[test]
    fn verification_detail_lines_show_next_action() {
        let facts = SetupRuntimeFacts {
            constitution_autonomy: "balanced".to_string(),
            runtime_result: "intent=agent, approval=suggest".to_string(),
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Verification,
            facts,
        );

        let text = lines_to_text(view.verification_detail_lines());

        assert!(text.contains("First-run:"));
        assert!(text.contains("Update checkpoint:"));
        assert!(text.contains("Operate/Fleet:"));
        assert!(text.contains("Constitution autonomy:"));
        assert!(text.contains("balanced"));
        assert!(text.contains("Runtime posture:"));
        assert!(text.contains("intent=agent, approval=suggest"));
        assert!(text.contains("Complete the constitution checkpoint"));
    }

    #[test]
    fn setup_wizard_body_scroll_resets_on_step_change() {
        let mut view = SetupWizardView::new(SetupState::default(), Locale::En);
        view.body_scroll = 12;
        view.move_next();
        assert_eq!(view.body_scroll, 0, "step change should reset body scroll");
        view.body_scroll = 5;
        view.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert!(view.body_scroll >= 5);
        view.move_back();
        assert_eq!(view.body_scroll, 0);
    }

    #[test]
    fn setup_wizard_page_down_clamps_scroll_at_80x24() {
        use ratatui::text::{Line, Span};

        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Constitution,
            SetupRuntimeFacts {
                constitution_file: SetupConstitutionFileState::Loaded,
                ..SetupRuntimeFacts::default()
            },
        );
        let wrap_width = 76usize;
        let visible_rows = 10usize;
        let mut lines = view.constitution_detail_lines();
        lines.extend(std::iter::repeat_n(
            Line::from(Span::raw("x".repeat(wrap_width))),
            40,
        ));
        let visual_rows: usize = lines
            .iter()
            .map(|line| line.width().div_ceil(wrap_width).max(1))
            .sum();
        let max_scroll = visual_rows.saturating_sub(visible_rows);
        assert!(max_scroll > 0, "fixture should overflow a small viewport");

        for _ in 0..32 {
            view.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        }
        assert!(
            view.body_scroll >= max_scroll.saturating_sub(8),
            "page down should reach the scroll ceiling"
        );

        let clamped = view.body_scroll.min(max_scroll);
        assert_eq!(
            clamped, max_scroll,
            "render path should clamp overshoot to max scroll"
        );
    }

    fn lines_to_text(lines: Vec<Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
