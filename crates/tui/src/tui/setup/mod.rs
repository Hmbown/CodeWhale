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
use crate::prompts::CONSTITUTION_OVERRIDE_FILE;
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
};

mod fleet_draft;
mod model_draft;

pub(crate) use fleet_draft::draft_fleet_profile_with_model;
pub(crate) use model_draft::draft_constitution_with_model;

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

const STEP_SPECS: [StaticSetupStep; 8] = [
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
        id: SetupStep::ToolsMcp,
        title_id: MessageId::SetupStepToolsMcpTitle,
        why_id: MessageId::SetupStepToolsMcpWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Hotbar,
        title_id: MessageId::SetupStepHotbarTitle,
        why_id: MessageId::SetupStepHotbarWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::RemoteRuntime,
        title_id: MessageId::SetupStepRemoteRuntimeTitle,
        why_id: MessageId::SetupStepRemoteRuntimeWhy,
        required: false,
    },
    StaticSetupStep {
        id: SetupStep::Constitution,
        title_id: MessageId::SetupStepConstitutionTitle,
        why_id: MessageId::SetupStepConstitutionWhy,
        required: true,
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
    default_mode: String,
    approval_policy_value: String,
    project_override_warning: Option<String>,
    constitution_autonomy: String,
    constitution_file: SetupConstitutionFileState,
    /// Hotbar status summary: off by default, disabled, configured, partial,
    /// or unknown. Setup reports this state but delegates writes to /hotbar.
    hotbar_status: String,
    /// Number of configured hotbar bindings (0 when disabled or off).
    hotbar_bindings_count: usize,
    hotbar_off_by_default: bool,
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
            default_mode: "agent".to_string(),
            approval_policy_value: "on-request".to_string(),
            project_override_warning: None,
            constitution_autonomy: "not loaded".to_string(),
            constitution_file: SetupConstitutionFileState::NotChecked,
            hotbar_status: "not loaded".to_string(),
            hotbar_bindings_count: 0,
            hotbar_off_by_default: false,
        }
    }
}

impl SetupRuntimeFacts {
    fn from_app_config(app: &App, config: &Config) -> Self {
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
            "ready for first turn; live validation remains with /provider"
        } else if app.api_provider == crate::config::ApiProvider::OpenaiCodex {
            "run codex login or set OPENAI_CODEX_ACCESS_TOKEN before first turn"
        } else {
            "needs key or local runtime before first turn"
        }
        .to_string();
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
        let constitution_autonomy = UserConstitution::load()
            .ok()
            .and_then(|load| {
                load.constitution().map(|constitution| {
                    autonomy_label(constitution.autonomy_preference, app.ui_locale).to_string()
                })
            })
            .unwrap_or_else(|| match app.ui_locale {
                Locale::ZhHans => "未指定或使用内置准则".to_string(),
                _ => "unspecified or bundled/default".to_string(),
            });

        let (hotbar_status, hotbar_bindings_count, hotbar_off_by_default) =
            hotbar_status_from_config(app, config);

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
            hotbar_status,
            hotbar_bindings_count,
            hotbar_off_by_default,
        }
    }
}

