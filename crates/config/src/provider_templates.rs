//! Beginner provider setup templates (#5350).
//!
//! First-class providers already have a default URL and catalog. These
//! templates exist so `/provider` and Settings can offer a key-only path
//! for:
//! - first-class gateways users still treat as "paste a Base URL"
//!   (OpenCode Zen / Go), and
//! - named OpenAI-compatible custom routes that are not `ProviderKind`
//!   variants (SenseNova, LM Studio, DS4).
//!
//! Values here are limited to hosts, models, and env names already
//! documented in this repository. Agnes is catalogued as unpublished so
//! the UI can say so without inventing a URL. Keyless loopback rows
//! (LM Studio, DS4) are reached from the `P` template list, not from
//! list-stage letter keys — those letters belong to a-z type-ahead.
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
pub const SENSENOVA_MODELS: &[&str] = &[SENSENOVA_DEFAULT_MODEL];

/// Agnes is requested by #5350 but has no published OpenAI-compatible
/// host in this repository.
pub const AGNES_TEMPLATE_ID: &str = "agnes";

/// Local LM Studio OpenAI-compatible loopback. Persisted custom-table
/// id stays `lm_studio` so existing `[providers.lm_studio]` rows keep
/// matching; the template catalog id is kebab-case like the others.
pub const LM_STUDIO_TEMPLATE_ID: &str = "lm-studio";
pub const LM_STUDIO_PROVIDER_ID: &str = "lm_studio";
pub const LM_STUDIO_BASE_URL: &str = "http://127.0.0.1:1234/v1";

/// Local DS4 (DwarfStar) OpenAI-compatible loopback. Same transport as
/// any custom OpenAI-compatible row; no API key.
pub const DS4_TEMPLATE_ID: &str = "ds4";
pub const DS4_PROVIDER_ID: &str = "ds4";
pub const DS4_BASE_URL: &str = "http://127.0.0.1:8000/v1";
pub const DS4_DEFAULT_MODEL: &str = "deepseek-v4-flash";
pub const DS4_MODELS: &[&str] = &[DS4_DEFAULT_MODEL];

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
    models: &'static [&'static str],
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

    /// Models shown in setup / `/model` when the live catalog is empty or
    /// Models.dev refresh failed. First-class Zen/Go use the curated
    /// roster already owned by the route catalog.
    #[must_use]
    pub fn picker_models(self) -> Vec<&'static str> {
        match self.apply {
            ProviderSetupApply::FirstClass(ProviderKind::OpencodeZen) => {
                crate::route::opencode_zen_picker_models()
            }
            ProviderSetupApply::FirstClass(ProviderKind::OpencodeGo) => {
                OPENCODE_GO_CHAT_MODELS.to_vec()
            }
            ProviderSetupApply::FirstClass(_) => self
                .default_model()
                .map(|model| vec![model])
                .unwrap_or_default(),
            ProviderSetupApply::Compatible => self.models.to_vec(),
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

    /// Keyless `http://127.0.0.1` OpenAI-compatible loopback. Not a
    /// catalog filter and not an API-key template.
    #[must_use]
    pub fn is_keyless_local(self) -> bool {
        self.is_compatible()
            && self.api_key_env().is_none()
            && self
                .base_url()
                .is_some_and(|url| url.starts_with("http://127.0.0.1:"))
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
        models: &[],
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
        models: &[],
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
        models: SENSENOVA_MODELS,
        api_key_env: Some(SENSENOVA_API_KEY_ENV),
        docs_url: None,
        credential_url: None,
        guidance: "OpenAI-compatible SenseTime SenseNova host. Store an env var name, not a raw key.",
    },
    ProviderSetupTemplate {
        id: LM_STUDIO_TEMPLATE_ID,
        display_name: "LM Studio",
        apply: ProviderSetupApply::Compatible,
        base_url: Some(LM_STUDIO_BASE_URL),
        default_model: None,
        models: &[],
        api_key_env: None,
        docs_url: None,
        credential_url: None,
        guidance: "Keyless local LM Studio loopback. Enter the model currently loaded in LM Studio. No API key.",
    },
    ProviderSetupTemplate {
        id: DS4_TEMPLATE_ID,
        display_name: "DS4",
        apply: ProviderSetupApply::Compatible,
        base_url: Some(DS4_BASE_URL),
        default_model: Some(DS4_DEFAULT_MODEL),
        models: DS4_MODELS,
        api_key_env: None,
        docs_url: None,
        credential_url: None,
        guidance: "Keyless local DS4 (DwarfStar) loopback. Start ds4-server first. No API key.",
    },
    ProviderSetupTemplate {
        id: AGNES_TEMPLATE_ID,
        display_name: "Agnes",
        apply: ProviderSetupApply::Unpublished,
        base_url: None,
        default_model: None,
        models: &[],
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
                "lmstudio" => template.id == LM_STUDIO_TEMPLATE_ID,
                "dwarfstar" => template.id == DS4_TEMPLATE_ID,
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
            if template.is_keyless_local() {
                let base_url = template
                    .base_url()
                    .unwrap_or_else(|| panic!("{} host", template.id));
                assert!(
                    base_url.starts_with("http://127.0.0.1:"),
                    "{} keyless local URL must be loopback: {base_url}",
                    template.id
                );
                assert!(
                    template.api_key_env().is_none(),
                    "{} must not invent an API key env",
                    template.id
                );
                if let Some(default_model) = template.default_model() {
                    assert!(
                        template
                            .picker_models()
                            .iter()
                            .any(|model| model.eq_ignore_ascii_case(default_model)),
                        "{} default {default_model} missing from picker models",
                        template.id
                    );
                }
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
        assert_eq!(sense.picker_models(), SENSENOVA_MODELS);
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
            "opencode-zen, opencode-go, sensenova, lm-studio, ds4; agnes unpublished"
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
    fn keyless_local_templates_use_loopback_and_skip_api_key_env() {
        let lm = provider_setup_template("lm_studio").expect("lm studio alias");
        assert_eq!(lm.id, LM_STUDIO_TEMPLATE_ID);
        assert!(lm.is_keyless_local());
        assert_eq!(lm.base_url(), Some(LM_STUDIO_BASE_URL));
        assert!(lm.api_key_env().is_none());
        assert!(lm.default_model().is_none());
        assert!(lm.picker_models().is_empty());

        let ds4 = provider_setup_template("dwarfstar").expect("ds4 alias");
        assert_eq!(ds4.id, DS4_TEMPLATE_ID);
        assert!(ds4.is_keyless_local());
        assert_eq!(ds4.base_url(), Some(DS4_BASE_URL));
        assert_eq!(ds4.default_model(), Some(DS4_DEFAULT_MODEL));
        assert_eq!(ds4.picker_models(), DS4_MODELS);
        assert!(ds4.api_key_env().is_none());
    }
}
