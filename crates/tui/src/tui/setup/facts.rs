//! Runtime facts captured from the live `App` + `Config` at wizard open time.
//!
//! These facts power the per-step detail cards (provider/model, trust/sandbox,
//! constitution) so they reflect the current runtime without re-reading config
//! on every render.

use crate::config::{Config, has_api_key_for};
use crate::localization::Locale;
use crate::tui::app::App;

use codewhale_config::UserConstitution;

use super::constitution::SetupConstitutionFileState;
use super::guided::autonomy_label;
use super::preset::project_runtime_override_warning;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupRuntimeFacts {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) auth: String,
    pub(crate) health: String,
    pub(crate) provider_ready: bool,
    pub(crate) provider_result: String,
    pub(crate) work_intent: String,
    pub(crate) approval: String,
    pub(crate) shell: String,
    pub(crate) allow_shell_enabled: bool,
    pub(crate) trust: String,
    pub(crate) sandbox: String,
    pub(crate) sandbox_mode_value: String,
    pub(crate) network: String,
    pub(crate) network_default_value: String,
    pub(crate) runtime_result: String,
    pub(crate) default_mode: String,
    pub(crate) approval_policy_value: String,
    pub(crate) project_override_warning: Option<String>,
    pub(crate) constitution_autonomy: String,
    pub(crate) constitution_file: SetupConstitutionFileState,
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
        }
    }
}

impl SetupRuntimeFacts {
    pub(crate) fn from_app_config(app: &App, config: &Config) -> Self {
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
        }
    }
}
