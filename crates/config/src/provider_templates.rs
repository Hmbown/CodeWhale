//! Beginner provider setup templates (#5350).
//!
//! First-class providers already have a default URL and catalog. These
//! templates exist so `/provider` and Settings can offer a key-only path
//! for:
//! - first-class gateways users still treat as "paste a Base URL"
//!   (OpenCode Zen / Go), and
//! - named OpenAI-compatible custom routes that are not `ProviderKind`
//!   variants (SenseNova, Baseten, Groq, Cerebras, Command Code). Hosted
//!   Chat Completions backends are descriptor rows — not new enum variants
//!   and not compiled model rosters. Live `GET /v1/models` and the
//!   Codewhale catalog are the offering list. Distinct *wires*
//!   (Anthropic Messages, Codex Responses, Google thought signatures) stay
//!   on `ProviderKind`.
//!
//! Values here are limited to hosts, models, and env names already
//! documented in this repository. Agnes is catalogued as unpublished so
//! the UI can say so without inventing a URL.
//!
//! A `/models` 2xx from Test Connection is reachability only. It is not
//! model readiness.

use crate::OPENCODE_GO_CHAT_MODELS;
use crate::provider::{credential_help, provider_for_kind};
use crate::provider_kind::ProviderKind;

/// SenseTime SenseNova OpenAI-compatible host already shipped on this
/// branch as the `/provider` `S` preset.
pub const SENSENOVA_TEMPLATE_ID: &str = "sensenova";
pub const SENSENOVA_BASE_URL: &str = "https://token.sensenova.cn/v1";
pub const SENSENOVA_DEFAULT_MODEL: &str = "deepseek-v4-flash";
pub const SENSENOVA_API_KEY_ENV: &str = "SENSENOVA_API_KEY";

/// Agnes is requested by #5350 but has no published OpenAI-compatible
/// host in this repository.
pub const AGNES_TEMPLATE_ID: &str = "agnes";

/// Baseten Model APIs — OpenAI Chat Completions, discovered at `/v1/models`.
pub const BASETEN_TEMPLATE_ID: &str = "baseten";
pub const BASETEN_BASE_URL: &str = "https://inference.baseten.co/v1";
pub const BASETEN_DEFAULT_MODEL: &str = "deepseek-ai/DeepSeek-V4-Pro";
pub const BASETEN_API_KEY_ENV: &str = "BASETEN_API_KEY";

/// Groq — OpenAI Chat Completions hosted inference.
pub const GROQ_TEMPLATE_ID: &str = "groq";
pub const GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
pub const GROQ_DEFAULT_MODEL: &str = "llama-3.3-70b-versatile";
pub const GROQ_API_KEY_ENV: &str = "GROQ_API_KEY";

/// Cerebras — OpenAI Chat Completions hosted inference.
pub const CEREBRAS_TEMPLATE_ID: &str = "cerebras";
pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";
pub const CEREBRAS_DEFAULT_MODEL: &str = "llama-3.3-70b";
pub const CEREBRAS_API_KEY_ENV: &str = "CEREBRAS_API_KEY";
pub const COMMAND_CODE_TEMPLATE_ID: &str = "command-code";

/// How a beginner template is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderSetupApply {
    /// Existing first-class `ProviderKind`. URL and models stay on the
    /// registry; the template only names the key-only setup path.
    FirstClass(ProviderKind),
    /// Named `[providers.<id>] kind = "openai-compatible"` table with a
    /// published host already recorded in this repository.
    Compatible,
    /// Catalog row with no published URL or model list. The UI must not
    /// invent one.
    Unpublished,
}

/// A built-in setup template. Compatible rows carry a fixed URL and a
/// proven model list; first-class rows delegate those facts to the
/// existing provider registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSetupTemplate {
    pub id: &'static str,
    pub display_name: &'static str,
    pub apply: ProviderSetupApply,
    base_url: Option<&'static str>,
    default_model: Option<&'static str>,
    api_key_env: Option<&'static str>,
    docs_url: Option<&'static str>,
    credential_url: Option<&'static str>,
    guidance: &'static str,
}