fn hotbar_status_from_config(app: &App, config: &Config) -> (String, usize, bool) {
    let known_action_ids: Vec<&str> = app.hotbar_actions.iter().map(|a| a.id()).collect();
    let resolution = config.resolve_hotbar_bindings(&known_action_ids);
    let binding_count = resolution.bindings.len();
    let warning_count = resolution.warnings.len();
    let unknown_count = resolution
        .warnings
        .iter()
        .filter(|warning| {
            matches!(
                warning,
                codewhale_config::HotbarConfigWarning::UnknownAction { .. }
            )
        })
        .count();

    match config.hotbar.as_ref() {
        None => ("off by default".to_string(), 0, true),
        Some(bindings) if bindings.is_empty() => ("disabled".to_string(), 0, false),
        Some(_) if unknown_count > 0 => (
            format!(
                "unknown: {binding_count} binding(s), {unknown_count} unknown action(s), {warning_count} warning(s)"
            ),
            binding_count,
            false,
        ),
        Some(_) if binding_count < usize::from(codewhale_config::HOTBAR_SLOT_COUNT) => {
            let mut status = format!(
                "partial: {binding_count} of {} slots",
                codewhale_config::HOTBAR_SLOT_COUNT
            );
            if warning_count > 0 {
                status.push_str(&format!(", {warning_count} warning(s)"));
            }
            (status, binding_count, false)
        }
        Some(_) if warning_count > 0 => (
            format!("configured: {binding_count} binding(s), {warning_count} warning(s)"),
            binding_count,
            false,
        ),
        Some(_) => (
            format!("configured: {binding_count} binding(s)"),
            binding_count,
            false,
        ),
    }
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

    fn label(self, choice: ConstitutionChoice, locale: Locale) -> &'static str {
        match locale {
            Locale::ZhHans => self.zh_hans_label(choice),
            _ => self.english_label(choice),
        }
    }

    fn english_label(self, choice: ConstitutionChoice) -> &'static str {
        match self {
            Self::NotChecked => "not checked yet",
            Self::Missing => "no constitution.json found; bundled/default applies",
            Self::Loaded if choice == ConstitutionChoice::GuidedCustom => {
                "valid constitution.json present and selected"
            }
            Self::Loaded if choice.is_explicit() => {
                "valid constitution.json present but inactive under the recorded choice"
            }
            Self::Loaded => "valid constitution.json present; preview or save guided to select it",
            Self::Empty => "constitution.json is empty; use G to regenerate or U for bundled",
            Self::Invalid => "constitution.json is invalid; use repair/regenerate or bundled",
            Self::Unreadable => "constitution.json is unreadable; use repair/regenerate or bundled",
            Self::PathError => "CODEWHALE_HOME could not be resolved for constitution.json",
        }
    }

    fn zh_hans_label(self, choice: ConstitutionChoice) -> &'static str {
        match self {
            Self::NotChecked => "尚未检查",
            Self::Missing => "未找到 constitution.json；使用内置/默认准则",
            Self::Loaded if choice == ConstitutionChoice::GuidedCustom => {
                "有效 constitution.json 已存在并已选择"
            }
            Self::Loaded if choice.is_explicit() => {
                "有效 constitution.json 已存在，但当前记录选择使其不生效"
            }
            Self::Loaded => "有效 constitution.json 已存在；预览或保存引导式宪法即可选择",
            Self::Empty => "constitution.json 为空；按 G 重新生成或按 U 使用内置",
            Self::Invalid => "constitution.json 无效；请修复/重新生成，或使用内置",
            Self::Unreadable => "constitution.json 无法读取；请修复/重新生成，或使用内置",
            Self::PathError => "无法解析 CODEWHALE_HOME 中的 constitution.json",
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

    fn to_constitution(self, locale: Locale) -> UserConstitution {
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
            notes: Some(self.notes(locale)),
            ..UserConstitution::default()
        }
    }

    fn notes(self, locale: Locale) -> String {
        match locale {
            Locale::ZhHans => format!(
                "引导式答案：用途={}；主动性={}；证据={}；沟通={}；隐私={}；原则={}。{} 自由文本原则只作为建议，不会改变审批、沙箱、Shell、网络、信任或 MCP 权限。",
                self.purpose.label(locale),
                autonomy_label(self.autonomy, locale),
                self.evidence.label(locale),
                self.communication.label(locale),
                self.privacy.label(locale),
                self.principles.label(locale),
                self.principles.note(locale)
            ),
            _ => format!(
                "Guided answers: purpose={}; initiative={}; evidence={}; communication={}; privacy={}; principles={}. {} Freeform principles are advisory and do not change approval, sandbox, shell, network, trust, or MCP permissions.",
                self.purpose.label(locale),
                autonomy_label(self.autonomy, locale),
                self.evidence.label(locale),
                self.communication.label(locale),
                self.privacy.label(locale),
                self.principles.label(locale),
                self.principles.note(locale)
            ),
        }
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

    fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Coding) => "编码工作台",
            (Locale::ZhHans, Self::Research) => "研究综合",
            (Locale::ZhHans, Self::Operations) => "运维协作",
            (Locale::ZhHans, Self::Mixed) => "混合工作台",
            (_, Self::Coding) => "coding workbench",
            (_, Self::Research) => "research synthesis",
            (_, Self::Operations) => "operations helper",
            (_, Self::Mixed) => "mixed workbench",
        }
    }

    fn about(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Coding) => "希望 CodeWhale 成为稳健、重证据的编码工作台用户。",
            (Locale::ZhHans, Self::Research) => {
                "希望 CodeWhale 帮助梳理实时资料、引用证据并谨慎综合研究的用户。"
            }
            (Locale::ZhHans, Self::Operations) => {
                "希望 CodeWhale 协助可靠执行运维任务、保留回滚点并明确风险的用户。"
            }
            (Locale::ZhHans, Self::Mixed) => {
                "希望 CodeWhale 在编码、研究、写作和运维之间灵活切换的用户。"
            }
            (_, Self::Coding) => {
                "A CodeWhale user who wants a calm, evidence-first coding workbench."
            }
            (_, Self::Research) => {
                "A CodeWhale user who wants current, cited research and careful synthesis."
            }
            (_, Self::Operations) => {
                "A CodeWhale user who wants reliable operational help with clear rollback points."
            }
            (_, Self::Mixed) => {
                "A CodeWhale user who wants a flexible workbench for coding, research, writing, and operations."
            }
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Coding) => "让代码改动贴近请求、仓库模式和可验证行为。",
            (Locale::ZhHans, Self::Research) => "区分实时证据与推断，并为易变事实引用来源。",
            (Locale::ZhHans, Self::Operations) => {
                "优先使用可逆运维步骤、预演、状态检查和回滚说明。"
            }
            (Locale::ZhHans, Self::Mixed) => {
                "可在编码、研究、写作和运维之间切换，但安全姿态不随意扩大。"
            }
            (_, Self::Coding) => {
                "Keep code changes scoped to requested behavior and existing repo patterns."
            }
            (_, Self::Research) => {
                "Separate live evidence from inference and cite sources for unstable facts."
            }
            (_, Self::Operations) => {
                "Prefer reversible operational steps with dry-runs, status checks, and rollback notes."
            }
            (_, Self::Mixed) => {
                "Adapt between coding, research, writing, and operations without widening the safety posture."
            }
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

    fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Assumptions) => "说明假设",
            (Locale::ZhHans, Self::TestsAndReceipts) => "测试/凭据",
            (Locale::ZhHans, Self::ReleaseReceipts) => "发布凭据",
            (_, Self::Assumptions) => "assumptions",
            (_, Self::TestsAndReceipts) => "tests/receipts",
            (_, Self::ReleaseReceipts) => "release receipts",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Assumptions) => "在宣称完成前总结假设、未知和剩余风险。",
            (Locale::ZhHans, Self::TestsAndReceipts) => {
                "在能降低不确定性时，用命令、测试、截图或引用给出具体验证。"
            }
            (Locale::ZhHans, Self::ReleaseReceipts) => {
                "对重要结论和发布证据标注文件、命令、截图、CI 或来源。"
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
            (Locale::ZhHans, Self::Concise) => "简洁",
            (Locale::ZhHans, Self::Teaching) => "教学式",
            (Locale::ZhHans, Self::Direct) => "直接",
            (_, Self::Concise) => "concise",
            (_, Self::Teaching) => "teaching",
            (_, Self::Direct) => "direct",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Concise) => "保持更新简洁，并只解释重要取舍。",
            (Locale::ZhHans, Self::Teaching) => "解释关键推理和取舍，让用户能理解系统。",
            (Locale::ZhHans, Self::Direct) => "直接说明阻塞、风险和不确定性，避免装饰性文案。",
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
            (Locale::ZhHans, Self::StandardCare) => "标准保护",
            (Locale::ZhHans, Self::StrictBoundaries) => "严格边界",
            (Locale::ZhHans, Self::ProjectLocal) => "项目内记忆",
            (_, Self::StandardCare) => "standard care",
            (_, Self::StrictBoundaries) => "strict boundaries",
            (_, Self::ProjectLocal) => "project-local memory",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::StandardCare) => {
                "保护密钥、用户文件、Git 历史、生产系统、成本、隐私和时间。"
            }
            (Locale::ZhHans, Self::StrictBoundaries) => {
                "把密钥、个人数据、凭据、生产状态、资金和发布动作视为先确认边界。"
            }
            (Locale::ZhHans, Self::ProjectLocal) => {
                "项目特定上下文留在项目内，除非明确要求，否则不要写入记忆。"
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
            (Locale::ZhHans, Self::StandardCare) => {
                "遇到破坏性、高成本、凭据、发布、法律或安全风险操作时先询问。"
            }
            (Locale::ZhHans, Self::StrictBoundaries) => {
                "在读取或传播敏感信息、触碰生产系统、花费资金或发布内容前停止并询问。"
            }
            (Locale::ZhHans, Self::ProjectLocal) => {
                "需要跨项目记忆、复制项目细节或引用旧交接时，先确认这些上下文仍适用。"
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
            (Locale::ZhHans, Self::ScopedChanges) => "小范围改动",
            (Locale::ZhHans, Self::UserVoice) => "保留用户语气",
            (Locale::ZhHans, Self::ReversibleOps) => "可逆步骤",
            (_, Self::ScopedChanges) => "scoped changes",
            (_, Self::UserVoice) => "user voice",
            (_, Self::ReversibleOps) => "reversible steps",
        }
    }

    fn note(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::ScopedChanges) => {
                "自由原则：优先采用小范围、可审查的改动；除非明确要求，不做无关重构。"
            }
            (Locale::ZhHans, Self::UserVoice) => {
                "自由原则：保留用户的语气、品牌和约束；不把偏好推断成权限扩大。"
            }
            (Locale::ZhHans, Self::ReversibleOps) => {
                "自由原则：先选择可逆步骤、检查点和回滚说明，再进行高影响操作。"
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
        (Locale::ZhHans, AutonomyPreference::Cautious) => "谨慎",
        (Locale::ZhHans, AutonomyPreference::Balanced) => "平衡",
        (Locale::ZhHans, AutonomyPreference::Autonomous) => "积极主动",
        (_, AutonomyPreference::Cautious) => "cautious",
        (_, AutonomyPreference::Balanced) => "balanced",
        (_, AutonomyPreference::Autonomous) => "ambitious",
        (_, AutonomyPreference::Unspecified) => "unspecified",
    }
}

