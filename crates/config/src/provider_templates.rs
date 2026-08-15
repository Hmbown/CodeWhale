//! Prefab third-party provider setup templates (#5350).
//!
//! First-class providers already have a default URL and catalog. These
//! templates exist so `/provider` can offer a key-only path for:
//! - first-class gateways users still treat as "paste a Base URL" (OpenCode
//!   Zen / Go), and
//! - named OpenAI-compatible custom routes that are not ProviderKind variants
//!   (Agnes, SenseNova).
//!
//! A `/models` 2xx from Test Connection is reachability only. It is not model
//! readiness.

use crate::provider_kind::ProviderKind;
use crate::{
    DEFAULT_OPENCODE_GO_BASE_URL, DEFAULT_OPENCODE_GO_MODEL, DEFAULT_OPENCODE_ZEN_BASE_URL,
    DEFAULT_OPENCODE_ZEN_MODEL, OPENCODE_GO_CHAT_MODELS,
};

/// Agnes AI OpenAI-compatible gateway.
pub const AGNES_TEMPLATE_ID: &str = "agnes";
pub const AGNES_BASE_URL: &str = "https://apihub.agnes-ai.com/v1";
pub const AGNES_DEFAULT_MODEL: &str = "agnes-2.5-flash";
pub const AGNES_API_KEY_ENV: &str = "AGNES_API_KEY";
pub const AGNES_MODELS: &[&str] = &[AGNES_DEFAULT_MODEL, "agnes-2.0-flash", "agnes-1.5-flash"];

/// SenseTime SenseNova Token Plan (the issue's "Meituan Sensenova" target).
/// Chat models only — `sensenova-u1-fast` is image generation and is omitted.
pub const SENSENOVA_TEMPLATE_ID: &str = "sensenova";
pub const SENSENOVA_BASE_URL: &str = "https://token.sensenova.cn/v1";
pub const SENSENOVA_DEFAULT_MODEL: &str = "sensenova-6.7-flash-lite";
pub const SENSENOVA_API_KEY_ENV: &str = "SENSENOVA_API_KEY";
pub const SENSENOVA_MODELS: &[&str] = &[SENSENOVA_DEFAULT_MODEL, "deepseek-v4-flash", "glm-5.2"];

/// A built-in setup template: fixed URL + common models. The user supplies
/// only an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSetupTemplate {
    pub id: &'static str,
    pub display_name: &'static str,
    pub base_url: &'static str,
    pub default_model: &'static str,
    pub models: &'static [&'static str],
    pub api_key_env: &'static str,
    pub docs_url: Option<&'static str>,
    pub credential_url: Option<&'static str>,
    /// When set, selecting this template starts the existing first-class
    /// key-only guided setup instead of creating a custom table.
    pub first_class: Option<ProviderKind>,
}

impl ProviderSetupTemplate {
    /// Models shown in setup / `/model` when the live catalog is empty or
    /// Models.dev refresh failed.
    #[must_use]
    pub fn picker_models(self) -> Vec<&'static str> {
        match self.first_class {
            Some(ProviderKind::OpencodeZen) => crate::route::opencode_zen_picker_models(),
            Some(ProviderKind::OpencodeGo) => OPENCODE_GO_CHAT_MODELS.to_vec(),
            _ => self.models.to_vec(),
        }
    }

    #[must_use]
    pub fn is_custom(self) -> bool {
        self.first_class.is_none()
    }

    #[must_use]
    pub fn is_first_class(self) -> bool {
        self.first_class.is_some()
    }
}