impl ProviderSetupTemplate {
    /// Published OpenAI-compatible host, when this repository has one.
    #[must_use]
    pub fn base_url(self) -> Option<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(kind) => {
                Some(provider_for_kind(kind).default_base_url())
            }
            ProviderSetupApply::Compatible => self.base_url,
            ProviderSetupApply::Unpublished => None,
        }
    }

    /// Default model, when this repository has one.
    #[must_use]
    pub fn default_model(self) -> Option<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(kind) => Some(provider_for_kind(kind).default_model()),
            ProviderSetupApply::Compatible => self.default_model,
            ProviderSetupApply::Unpublished => None,
        }
    }

    /// Bootstrap model ids when the live catalog has not yet answered.
    /// Compatible hosts expose only the descriptor default — never a compiled roster.
    #[must_use]
    pub fn picker_models(self) -> Vec<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(ProviderKind::OpencodeZen) => {
                crate::route::opencode_zen_picker_models()
            }
            ProviderSetupApply::FirstClass(ProviderKind::OpencodeGo) => {
                OPENCODE_GO_CHAT_MODELS.to_vec()
            }
            ProviderSetupApply::FirstClass(_) | ProviderSetupApply::Compatible => self
                .default_model()
                .map(|model| vec![model])
                .unwrap_or_default(),
            ProviderSetupApply::Unpublished => Vec::new(),
        }
    }

    /// Canonical API-key environment variable name, when known.
    #[must_use]
    pub fn api_key_env(self) -> Option<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(kind) => {
                provider_for_kind(kind).env_vars().first().copied()
            }
            ProviderSetupApply::Compatible => self.api_key_env,
            ProviderSetupApply::Unpublished => None,
        }
    }

    /// Provider-owned documentation URL already recorded in this repository.
    #[must_use]
    pub fn docs_url(self) -> Option<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(kind) => credential_help(kind).docs_url,
            _ => self.docs_url,
        }
    }

    /// Provider-owned credential page already recorded in this repository.
    #[must_use]
    pub fn credential_url(self) -> Option<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(kind) => credential_help(kind).credential_url,
            _ => self.credential_url,
        }
    }

    /// Concise, non-secret setup guidance. First-class rows reuse the
    /// existing credential-help sentence.
    #[must_use]
    pub fn guidance(self) -> &'static str {
        match self.apply {
            ProviderSetupApply::FirstClass(kind) => credential_help(kind).guidance,
            _ => self.guidance,
        }
    }

    #[must_use]
    pub fn is_first_class(self) -> bool {
        matches!(self.apply, ProviderSetupApply::FirstClass(_))
    }

    #[must_use]
    pub fn is_compatible(self) -> bool {
        matches!(self.apply, ProviderSetupApply::Compatible)
    }

    #[must_use]
    pub fn is_unpublished(self) -> bool {
        matches!(self.apply, ProviderSetupApply::Unpublished)
    }

    /// Compact Settings-row value: fillable ids, then unpublished ids.
    #[must_use]
    pub fn settings_value() -> String {
        let mut fillable = Vec::new();
        let mut unpublished = Vec::new();
        for template in provider_setup_templates() {
            if template.is_unpublished() {
                unpublished.push(template.id);
            } else {
                fillable.push(template.id);
            }
        }
        match (fillable.is_empty(), unpublished.is_empty()) {
            (true, true) => String::new(),
            (false, true) => fillable.join(", "),
            (true, false) => format!("{} unpublished", unpublished.join(", ")),
            (false, false) => format!(
                "{}; {} unpublished",
                fillable.join(", "),
                unpublished.join(", ")
            ),
        }
    }
}