fn autonomy_priority(preference: AutonomyPreference, locale: Locale) -> &'static str {
    match (locale, preference) {
        (Locale::ZhHans, AutonomyPreference::Cautious) => {
            "在编辑文件、运行命令或产品选择不明确前，倾向先停下询问。"
        }
        (Locale::ZhHans, AutonomyPreference::Balanced) => {
            "清晰低风险任务可直接行动；遇到风险、破坏性或歧义时先确认。"
        }
        (Locale::ZhHans, AutonomyPreference::Autonomous) => {
            "可批量处理安全的常规工作，但遇到破坏性、凭据、发布、高成本、法律或安全风险时停止询问。"
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
        Locale::ZhHans => "当前用户请求和实时工具证据优先于记忆、陈旧交接和猜测。",
        _ => {
            "Current user requests and live tool evidence outrank memory, stale handoffs, and guesses."
        }
    }
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
            guided_preview_seen: false,
            existing_preview_seen: false,
            model_draft: None,
            model_draft_label: None,
            runtime_preset: SetupRuntimePreset::default(),
            runtime_preset_preview_seen: false,
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
            guided_preview_seen: false,
            existing_preview_seen: false,
            model_draft: None,
            model_draft_label: None,
            runtime_preset: SetupRuntimePreset::default(),
            runtime_preset_preview_seen: false,
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
            selected: step_index(step),
            locale,
            facts,
            guided_draft: GuidedConstitutionDraft::default(),
            guided_preview_seen: false,
            existing_preview_seen: false,
            model_draft: None,
            model_draft_label: None,
            runtime_preset: SetupRuntimePreset::default(),
            runtime_preset_preview_seen: false,
        }
    }

    fn move_next(&mut self) {
        self.selected = (self.selected + 1).min(STEP_SPECS.len().saturating_sub(1));
    }

    fn move_back(&mut self) {
        self.selected = self.selected.saturating_sub(1);
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

    fn commit_provider_model_review(&mut self) -> ViewAction {
        let status = if self.facts.provider_ready {
            StepStatus::Verified
        } else {
            StepStatus::NeedsAction
        };
        let mut state = self.state.clone();
        state.set_step(
            SetupStep::ProviderModel,
            StepEntry::new(status, true, CONSTITUTION_CHECKPOINT_VERSION)
                .with_result(self.facts.provider_result.clone()),
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
                self.guided_draft.to_constitution(self.locale),
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
                self.guided_draft.to_constitution(self.locale),
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
            locale: self.locale,
        })
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
            KeyCode::Char('m') if self.selected_step() == SetupStep::TrustSandbox => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenModeRequested)
            }
            KeyCode::Char('c') if self.selected_step() == SetupStep::TrustSandbox => {
                ViewAction::EmitAndClose(ViewEvent::SetupOpenConfigRequested)
            }
            KeyCode::Char('h') if self.selected_step() == SetupStep::Hotbar => {
                ViewAction::Emit(ViewEvent::SetupOpenHotbarRequested)
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
            KeyCode::Char('k') if self.selected_step() == SetupStep::Constitution => {
                self.commit_keep_existing_constitution()
            }
            KeyCode::Char('u') => self.commit_constitution(SetupCommitKind::BundledConstitution),
            KeyCode::Char('d') => self.commit_constitution(SetupCommitKind::DeferredConstitution),
            KeyCode::Enter if self.selected_step() == SetupStep::Constitution => {
                self.commit_constitution(SetupCommitKind::BundledConstitution)
            }
            KeyCode::Enter if self.selected_step() == SetupStep::ProviderModel => {
                self.commit_provider_model_review()
            }
            KeyCode::Enter if self.selected_step() == SetupStep::TrustSandbox => {
                self.commit_runtime_posture_review()
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
            .style(Style::default().bg(palette::DEEPSEEK_SLATE))
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
        } else if self.selected_step() == SetupStep::Hotbar {
            hints.push(ActionHint::new(
                "H",
                tr(self.locale, MessageId::SetupActionHotbar).to_string(),
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
                    .fg(palette::DEEPSEEK_SKY)
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
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
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
            SetupStep::Hotbar => self.hotbar_detail_lines(),
            SetupStep::Constitution => self.constitution_detail_lines(),
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
            self.detail_row(MessageId::SetupConstitutionExistingLabel, existing_file),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupConstitutionGuidedAnswersHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            self.guided_answer_pair(
                (
                    "1",
                    MessageId::SetupConstitutionPurposeLabel,
                    self.guided_draft.purpose.label(self.locale),
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
                    self.guided_draft.evidence.label(self.locale),
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
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupRuntimePostureReviewHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
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

    fn hotbar_detail_lines(&self) -> Vec<Line<'static>> {
        let binding_count = format!("{}", self.facts.hotbar_bindings_count);
        let mut lines = vec![
            self.detail_row(MessageId::SetupCardHotbarLabel, &self.facts.hotbar_status),
            self.detail_row(MessageId::SetupCardHotbarBindingsLabel, &binding_count),
            Line::from(""),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupHotbarLaunchHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(Span::styled(
                tr(self.locale, MessageId::SetupHotbarReturnHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ];
        if self.facts.hotbar_off_by_default {
            lines.push(Line::from(Span::styled(
                tr(self.locale, MessageId::SetupHotbarOffHint).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        lines
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
        || "unchanged; YOLO derives bypass from default_mode".to_string(),
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
        "first_run={}, update={}, constitution={:?}, autonomy={}, posture={:?}, runtime={}",
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
        state.constitution_choice,
        facts.constitution_autonomy,
        state.runtime_posture_source,
        facts.runtime_result
    )
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
        Locale::ZhHans => "用户宪法 — 批准前草案",
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
            Locale::ZhHans => "结构化宪法为空。".to_string(),
            _ => "The structured constitution is empty.".to_string(),
        });
    let layer_order = tr(locale, MessageId::SetupCheckpointLayerOrder);

    match locale {
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
                 批准\n{ratify_how}"
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
                 RATIFICATION\n{ratify_how}"
            )
        }
    }
}

/// Card line inviting the user to let their configured model draft the law.
fn model_draft_invitation_line(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::ZhHans => {
            format!("A {model_label} 起草，你批准。未经确认不会保存。")
        }
        _ => format!("A {model_label} can draft it. You ratify it. Nothing saves without you."),
    }
}

/// Card line offering to keep an existing valid constitution unchanged.
fn keep_existing_invitation_line(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "K 保留现有宪法——先查看，再保留，文件不变。",
        _ => "K Keep your existing constitution — review it, keep it, file unchanged.",
    }
}

/// Card line shown while a model draft awaits ratification.
fn model_draft_ready_line(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::ZhHans => {
            format!("{model_label} 的草案待批准——按 G 查看并批准；按 1-6 会丢弃草案。")
        }
        _ => format!(
            "Draft by {model_label} awaits ratification — G to review and ratify; 1-6 discards it."
        ),
    }
}

/// Host-facing status line after a successful model draft.
pub(crate) fn model_draft_ready_message(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::ZhHans => format!("{model_label} 已起草你的宪法。请查看预览，然后按 G 批准。"),
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
        Locale::ZhHans => {
            format!("{model_label} 未能完成起草（{reason}）。引导式草案仍然有效——按 G 预览并批准。")
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

#[must_use]
pub fn load_setup_state_for_app(app: &App, config: &Config) -> SetupState {
    if let Ok(Some(state)) = SetupState::load() {
        return state;
    }
    SetupState::derive_inherited(&inherited_facts_for_app(app, config))
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
    InheritedConfigFacts {
        language: Some(app.ui_locale.tag().to_string()),
        has_provider_route: !config.default_model().trim().is_empty(),
        has_credentials_or_local_runtime: has_api_key(config),
        trust_chosen: app.trust_mode || !onboarding::needs_trust(&app.workspace),
        has_expert_override: expert_override_path().is_some_and(|path| path.exists()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::TuiOptions;
    use crate::tui::hotbar::setup::HotbarSetupView;
    use crate::tui::views::ViewStack;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn test_app_with_config(config: &Config) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, config);
        app.ui_locale = Locale::En;
        app
    }

    fn open_hotbar_from_setup_stack(app: &App, config: &Config) -> ViewStack {
        let mut stack = ViewStack::new();
        stack.push(SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            SetupRuntimeFacts::default(),
        ));

        let events = stack.handle_key(key(KeyCode::Char('h')));

        assert_eq!(stack.top_kind(), Some(ModalKind::SetupWizard));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.as_slice(),
            [ViewEvent::SetupOpenHotbarRequested]
        ));
        stack.push(HotbarSetupView::new(app, config));
        assert_eq!(stack.top_kind(), Some(ModalKind::HotbarSetup));
        stack
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
        assert_eq!(view.selected_step(), SetupStep::Verification);

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
        assert_eq!(view.selected_step(), SetupStep::Verification);

        let action = view.handle_key(key(KeyCode::Char('r')));

        let ViewAction::Emit(ViewEvent::SetupStateCommitRequested { state, message }) = action
        else {
            panic!("expected retry setup-state commit event");
        };
        assert_eq!(
            state.status(SetupStep::Verification),
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

        let action = view.handle_key(key(KeyCode::Char('a')));

        let ViewAction::Emit(ViewEvent::SetupConstitutionModelDraftRequested { draft, locale }) =
            action
        else {
            panic!("expected model draft request event");
        };
        assert_eq!(locale, Locale::En);
        assert_eq!(draft.autonomy, AutonomyPreference::Autonomous);
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
                [palette::DEEPSEEK_INK, palette::DEEPSEEK_SLATE].contains(&buf[(w / 2, h / 2)].bg),
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
        assert_eq!(view.selected_step(), SetupStep::ToolsMcp);
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
            "config.approval_policy: never -> unchanged; YOLO derives bypass from default_mode"
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
        assert_eq!(view.selected_step(), SetupStep::ToolsMcp);
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
                .is_some_and(|result| result.contains("update=ready"))
        );
    }

    #[test]
    fn hotbar_detail_lines_show_off_status_when_no_config() {
        let facts = SetupRuntimeFacts {
            hotbar_status: "off by default".to_string(),
            hotbar_bindings_count: 0,
            hotbar_off_by_default: true,
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let text = lines_to_text(view.hotbar_detail_lines());

        assert!(text.contains("off by default"));
        assert!(text.contains("Bindings:"));
        assert!(text.contains("0"));
        assert!(text.contains("Press h to customize"));
        assert!(text.contains("Esc cancels staged changes and returns here"));
        assert!(text.contains("Hotbar is off by default"));
    }

    #[test]
    fn hotbar_detail_lines_show_disabled_status() {
        let facts = SetupRuntimeFacts {
            hotbar_status: "disabled".to_string(),
            hotbar_bindings_count: 0,
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let text = lines_to_text(view.hotbar_detail_lines());

        assert!(text.contains("disabled"));
        assert!(text.contains("Press h to customize"));
        assert!(text.contains("s saves through the Hotbar path"));
        assert!(!text.contains("Hotbar is off by default"));
    }

    #[test]
    fn hotbar_detail_lines_show_configured_status() {
        let facts = SetupRuntimeFacts {
            hotbar_status: "configured: 5 binding(s)".to_string(),
            hotbar_bindings_count: 5,
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let text = lines_to_text(view.hotbar_detail_lines());

        assert!(text.contains("configured: 5 binding(s)"));
        assert!(text.contains("Bindings:"));
        assert!(text.contains("5"));
        assert!(text.contains("Press h to customize"));
    }

    #[test]
    fn hotbar_detail_lines_show_partial_with_warnings() {
        let facts = SetupRuntimeFacts {
            hotbar_status: "partial: 3 of 8 slots, 2 warning(s)".to_string(),
            hotbar_bindings_count: 3,
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let text = lines_to_text(view.hotbar_detail_lines());

        assert!(text.contains("partial: 3 of 8 slots, 2 warning(s)"));
        assert!(text.contains("Bindings:"));
        assert!(text.contains("3"));
    }

    #[test]
    fn hotbar_detail_lines_show_unknown_status() {
        let facts = SetupRuntimeFacts {
            hotbar_status: "unknown: 1 binding(s), 1 unknown action(s), 1 warning(s)".to_string(),
            hotbar_bindings_count: 1,
            ..SetupRuntimeFacts::default()
        };
        let view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            facts,
        );

        let text = lines_to_text(view.hotbar_detail_lines());

        assert!(text.contains("unknown: 1 binding(s), 1 unknown action(s), 1 warning(s)"));
        assert!(text.contains("Bindings:"));
        assert!(text.contains("1"));
    }

    #[test]
    fn hotbar_runtime_facts_distinguish_config_states() {
        let config = Config::default();
        let app = test_app_with_config(&config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        assert_eq!(facts.hotbar_status, "off by default");
        assert_eq!(facts.hotbar_bindings_count, 0);
        assert!(facts.hotbar_off_by_default);

        let config = Config {
            hotbar: Some(Vec::new()),
            ..Config::default()
        };
        let app = test_app_with_config(&config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        assert_eq!(facts.hotbar_status, "disabled");
        assert_eq!(facts.hotbar_bindings_count, 0);
        assert!(!facts.hotbar_off_by_default);

        let config = Config {
            hotbar: Some(codewhale_config::default_hotbar_bindings_toml()),
            ..Config::default()
        };
        let app = test_app_with_config(&config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        assert_eq!(facts.hotbar_status, "configured: 8 binding(s)");
        assert_eq!(facts.hotbar_bindings_count, 8);

        let config = Config {
            hotbar: Some(vec![codewhale_config::HotbarBindingToml {
                slot: 1,
                action: "mode.agent".to_string(),
                label: None,
            }]),
            ..Config::default()
        };
        let app = test_app_with_config(&config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        assert_eq!(facts.hotbar_status, "partial: 1 of 8 slots");
        assert_eq!(facts.hotbar_bindings_count, 1);

        let config = Config {
            hotbar: Some(vec![codewhale_config::HotbarBindingToml {
                slot: 1,
                action: "unknown.custom".to_string(),
                label: None,
            }]),
            ..Config::default()
        };
        let app = test_app_with_config(&config);
        let facts = SetupRuntimeFacts::from_app_config(&app, &config);
        assert_eq!(
            facts.hotbar_status,
            "unknown: 1 binding(s), 1 unknown action(s), 1 warning(s)"
        );
        assert_eq!(facts.hotbar_bindings_count, 1);
    }

    #[test]
    fn hotbar_key_emits_setup_open_hotbar_requested() {
        let mut view = SetupWizardView::new_at_with_facts(
            SetupState::default(),
            Locale::En,
            SetupStep::Hotbar,
            SetupRuntimeFacts::default(),
        );

        let action = view.handle_key(key(KeyCode::Char('h')));

        assert!(matches!(
            action,
            ViewAction::Emit(ViewEvent::SetupOpenHotbarRequested)
        ));
    }

    #[test]
    fn hotbar_setup_cancel_returns_to_setup_without_save_event() {
        let config = Config::default();
        let app = test_app_with_config(&config);
        let mut stack = open_hotbar_from_setup_stack(&app, &config);

        assert!(stack.handle_key(key(KeyCode::Enter)).is_empty());
        let events = stack.handle_key(key(KeyCode::Esc));

        assert!(events.is_empty());
        assert_eq!(stack.top_kind(), Some(ModalKind::SetupWizard));
        assert_eq!(config.hotbar, None);
    }

    #[test]
    fn hotbar_setup_save_returns_to_setup_with_existing_save_event() {
        let config = Config::default();
        let app = test_app_with_config(&config);
        let mut stack = open_hotbar_from_setup_stack(&app, &config);

        assert!(stack.handle_key(key(KeyCode::Enter)).is_empty());
        let events = stack.handle_key(key(KeyCode::Char('s')));

        assert_eq!(stack.top_kind(), Some(ModalKind::SetupWizard));
        let [ViewEvent::HotbarSetupSaved { bindings }] = events.as_slice() else {
            panic!("expected HotbarSetupSaved event, got {events:?}");
        };
        assert!(!bindings.is_empty());
    }

    #[test]
    fn hotbar_setup_disable_returns_to_setup_with_existing_disable_event() {
        let config = Config::default();
        let app = test_app_with_config(&config);
        let mut stack = open_hotbar_from_setup_stack(&app, &config);

        let events = stack.handle_key(key(KeyCode::Char('d')));

        assert_eq!(stack.top_kind(), Some(ModalKind::SetupWizard));
        assert!(matches!(
            events.as_slice(),
            [ViewEvent::HotbarDisableRequested]
        ));
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
        assert!(text.contains("Constitution autonomy:"));
        assert!(text.contains("balanced"));
        assert!(text.contains("Runtime posture:"));
        assert!(text.contains("intent=agent, approval=suggest"));
        assert!(text.contains("Complete the constitution checkpoint"));
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