const TEMPLATES: &[ProviderSetupTemplate] = &[
    ProviderSetupTemplate {
        id: "opencode-zen",
        display_name: "OpenCode Zen",
        base_url: DEFAULT_OPENCODE_ZEN_BASE_URL,
        default_model: DEFAULT_OPENCODE_ZEN_MODEL,
        models: &[DEFAULT_OPENCODE_ZEN_MODEL],
        api_key_env: "OPENCODE_ZEN_API_KEY",
        docs_url: Some("https://opencode.ai/docs/zen/"),
        credential_url: Some("https://opencode.ai/zen/"),
        first_class: Some(ProviderKind::OpencodeZen),
    },
    ProviderSetupTemplate {
        id: "opencode-go",
        display_name: "OpenCode Go",
        base_url: DEFAULT_OPENCODE_GO_BASE_URL,
        default_model: DEFAULT_OPENCODE_GO_MODEL,
        models: OPENCODE_GO_CHAT_MODELS,
        api_key_env: "OPENCODE_GO_API_KEY",
        docs_url: Some("https://opencode.ai/docs/go/"),
        credential_url: Some("https://opencode.ai/zen/"),
        first_class: Some(ProviderKind::OpencodeGo),
    },
    ProviderSetupTemplate {
        id: AGNES_TEMPLATE_ID,
        display_name: "Agnes",
        base_url: AGNES_BASE_URL,
        default_model: AGNES_DEFAULT_MODEL,
        models: AGNES_MODELS,
        api_key_env: AGNES_API_KEY_ENV,
        docs_url: Some("https://agnes-ai.com/en/docs/overview"),
        credential_url: Some("https://platform.agnes-ai.com/"),
        first_class: None,
    },
    ProviderSetupTemplate {
        id: SENSENOVA_TEMPLATE_ID,
        display_name: "Meituan Sensenova",
        base_url: SENSENOVA_BASE_URL,
        default_model: SENSENOVA_DEFAULT_MODEL,
        models: SENSENOVA_MODELS,
        api_key_env: SENSENOVA_API_KEY_ENV,
        docs_url: Some("https://platform.sensenova.cn/token-plan"),
        credential_url: Some("https://platform.sensenova.cn/token-plan"),
        first_class: None,
    },
];

/// Every built-in setup template, first-class then custom.
#[must_use]
pub fn provider_setup_templates() -> &'static [ProviderSetupTemplate] {
    TEMPLATES
}

/// Look up a template by id or a documented alias.
#[must_use]
pub fn provider_setup_template(id: &str) -> Option<&'static ProviderSetupTemplate> {
    let needle = id.trim().to_ascii_lowercase().replace('_', "-");
    TEMPLATES.iter().find(|template| {
        template.id == needle
            || template
                .first_class
                .is_some_and(|kind| kind.as_str() == needle)
            || match needle.as_str() {
                "zen" | "opencodezen" => template.id == "opencode-zen",
                "opencodego" => template.id == "opencode-go",
                "sense-nova" | "meituan-sensenova" | "meituan-sensenova-cn" => {
                    template.id == SENSENOVA_TEMPLATE_ID
                }
                _ => false,
            }
    })
}

/// Templates that persist as named `[providers.<id>] kind = "openai-compatible"`
/// tables rather than a first-class ProviderKind.
#[must_use]
pub fn custom_provider_setup_templates() -> impl Iterator<Item = &'static ProviderSetupTemplate> {
    TEMPLATES.iter().filter(|template| template.is_custom())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_have_fixed_urls_and_models() {
        for template in provider_setup_templates() {
            assert!(
                template.base_url.starts_with("https://"),
                "{} base URL must be https: {}",
                template.id,
                template.base_url
            );
            assert!(
                !template.picker_models().is_empty(),
                "{} must list at least one model",
                template.id
            );
            assert!(
                template
                    .picker_models()
                    .iter()
                    .any(|model| *model == template.default_model),
                "{} default {} missing from picker models {:?}",
                template.id,
                template.default_model,
                template.picker_models()
            );
            assert!(
                !template.api_key_env.is_empty(),
                "{} must name an API key env",
                template.id
            );
        }
    }

    #[test]
    fn custom_template_ids_do_not_shadow_built_ins() {
        for template in custom_provider_setup_templates() {
            assert!(
                ProviderKind::parse(template.id).is_none(),
                "custom template '{}' shadows ProviderKind",
                template.id
            );
        }
    }

    #[test]
    fn first_class_templates_map_to_existing_kinds() {
        let zen = provider_setup_template("opencode-zen").expect("zen");
        assert_eq!(zen.first_class, Some(ProviderKind::OpencodeZen));
        assert_eq!(zen.base_url, DEFAULT_OPENCODE_ZEN_BASE_URL);
        assert!(zen.picker_models().len() > 1);

        let go = provider_setup_template("opencode-go").expect("go");
        assert_eq!(go.first_class, Some(ProviderKind::OpencodeGo));
        assert_eq!(go.base_url, DEFAULT_OPENCODE_GO_BASE_URL);
        assert_eq!(go.picker_models(), OPENCODE_GO_CHAT_MODELS);

        let agnes = provider_setup_template("agnes").expect("agnes");
        assert!(agnes.is_custom());
        assert_eq!(agnes.base_url, AGNES_BASE_URL);

        let sense = provider_setup_template("meituan-sensenova").expect("sensenova alias");
        assert_eq!(sense.id, SENSENOVA_TEMPLATE_ID);
        assert_eq!(sense.base_url, SENSENOVA_BASE_URL);
        assert!(!sense.models.contains(&"sensenova-u1-fast"));
    }
}