const TEMPLATES: &[ProviderSetupTemplate] = &[
    ProviderSetupTemplate {
        id: "opencode-zen",
        display_name: "OpenCode Zen",
        apply: ProviderSetupApply::FirstClass(ProviderKind::OpencodeZen),
        base_url: None,
        default_model: None,
        api_key_env: None,
        docs_url: None,
        credential_url: None,
        guidance: "",
    },
    ProviderSetupTemplate {
        id: "opencode-go",
        display_name: "OpenCode Go",
        apply: ProviderSetupApply::FirstClass(ProviderKind::OpencodeGo),
        base_url: None,
        default_model: None,
        api_key_env: None,
        docs_url: None,
        credential_url: None,
        guidance: "",
    },
    ProviderSetupTemplate {
        id: SENSENOVA_TEMPLATE_ID,
        display_name: "SenseNova",
        apply: ProviderSetupApply::Compatible,
        base_url: Some(SENSENOVA_BASE_URL),
        default_model: Some(SENSENOVA_DEFAULT_MODEL),
        api_key_env: Some(SENSENOVA_API_KEY_ENV),
        docs_url: None,
        credential_url: None,
        guidance: "OpenAI-compatible SenseTime SenseNova host. Store an env var name, not a raw key.",
    },
    ProviderSetupTemplate {
        id: BASETEN_TEMPLATE_ID,
        display_name: "Baseten",
        apply: ProviderSetupApply::Compatible,
        base_url: Some(BASETEN_BASE_URL),
        default_model: Some(BASETEN_DEFAULT_MODEL),
        api_key_env: Some(BASETEN_API_KEY_ENV),
        docs_url: Some("https://docs.baseten.co/inference/model-apis/overview"),
        credential_url: Some("https://app.baseten.co/settings/api_keys"),
        guidance: "Baseten Model APIs. OpenAI Chat Completions at inference.baseten.co. Store BASETEN_API_KEY, not a raw key.",
    },
    ProviderSetupTemplate {
        id: GROQ_TEMPLATE_ID,
        display_name: "Groq",
        apply: ProviderSetupApply::Compatible,
        base_url: Some(GROQ_BASE_URL),
        default_model: Some(GROQ_DEFAULT_MODEL),
        api_key_env: Some(GROQ_API_KEY_ENV),
        docs_url: Some("https://console.groq.com/docs/quickstart"),
        credential_url: Some("https://console.groq.com/keys"),
        guidance: "Groq hosted inference. OpenAI Chat Completions. Store GROQ_API_KEY, not a raw key.",
    },
    ProviderSetupTemplate {
        id: CEREBRAS_TEMPLATE_ID,
        display_name: "Cerebras",
        apply: ProviderSetupApply::Compatible,
        base_url: Some(CEREBRAS_BASE_URL),
        default_model: Some(CEREBRAS_DEFAULT_MODEL),
        api_key_env: Some(CEREBRAS_API_KEY_ENV),
        docs_url: Some("https://inference-docs.cerebras.ai/quickstart"),
        credential_url: Some("https://cloud.cerebras.ai"),
        guidance: "Cerebras hosted inference. OpenAI Chat Completions. Store CEREBRAS_API_KEY, not a raw key.",
    },
    ProviderSetupTemplate {
        id: COMMAND_CODE_TEMPLATE_ID,
        display_name: "Command Code",
        apply: ProviderSetupApply::Compatible,
        base_url: Some("https://api.commandcode.ai/provider/v1"),
        default_model: Some("deepseek/deepseek-v4-flash"),
        api_key_env: Some("COMMAND_CODE_API_KEY"),
        docs_url: Some("https://commandcode.ai/docs/provider"),
        credential_url: Some("https://commandcode.ai/provider"),
        guidance: "Published Provider API. Live GET /v1/models is the roster. Do not import the Command Code CLI login. Store COMMAND_CODE_API_KEY, not a raw key.",
    },
    ProviderSetupTemplate {
        id: AGNES_TEMPLATE_ID,
        display_name: "Agnes",
        apply: ProviderSetupApply::Unpublished,
        base_url: None,
        default_model: None,
        api_key_env: None,
        docs_url: None,
        credential_url: None,
        guidance: "Agnes has no published OpenAI-compatible URL in this repository, so it has no fillable preset.",
    },
];

