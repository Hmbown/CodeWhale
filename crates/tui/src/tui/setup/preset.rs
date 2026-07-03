//! Runtime posture presets for the setup wizard trust/sandbox step.
//!
//! Each preset bundles a default mode, approval policy, shell toggle, and
//! sandbox mode into one selectable option with a preview-before-apply gate.

use std::path::Path;

use crate::localization::{Locale, MessageId, tr};

use super::facts::SetupRuntimeFacts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetupRuntimePreset {
    AskFirst,
    #[default]
    NormalAgent,
    HighTrustLocal,
}

impl SetupRuntimePreset {
    pub(crate) const ALL: [Self; 3] = [Self::AskFirst, Self::NormalAgent, Self::HighTrustLocal];

    pub(crate) fn from_key(key: char) -> Option<Self> {
        match key {
            '1' => Some(Self::AskFirst),
            '2' => Some(Self::NormalAgent),
            '3' => Some(Self::HighTrustLocal),
            _ => None,
        }
    }

    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::AskFirst => "ask-first",
            Self::NormalAgent => "normal-agent",
            Self::HighTrustLocal => "high-trust-local",
        }
    }

    pub(crate) fn title_id(self) -> MessageId {
        match self {
            Self::AskFirst => MessageId::SetupRuntimePresetAskFirstTitle,
            Self::NormalAgent => MessageId::SetupRuntimePresetNormalAgentTitle,
            Self::HighTrustLocal => MessageId::SetupRuntimePresetHighTrustTitle,
        }
    }

    pub(crate) fn description_id(self) -> MessageId {
        match self {
            Self::AskFirst => MessageId::SetupRuntimePresetAskFirstDescription,
            Self::NormalAgent => MessageId::SetupRuntimePresetNormalAgentDescription,
            Self::HighTrustLocal => MessageId::SetupRuntimePresetHighTrustDescription,
        }
    }

    #[must_use]
    pub fn default_mode(self) -> &'static str {
        match self {
            Self::AskFirst => "plan",
            Self::NormalAgent => "agent",
            Self::HighTrustLocal => "yolo",
        }
    }

    #[must_use]
    pub fn approval_policy(self) -> Option<&'static str> {
        match self {
            Self::AskFirst | Self::NormalAgent => Some("on-request"),
            // YOLO derives bypass approval from `default_mode = "yolo"`.
            // `approval_policy = "bypass"` is intentionally not a persisted
            // config value in v0.8.67.
            Self::HighTrustLocal => None,
        }
    }

    #[must_use]
    pub fn allow_shell(self) -> bool {
        match self {
            Self::AskFirst => false,
            Self::NormalAgent | Self::HighTrustLocal => true,
        }
    }

    #[must_use]
    pub fn sandbox_mode(self) -> &'static str {
        match self {
            Self::AskFirst => "read-only",
            Self::NormalAgent | Self::HighTrustLocal => "workspace-write",
        }
    }

    #[must_use]
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

pub(crate) fn runtime_preset_summary(locale: Locale, preset: SetupRuntimePreset) -> String {
    format!(
        "{} - {}",
        tr(locale, preset.title_id()),
        tr(locale, preset.description_id())
    )
}

pub(crate) fn runtime_preset_inline_diff(
    preset: SetupRuntimePreset,
    facts: &SetupRuntimeFacts,
) -> String {
    runtime_preset_diff_rows(preset, facts).join("; ")
}

pub(crate) fn runtime_preset_preview_text(
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

pub(crate) fn runtime_preset_diff_rows(
    preset: SetupRuntimePreset,
    facts: &SetupRuntimeFacts,
) -> Vec<String> {
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

pub(crate) fn project_runtime_override_warning(workspace: &Path, locale: Locale) -> Option<String> {
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