/// Every built-in setup template, first-class then compatible then unpublished.
#[must_use]
pub fn provider_setup_templates() -> &'static [ProviderSetupTemplate] {
    TEMPLATES
}

/// Templates that persist as named OpenAI-compatible tables.
pub fn compatible_provider_setup_templates() -> impl Iterator<Item = &'static ProviderSetupTemplate>
{
    TEMPLATES.iter().filter(|template| template.is_compatible())
}

/// Look up a template by id, documented alias, or first-class provider id.
#[must_use]
pub fn provider_setup_template(id: &str) -> Option<&'static ProviderSetupTemplate> {
    let needle = id.trim().to_ascii_lowercase().replace('_', "-");
    if needle.is_empty() {
        return None;
    }
    TEMPLATES
        .iter()
        .find(|template| template.id == needle)
        .or_else(|| {
            TEMPLATES.iter().find(|template| match needle.as_str() {
                "zen" | "opencodezen" => template.id == "opencode-zen",
                "opencodego" => template.id == "opencode-go",
                "sense-nova" | "meituan-sensenova" | "meituan-sensenova-cn" => {
                    template.id == SENSENOVA_TEMPLATE_ID
                }
                "base-ten" | "base_ten" => template.id == BASETEN_TEMPLATE_ID,
                "commandcode" | "cmd-code" => template.id == COMMAND_CODE_TEMPLATE_ID,
                _ => false,
            })
        })
        .or_else(|| {
            let kind = ProviderKind::parse(&needle)?;
            TEMPLATES.iter().find(|template| {
            matches!(template.apply, ProviderSetupApply::FirstClass(candidate) if candidate == kind)
        })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_OPENCODE_GO_BASE_URL, DEFAULT_OPENCODE_ZEN_BASE_URL};

    #[test]
    fn fillable_templates_have_https_hosts_and_models() {
        for template in provider_setup_templates() {
            if template.is_unpublished() {
                assert!(template.base_url().is_none(), "{}", template.id);
                assert!(template.picker_models().is_empty(), "{}", template.id);
                assert!(template.api_key_env().is_none(), "{}", template.id);
                continue;
            }
            let base_url = template
                .base_url()
                .unwrap_or_else(|| panic!("{} host", template.id));
            assert!(
                base_url.starts_with("https://"),
                "{} base URL must be https: {base_url}",
                template.id
            );
            let models = template.picker_models();
            assert!(
                !models.is_empty(),
                "{} must list at least one model",
                template.id
            );
            let default_model = template
                .default_model()
                .unwrap_or_else(|| panic!("{} default model", template.id));
            assert!(
                models
                    .iter()
                    .any(|model| model.eq_ignore_ascii_case(default_model)),
                "{} default {default_model} missing from picker models {models:?}",
                template.id
            );
            assert!(
                template.api_key_env().is_some_and(|env| !env.is_empty()),
                "{} must name an API key env",
                template.id
            );
        }
    }

    #[test]
    fn compatible_template_ids_do_not_shadow_built_ins() {
        for template in compatible_provider_setup_templates() {
            assert!(
                ProviderKind::parse(template.id).is_none(),
                "compatible template '{}' shadows ProviderKind",
                template.id
            );
        }
    }

    #[test]
    fn first_class_templates_reuse_registry_facts() {
        let zen = provider_setup_template("opencode-zen").expect("zen");
        assert_eq!(
            zen.apply,
            ProviderSetupApply::FirstClass(ProviderKind::OpencodeZen)
        );
        assert_eq!(zen.base_url(), Some(DEFAULT_OPENCODE_ZEN_BASE_URL));
        assert_eq!(zen.api_key_env(), Some("OPENCODE_ZEN_API_KEY"));
        assert_eq!(zen.docs_url(), Some("https://opencode.ai/docs/zen/"));
        assert_eq!(zen.credential_url(), Some("https://opencode.ai/zen/"));
        assert!(zen.picker_models().len() > 1);
        assert!(zen.picker_models().contains(&"deepseek-v4-flash"));

        let go = provider_setup_template("opencode-go").expect("go");
        assert_eq!(
            go.apply,
            ProviderSetupApply::FirstClass(ProviderKind::OpencodeGo)
        );
        assert_eq!(go.base_url(), Some(DEFAULT_OPENCODE_GO_BASE_URL));
        assert_eq!(go.picker_models(), OPENCODE_GO_CHAT_MODELS);
        assert_eq!(go.docs_url(), Some("https://opencode.ai/docs/go/"));
    }

    #[test]
    fn sensenova_uses_the_published_host_already_on_this_branch() {
        let sense = provider_setup_template("meituan-sensenova").expect("sensenova alias");
        assert_eq!(sense.id, SENSENOVA_TEMPLATE_ID);
        assert!(sense.is_compatible());
        assert_eq!(sense.base_url(), Some(SENSENOVA_BASE_URL));
        assert_eq!(sense.default_model(), Some(SENSENOVA_DEFAULT_MODEL));
        assert_eq!(sense.api_key_env(), Some(SENSENOVA_API_KEY_ENV));
        assert_eq!(sense.picker_models(), vec![SENSENOVA_DEFAULT_MODEL]);
        assert!(sense.docs_url().is_none());
        assert!(sense.credential_url().is_none());
    }

    #[test]
    fn agnes_stays_unpublished_without_invented_values() {
        let agnes = provider_setup_template("agnes").expect("agnes");
        assert!(agnes.is_unpublished());
        assert!(agnes.base_url().is_none());
        assert!(agnes.default_model().is_none());
        assert!(agnes.picker_models().is_empty());
        assert!(agnes.api_key_env().is_none());
        assert!(agnes.docs_url().is_none());
        assert!(agnes.guidance().contains("no published"));
    }

    #[test]
    fn settings_value_names_fillable_then_unpublished() {
        assert_eq!(
            ProviderSetupTemplate::settings_value(),
            "opencode-zen, opencode-go, sensenova, baseten, groq, cerebras, command-code; agnes unpublished"
        );
    }

    #[test]
    fn zen_alias_from_provider_kind_parse_resolves() {
        assert_eq!(
            provider_setup_template("zen").map(|template| template.id),
            Some("opencode-zen")
        );
        assert_eq!(
            provider_setup_template("OPENCODE_ZEN").map(|template| template.id),
            Some("opencode-zen")
        );
    }

    #[test]
    fn hosted_openai_compat_hosts_are_templates_not_enum_variants() {
        for (alias, id, url, env) in [
            (
                "baseten",
                BASETEN_TEMPLATE_ID,
                BASETEN_BASE_URL,
                BASETEN_API_KEY_ENV,
            ),
            (
                "base-ten",
                BASETEN_TEMPLATE_ID,
                BASETEN_BASE_URL,
                BASETEN_API_KEY_ENV,
            ),
            ("groq", GROQ_TEMPLATE_ID, GROQ_BASE_URL, GROQ_API_KEY_ENV),
            (
                "cerebras",
                CEREBRAS_TEMPLATE_ID,
                CEREBRAS_BASE_URL,
                CEREBRAS_API_KEY_ENV,
            ),
            (
                "command-code",
                COMMAND_CODE_TEMPLATE_ID,
                "https://api.commandcode.ai/provider/v1",
                "COMMAND_CODE_API_KEY",
            ),
        ] {
            let template = provider_setup_template(alias).unwrap_or_else(|| panic!("{alias}"));
            assert_eq!(template.id, id);
            assert!(template.is_compatible(), "{alias}");
            assert_eq!(template.base_url(), Some(url));
            assert_eq!(template.api_key_env(), Some(env));
            assert!(
                ProviderKind::parse(id).is_none(),
                "{id} must not be a ProviderKind"
            );
        }
    }
}
