//! Built-in provider metadata.
//!
//! This module is a metadata foundation for collapsing provider drift over
//! time. It deliberately does not mutate request bodies or choose fallback
//! providers; runtime routing remains in `ConfigToml::resolve_runtime_options`.

use super::{
    DEFAULT_ARCEE_BASE_URL, DEFAULT_ARCEE_MODEL, DEFAULT_ATLASCLOUD_BASE_URL,
    DEFAULT_ATLASCLOUD_MODEL, DEFAULT_DEEPINFRA_BASE_URL, DEFAULT_DEEPINFRA_MODEL,
    DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL, DEFAULT_DEEPSEEK_ANTHROPIC_MODEL,
    DEFAULT_DEEPSEEK_BASE_URL, DEFAULT_DEEPSEEK_MODEL, DEFAULT_FIREWORKS_BASE_URL,
    DEFAULT_FIREWORKS_MODEL, DEFAULT_HUGGINGFACE_BASE_URL, DEFAULT_HUGGINGFACE_MODEL,
    DEFAULT_LONGCAT_BASE_URL, DEFAULT_LONGCAT_MODEL, DEFAULT_META_BASE_URL, DEFAULT_META_MODEL,
    DEFAULT_MINIMAX_ANTHROPIC_BASE_URL, DEFAULT_MINIMAX_BASE_URL, DEFAULT_MINIMAX_MODEL,
    DEFAULT_MOONSHOT_BASE_URL, DEFAULT_MOONSHOT_MODEL, DEFAULT_NOVITA_BASE_URL,
    DEFAULT_NOVITA_MODEL, DEFAULT_NVIDIA_NIM_BASE_URL, DEFAULT_NVIDIA_NIM_MODEL,
    DEFAULT_OLLAMA_BASE_URL, DEFAULT_OLLAMA_MODEL, DEFAULT_OPENAI_BASE_URL,
    DEFAULT_OPENAI_CODEX_BASE_URL, DEFAULT_OPENAI_CODEX_MODEL, DEFAULT_OPENAI_MODEL,
    DEFAULT_OPENCODE_GO_BASE_URL, DEFAULT_OPENCODE_GO_MODEL, DEFAULT_OPENMODEL_BASE_URL,
    DEFAULT_OPENMODEL_MODEL, DEFAULT_OPENROUTER_BASE_URL, DEFAULT_OPENROUTER_MODEL,
    DEFAULT_QIANFAN_BASE_URL, DEFAULT_QIANFAN_MODEL, DEFAULT_SAKANA_BASE_URL, DEFAULT_SAKANA_MODEL,
    DEFAULT_SGLANG_BASE_URL, DEFAULT_SGLANG_MODEL, DEFAULT_SILICONFLOW_BASE_URL,
    DEFAULT_SILICONFLOW_CN_BASE_URL, DEFAULT_SILICONFLOW_MODEL, DEFAULT_STEPFUN_BASE_URL,
    DEFAULT_STEPFUN_MODEL, DEFAULT_TOGETHER_BASE_URL, DEFAULT_TOGETHER_MODEL,
    DEFAULT_VLLM_BASE_URL, DEFAULT_VLLM_MODEL, DEFAULT_VOLCENGINE_BASE_URL,
    DEFAULT_VOLCENGINE_MODEL, DEFAULT_WANJIE_ARK_BASE_URL, DEFAULT_WANJIE_ARK_MODEL,
    DEFAULT_XAI_BASE_URL, DEFAULT_XAI_MODEL, DEFAULT_XIAOMI_MIMO_BASE_URL,
    DEFAULT_XIAOMI_MIMO_MODEL, DEFAULT_ZAI_BASE_URL, DEFAULT_ZAI_MODEL, ProviderKind,
};

/// Wire protocol spoken by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireFormat {
    /// OpenAI-compatible `/v1/chat/completions` style payloads.
    ChatCompletions,
    /// OpenAI Responses API (`/responses`).
    Responses,
    /// Native Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
}

/// How a user obtains or supplies credentials for a built-in provider.
///
/// Keeping this typed prevents API-key onboarding from accidentally describing
/// a local runtime, OAuth-only route, or user-defined endpoint as though it had
/// a vendor key console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialAcquisition {
    /// A provider-issued API key or access token.
    ApiKey,
    /// Either a provider-issued API key or the provider's supported OAuth path.
    ApiKeyOrOAuth,
    /// A self-hosted route that is keyless by default but can be configured with auth.
    LocalOptional,
    /// An OAuth-only route; Codewhale does not collect an API key for it.
    OAuth,
    /// A user-defined route whose credential source belongs in configuration.
    Configuration,
}

impl CredentialAcquisition {
    /// Stable machine-readable label for diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::ApiKeyOrOAuth => "api_key_or_oauth",
            Self::LocalOptional => "local_optional",
            Self::OAuth => "oauth",
            Self::Configuration => "configuration",
        }
    }
}

/// Canonical, non-secret help for configuring one provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialHelp {
    pub acquisition: CredentialAcquisition,
    /// Stable provider-owned page for creating or locating credentials.
    ///
    /// `None` is deliberate for local, OAuth-only, and user-defined routes; UI
    /// callers must show [`Self::guidance`] instead of guessing a URL.
    pub credential_url: Option<&'static str>,
    /// Provider-owned documentation when the repository already has a stable link.
    pub docs_url: Option<&'static str>,
    /// Concise fallback or qualification for non-key and mixed-auth routes.
    pub guidance: &'static str,
}

/// Kimi Code's membership-plan key console.
///
/// This is intentionally distinct from Moonshot's direct API console.  The
/// route-specific helper below owns the choice so a configured Kimi Code route
/// is never described as a generic Moonshot route.
pub const KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL: &str = "https://www.kimi.com/code/console";

/// Static metadata for a built-in model provider.
pub trait Provider: Send + Sync {
    /// Provider enum variant represented by this entry.
    fn kind(&self) -> ProviderKind;

    /// Canonical provider identifier.
    fn id(&self) -> &'static str {
        self.kind().as_str()
    }

    /// Human-readable provider label for UIs and diagnostics.
    fn display_name(&self) -> &'static str;

    /// Default base URL used when no config/env/CLI override is present.
    fn default_base_url(&self) -> &'static str;

    /// Default model used when no config/env/CLI override is present.
    fn default_model(&self) -> &'static str;

    /// Environment variable candidates used for this provider's API key.
    fn env_vars(&self) -> &'static [&'static str];

    /// TOML table key under `[providers.<key>]`.
    fn provider_config_key(&self) -> &'static str;

    /// Alternate names accepted during provider resolution.
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }

    /// Wire format used by the provider.
    fn wire(&self) -> WireFormat {
        WireFormat::ChatCompletions
    }

    /// Credential acquisition metadata shared by onboarding, setup, diagnostics,
    /// and provider-help surfaces.
    fn credential_help(&self) -> CredentialHelp {
        credential_help(self.kind())
    }
}

/// Return the canonical credential-acquisition metadata for a provider kind.
///
/// URLs here are provider-owned links already documented in this repository.
/// If no stable vendor credential page is known, the URL remains absent and the
/// guidance explains the supported local, OAuth, or configuration path.
/// This is provider-level fallback metadata: callers that know a concrete base
/// URL must use [`credential_help_for_route`] so route-owned credentials do not
/// inherit a default endpoint's console.
#[must_use]
pub const fn credential_help(kind: ProviderKind) -> CredentialHelp {
    use CredentialAcquisition::{ApiKey, ApiKeyOrOAuth, Configuration, LocalOptional, OAuth};

    match kind {
        ProviderKind::Deepseek | ProviderKind::DeepseekAnthropic => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://platform.deepseek.com/api_keys"),
            docs_url: Some("https://api-docs.deepseek.com/"),
            guidance: "Create an API key in the DeepSeek platform console.",
        },
        ProviderKind::NvidiaNim => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://build.nvidia.com/settings/api-keys"),
            docs_url: Some("https://build.nvidia.com/explore/discover"),
            guidance: "Create an NVIDIA NIM key in the NVIDIA build console.",
        },
        ProviderKind::Openai => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://platform.openai.com/api-keys"),
            docs_url: Some("https://platform.openai.com/docs/api-reference"),
            guidance: "Create an OpenAI API key, or configure the credential for your compatible endpoint.",
        },
        ProviderKind::Atlascloud => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://atlascloud.ai/docs/en/api-keys"),
            docs_url: Some("https://atlascloud.ai/docs/en/api-keys"),
            guidance: "Follow Atlas Cloud's API Keys guide to create a credential.",
        },
        ProviderKind::WanjieArk => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://docs.wanjiedata.com/maas/maas-openapi-v1.html"),
            docs_url: Some("https://docs.wanjiedata.com/maas/maas-openapi-v1.html"),
            guidance: "Follow Wanjie MaaS's APIKEY guide to create a credential.",
        },
        ProviderKind::Volcengine => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://console.volcengine.com/ark/apiKey"),
            docs_url: Some("https://www.volcengine.com/docs/82379/1541594"),
            guidance: "Create a Volcengine Ark API key in the Ark console.",
        },
        ProviderKind::Openrouter => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://openrouter.ai/settings/keys"),
            docs_url: Some("https://openrouter.ai/docs/api/reference/authentication"),
            guidance: "Create an OpenRouter key from account settings.",
        },
        ProviderKind::XiaomiMimo => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://platform.xiaomimimo.com/token-plan"),
            docs_url: Some("https://mimo.mi.com/docs/en-US/tokenplan/Token%20Plan/subscription"),
            guidance: "Create a Xiaomi MiMo Token Plan or pay-as-you-go key and keep its matching base URL.",
        },
        ProviderKind::Novita => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://novita.ai/en/settings/key-management"),
            docs_url: Some("https://novita.ai/docs/guides/quickstart"),
            guidance: "Create a Novita key in account Key Management.",
        },
        ProviderKind::Fireworks => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://fireworks.ai/api-keys"),
            docs_url: Some("https://docs.fireworks.ai/getting-started/quickstart"),
            guidance: "Create a Fireworks API key before configuring the provider.",
        },
        ProviderKind::Siliconflow => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://cloud.siliconflow.com/account/ak"),
            docs_url: Some("https://docs.siliconflow.com/en/userguide/quickstart"),
            guidance: "Use the global SiliconFlow console for the global endpoint.",
        },
        ProviderKind::SiliconflowCN => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://cloud.siliconflow.cn/account/ak"),
            docs_url: Some("https://docs.siliconflow.cn/en/userguide/quickstart"),
            guidance: "Use the China SiliconFlow console for the China endpoint.",
        },
        ProviderKind::Arcee => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://docs.arcee.ai/other/create-your-first-api-key"),
            docs_url: Some("https://docs.arcee.ai/other/create-your-first-api-key"),
            guidance: "Follow Arcee's API key guide to create a credential.",
        },
        ProviderKind::Moonshot => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://platform.kimi.ai/console/api-keys"),
            docs_url: Some("https://platform.kimi.ai/docs/overview"),
            guidance: "For Moonshot's default direct API route, sign in to Kimi API Platform and create and copy an API key. A configured Kimi Code route uses a separate membership-plan console and never imports Kimi CLI credentials; first-class Kimi OAuth is not available.",
        },
        ProviderKind::Sglang => CredentialHelp {
            acquisition: LocalOptional,
            credential_url: None,
            docs_url: Some("https://docs.sglang.ai/"),
            guidance: "Self-hosted SGLang is keyless by default; configure a key only if your server requires one.",
        },
        ProviderKind::Vllm => CredentialHelp {
            acquisition: LocalOptional,
            credential_url: None,
            docs_url: Some("https://docs.vllm.ai/en/stable/serving/openai_compatible_server/"),
            guidance: "Self-hosted vLLM is keyless by default; configure a key only if your server requires one.",
        },
        ProviderKind::Ollama => CredentialHelp {
            acquisition: LocalOptional,
            credential_url: None,
            docs_url: Some("https://docs.ollama.com/api"),
            guidance: "Local Ollama is keyless by default; configure a key only if your server requires one.",
        },
        ProviderKind::Huggingface => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://huggingface.co/settings/tokens"),
            docs_url: Some("https://huggingface.co/docs/hub/en/security-tokens"),
            guidance: "Create a scoped Hugging Face access token.",
        },
        ProviderKind::Together => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://api.together.ai/settings/api-keys"),
            docs_url: Some("https://docs.together.ai/docs/api-keys-authentication"),
            guidance: "Create a Together API key from account settings.",
        },
        ProviderKind::Qianfan => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://console.bce.baidu.com/iam/#/iam/accesslist"),
            docs_url: Some("https://cloud.baidu.com/doc/qianfan/index.html"),
            guidance: "Create Baidu Qianfan credentials in the Baidu Cloud console.",
        },
        ProviderKind::OpenaiCodex => CredentialHelp {
            acquisition: OAuth,
            credential_url: None,
            docs_url: Some("https://developers.openai.com/codex/"),
            guidance: "Run `codex login`, then explicitly grant Codewhale read-only access to that exact Codex credential file; or use a process-scoped token environment variable.",
        },
        ProviderKind::Anthropic => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://console.anthropic.com/settings/keys"),
            docs_url: Some("https://docs.anthropic.com/en/api/overview"),
            guidance: "Create an Anthropic API key in the Anthropic Console.",
        },
        ProviderKind::Openmodel => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://console.openmodel.ai/"),
            docs_url: Some("https://docs.openmodel.ai/en/docs/getting-started/authentication"),
            guidance: "Create an API key in the OpenModel console, then follow the authentication guide.",
        },
        ProviderKind::Zai => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://z.ai/model-api"),
            docs_url: Some("https://docs.z.ai/api-reference/introduction"),
            guidance: "Create or manage a Z.ai API key from the Model API page.",
        },
        ProviderKind::Stepfun => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://platform.stepfun.ai/"),
            docs_url: Some("https://platform.stepfun.ai/docs/en/quickstart/overview"),
            guidance: "Open Account Management, then Interface Keys, in the StepFun console.",
        },
        ProviderKind::Minimax | ProviderKind::MinimaxAnthropic => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some(
                "https://platform.minimax.io/user-center/basic-information/interface-key",
            ),
            docs_url: Some("https://platform.minimax.io/docs/api-reference/api-overview"),
            guidance: "Create a MiniMax API key or subscription-plan key in the user center.",
        },
        ProviderKind::MinimaxCN | ProviderKind::MinimaxAnthropicCN => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some(
                "https://platform.minimaxi.com/user-center/basic-information",
            ),
            docs_url: Some("https://platform.minimaxi.com/docs/api-reference"),
            guidance: "Create a MiniMax Token Plan or pay-as-you-go key at platform.minimaxi.com; the api.minimaxi.com endpoint is auto-selected.",
        },
        ProviderKind::Deepinfra => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://deepinfra.com/dash/api_keys"),
            docs_url: Some("https://docs.deepinfra.com/quickstart"),
            guidance: "Create a DeepInfra API key from the dashboard.",
        },
        ProviderKind::Sakana => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://console.sakana.ai/api-keys"),
            docs_url: Some("https://console.sakana.ai/get-started"),
            guidance: "Create a Sakana AI key in the console and copy it when shown.",
        },
        ProviderKind::LongCat => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://longcat.chat/platform"),
            docs_url: Some("https://longcat.chat/platform"),
            guidance: "Sign up on the LongCat platform and create an API key.",
        },
        ProviderKind::OpencodeGo => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://opencode.ai/zen/"),
            docs_url: Some("https://opencode.ai/docs/go/"),
            guidance: "Create or copy an OpenCode Go subscription key from OpenCode Zen.",
        },
        ProviderKind::Meta => CredentialHelp {
            acquisition: ApiKey,
            credential_url: Some("https://developer.meta.com/ai/"),
            docs_url: Some("https://developer.meta.com/ai/resources/blog/build-with-muse-spark/"),
            guidance: "Use the Meta developer portal to obtain Model API access and a key.",
        },
        ProviderKind::Xai => CredentialHelp {
            acquisition: ApiKeyOrOAuth,
            credential_url: Some("https://console.x.ai/"),
            docs_url: None,
            guidance: "Use an xAI Console API key or Codewhale's native device login. Reading an existing Grok CLI file requires explicit provider-scoped read-only consent.",
        },
        ProviderKind::Custom => CredentialHelp {
            acquisition: Configuration,
            credential_url: None,
            docs_url: None,
            guidance: "Set this custom provider's base_url and api_key_env or api_key in configuration; no canonical vendor credential page exists.",
        },
    }
}

fn is_exact_https_route(base_url: &str, expected_authority: &str, expected_path: &str) -> bool {
    // URL schemes and host names are ASCII case-insensitive; paths are not.
    // Do not lowercase the whole URL here: a differently-cased path is a
    // neighboring route, not the official endpoint. Keep this intentionally
    // dependency-free because provider metadata is used by low-level config
    // callers that should not need URL parsing machinery just for this guard.
    let trimmed = base_url.trim();
    let normalized = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let Some((scheme, authority_and_path)) = normalized.split_once("://") else {
        return false;
    };
    let Some((authority, path)) = authority_and_path.split_once('/') else {
        return false;
    };

    scheme.eq_ignore_ascii_case("https")
        && authority.eq_ignore_ascii_case(expected_authority)
        && path == expected_path
}

/// Whether a configured route is exactly the official Kimi Code endpoint.
///
/// A trailing slash is insignificant, but neighboring Kimi-hosted paths must
/// not inherit membership-plan credentials merely because they share a host.
#[must_use]
pub fn is_exact_kimi_code_route(kind: ProviderKind, base_url: &str) -> bool {
    if kind != ProviderKind::Moonshot {
        return false;
    }

    is_exact_https_route(base_url, "api.kimi.com", "coding/v1")
}

/// Whether a configured route is exactly Moonshot's direct API endpoint.
///
/// Direct K3 owns a different reasoning-control dialect from the Kimi Code
/// membership endpoint. Keep this route guard exact so custom gateways and
/// neighboring Moonshot paths do not inherit direct-K3 wire semantics.
#[must_use]
pub fn is_exact_moonshot_platform_route(kind: ProviderKind, base_url: &str) -> bool {
    kind == ProviderKind::Moonshot && is_exact_https_route(base_url, "api.moonshot.ai", "v1")
}

/// Return credential help for one concrete provider route.
///
/// This protects non-UI callers such as diagnostics and command surfaces from
/// presenting Moonshot's direct API console for a Kimi Code membership-plan
/// endpoint. It performs no discovery, credential lookup, or network I/O.
#[must_use]
pub fn credential_help_for_route(kind: ProviderKind, base_url: &str) -> CredentialHelp {
    if is_exact_kimi_code_route(kind, base_url) {
        return CredentialHelp {
            acquisition: CredentialAcquisition::ApiKey,
            credential_url: Some(KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL),
            docs_url: None,
            guidance: "Create a Kimi Code membership-plan API key in the Kimi Code console. This route uses api.kimi.com/coding/v1; Codewhale does not import Kimi CLI credentials.",
        };
    }

    credential_help(kind)
}

macro_rules! provider {
    (
        $struct_name:ident,
        $kind:ident,
        $id:literal,
        $display_name:literal,
        $base_url:ident,
        $model:ident,
        [$($env_var:literal),* $(,)?],
        $config_key:literal,
        aliases: [$($alias:literal),* $(,)?]
    ) => {
        /// Zero-sized metadata entry for this built-in provider.
        pub struct $struct_name;

        impl Provider for $struct_name {
            fn id(&self) -> &'static str {
                $id
            }

            fn kind(&self) -> ProviderKind {
                ProviderKind::$kind
            }

            fn display_name(&self) -> &'static str {
                $display_name
            }

            fn default_base_url(&self) -> &'static str {
                $base_url
            }

            fn default_model(&self) -> &'static str {
                $model
            }

            fn env_vars(&self) -> &'static [&'static str] {
                &[$($env_var),*]
            }

            fn provider_config_key(&self) -> &'static str {
                $config_key
            }

            fn aliases(&self) -> &'static [&'static str] {
                &[$($alias),*]
            }
        }
    };
}

provider!(
    Deepseek,
    Deepseek,
    "deepseek",
    "DeepSeek",
    DEFAULT_DEEPSEEK_BASE_URL,
    DEFAULT_DEEPSEEK_MODEL,
    ["DEEPSEEK_API_KEY"],
    "deepseek",
    aliases: ["deep-seek", "deepseek-cn", "deepseek_china", "deepseekcn", "deepseek-china"]
);

/// Opt-in DeepSeek route that speaks the Anthropic Messages wire protocol.
pub struct DeepseekAnthropic;

impl Provider for DeepseekAnthropic {
    fn id(&self) -> &'static str {
        "deepseek-anthropic"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::DeepseekAnthropic
    }

    fn display_name(&self) -> &'static str {
        "DeepSeek (Anthropic-compatible)"
    }

    fn default_base_url(&self) -> &'static str {
        DEFAULT_DEEPSEEK_ANTHROPIC_BASE_URL
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_DEEPSEEK_ANTHROPIC_MODEL
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["DEEPSEEK_API_KEY"]
    }

    fn provider_config_key(&self) -> &'static str {
        "deepseek_anthropic"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["deepseek_anthropic", "deepseek-claude", "deepseek_claude"]
    }

    fn wire(&self) -> WireFormat {
        WireFormat::AnthropicMessages
    }
}
provider!(
    NvidiaNim,
    NvidiaNim,
    "nvidia-nim",
    "NVIDIA NIM",
    DEFAULT_NVIDIA_NIM_BASE_URL,
    DEFAULT_NVIDIA_NIM_MODEL,
    ["NVIDIA_API_KEY", "NVIDIA_NIM_API_KEY", "DEEPSEEK_API_KEY"],
    "nvidia_nim",
    aliases: ["nvidia", "nvidia_nim", "nim"]
);
provider!(
    Openai,
    Openai,
    "openai",
    "OpenAI-compatible",
    DEFAULT_OPENAI_BASE_URL,
    DEFAULT_OPENAI_MODEL,
    ["OPENAI_API_KEY"],
    "openai",
    aliases: ["open-ai"]
);
provider!(
    Atlascloud,
    Atlascloud,
    "atlascloud",
    "AtlasCloud",
    DEFAULT_ATLASCLOUD_BASE_URL,
    DEFAULT_ATLASCLOUD_MODEL,
    ["ATLASCLOUD_API_KEY"],
    "atlascloud",
    aliases: ["atlas-cloud", "atlas_cloud", "atlas"]
);
provider!(
    WanjieArk,
    WanjieArk,
    "wanjie-ark",
    "Wanjie Ark",
    DEFAULT_WANJIE_ARK_BASE_URL,
    DEFAULT_WANJIE_ARK_MODEL,
    [
        "WANJIE_ARK_API_KEY",
        "WANJIE_API_KEY",
        "WANJIE_MAAS_API_KEY"
    ],
    "wanjie_ark",
    aliases: ["wanjie", "wanjie_ark", "ark-wanjie", "ark_wanjie", "wanjieark", "wanjie-maas", "wanjie_maas", "wanjiemaas"]
);
provider!(
    Volcengine,
    Volcengine,
    "volcengine",
    "Volcengine Ark",
    DEFAULT_VOLCENGINE_BASE_URL,
    DEFAULT_VOLCENGINE_MODEL,
    [
        "VOLCENGINE_API_KEY",
        "VOLCENGINE_ARK_API_KEY",
        "ARK_API_KEY"
    ],
    "volcengine",
    aliases: ["volcengine-ark", "volcengine_ark", "ark", "volc-ark", "volcengineark"]
);
provider!(
    Openrouter,
    Openrouter,
    "openrouter",
    "OpenRouter",
    DEFAULT_OPENROUTER_BASE_URL,
    DEFAULT_OPENROUTER_MODEL,
    ["OPENROUTER_API_KEY"],
    "openrouter",
    aliases: ["open_router"]
);
provider!(
    XiaomiMimo,
    XiaomiMimo,
    "xiaomi-mimo",
    "Xiaomi MiMo",
    DEFAULT_XIAOMI_MIMO_BASE_URL,
    DEFAULT_XIAOMI_MIMO_MODEL,
    [
        "XIAOMI_MIMO_TOKEN_PLAN_API_KEY",
        "MIMO_TOKEN_PLAN_API_KEY",
        "XIAOMI_MIMO_API_KEY",
        "XIAOMI_API_KEY",
        "MIMO_API_KEY",
    ],
    "xiaomi_mimo",
    aliases: ["xiaomi_mimo", "xiaomimimo", "mimo", "xiaomi"]
);
provider!(
    Novita,
    Novita,
    "novita",
    "Novita AI",
    DEFAULT_NOVITA_BASE_URL,
    DEFAULT_NOVITA_MODEL,
    ["NOVITA_API_KEY"],
    "novita",
    // `novita-ai` is the id Models.dev publishes for this provider; without it a
    // live/full Models.dev catalog row keyed `novita-ai` would fail to normalize
    // onto ProviderKind::Novita (Refs #4186).
    aliases: ["novita-ai", "novita_ai"]
);
provider!(
    Fireworks,
    Fireworks,
    "fireworks",
    "Fireworks AI",
    DEFAULT_FIREWORKS_BASE_URL,
    DEFAULT_FIREWORKS_MODEL,
    ["FIREWORKS_API_KEY"],
    "fireworks",
    aliases: ["fireworks-ai"]
);
provider!(
    Siliconflow,
    Siliconflow,
    "siliconflow",
    "SiliconFlow",
    DEFAULT_SILICONFLOW_BASE_URL,
    DEFAULT_SILICONFLOW_MODEL,
    ["SILICONFLOW_API_KEY"],
    "siliconflow",
    aliases: ["silicon-flow", "silicon_flow"]
);
provider!(
    SiliconflowCN,
    SiliconflowCN,
    "siliconflow-CN",
    "SiliconFlow (China)",
    DEFAULT_SILICONFLOW_CN_BASE_URL,
    DEFAULT_SILICONFLOW_MODEL,
    ["SILICONFLOW_API_KEY"],
    "siliconflow_cn",
    aliases: [
        "silicon-flow-cn",
        "silicon-flow-CN",
        "silicon_flow_cn",
        "silicon_flow_CN",
        "siliconflow-china",
    ]
);
provider!(
    Arcee,
    Arcee,
    "arcee",
    "Arcee AI",
    DEFAULT_ARCEE_BASE_URL,
    DEFAULT_ARCEE_MODEL,
    ["ARCEE_API_KEY"],
    "arcee",
    aliases: ["arcee-ai", "arcee_ai"]
);
provider!(
    Moonshot,
    Moonshot,
    "moonshot",
    "Moonshot/Kimi",
    DEFAULT_MOONSHOT_BASE_URL,
    DEFAULT_MOONSHOT_MODEL,
    ["MOONSHOT_API_KEY", "KIMI_API_KEY"],
    "moonshot",
    // `moonshotai` is the id Models.dev publishes for Moonshot/Kimi; without
    // it a live/full Models.dev catalog row keyed `moonshotai` would fail to
    // normalize onto ProviderKind::Moonshot (Refs #4186).
    aliases: ["moonshot-ai", "moonshotai", "moonshot_ai", "kimi", "kimi-k2"]
);
provider!(
    Sglang,
    Sglang,
    "sglang",
    "SGLang",
    DEFAULT_SGLANG_BASE_URL,
    DEFAULT_SGLANG_MODEL,
    ["SGLANG_API_KEY"],
    "sglang",
    aliases: ["sg-lang"]
);
provider!(
    Vllm,
    Vllm,
    "vllm",
    "vLLM",
    DEFAULT_VLLM_BASE_URL,
    DEFAULT_VLLM_MODEL,
    ["VLLM_API_KEY"],
    "vllm",
    aliases: ["v-llm"]
);
provider!(
    Ollama,
    Ollama,
    "ollama",
    "Ollama",
    DEFAULT_OLLAMA_BASE_URL,
    DEFAULT_OLLAMA_MODEL,
    ["OLLAMA_API_KEY"],
    "ollama",
    aliases: ["ollama-local"]
);
provider!(
    Huggingface,
    Huggingface,
    "huggingface",
    "Hugging Face",
    DEFAULT_HUGGINGFACE_BASE_URL,
    DEFAULT_HUGGINGFACE_MODEL,
    ["HUGGINGFACE_API_KEY", "HF_TOKEN"],
    "huggingface",
    aliases: ["hugging-face", "hugging_face", "hf"]
);
provider!(
    Together,
    Together,
    "together",
    "Together AI",
    DEFAULT_TOGETHER_BASE_URL,
    DEFAULT_TOGETHER_MODEL,
    ["TOGETHER_API_KEY"],
    "together",
    // `togetherai` (no separator) is the id Models.dev publishes for Together;
    // the hyphen/underscore spellings are legacy config aliases. All three must
    // normalize onto ProviderKind::Together so live-catalog rows keyed
    // `togetherai` resolve to the right kind (Refs #4186).
    aliases: ["together-ai", "together_ai", "togetherai"]
);
provider!(
    Qianfan,
    Qianfan,
    "qianfan",
    "Baidu Qianfan",
    DEFAULT_QIANFAN_BASE_URL,
    DEFAULT_QIANFAN_MODEL,
    ["QIANFAN_API_KEY", "BAIDU_QIANFAN_API_KEY"],
    "qianfan",
    aliases: ["baidu-qianfan", "baidu_qianfan", "baidu"]
);

/// OpenAI Codex / ChatGPT OAuth provider using the Responses API.
pub struct OpenaiCodex;

impl Provider for OpenaiCodex {
    fn id(&self) -> &'static str {
        "openai-codex"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenaiCodex
    }

    fn display_name(&self) -> &'static str {
        "OpenAI Codex (ChatGPT)"
    }

    fn default_base_url(&self) -> &'static str {
        DEFAULT_OPENAI_CODEX_BASE_URL
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_OPENAI_CODEX_MODEL
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["OPENAI_CODEX_ACCESS_TOKEN", "CODEX_ACCESS_TOKEN"]
    }

    fn provider_config_key(&self) -> &'static str {
        "openai_codex"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[
            "openai_codex",
            "openaicodex",
            "codex",
            "chatgpt",
            "chatgpt-codex",
            "chatgpt_codex",
            "chatgptcodex",
        ]
    }

    fn wire(&self) -> WireFormat {
        WireFormat::Responses
    }
}

/// Native Anthropic Messages API provider (#3014).
pub struct Anthropic;

impl Provider for Anthropic {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn display_name(&self) -> &'static str {
        "Anthropic"
    }

    fn default_base_url(&self) -> &'static str {
        crate::DEFAULT_ANTHROPIC_BASE_URL
    }

    fn default_model(&self) -> &'static str {
        crate::DEFAULT_ANTHROPIC_MODEL
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["ANTHROPIC_API_KEY"]
    }

    fn provider_config_key(&self) -> &'static str {
        "anthropic"
    }

    fn wire(&self) -> WireFormat {
        WireFormat::AnthropicMessages
    }
}

/// OpenModel Anthropic-compatible Messages API provider.
pub struct Openmodel;

impl Provider for Openmodel {
    fn id(&self) -> &'static str {
        "openmodel"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Openmodel
    }

    fn display_name(&self) -> &'static str {
        "OpenModel"
    }

    fn default_base_url(&self) -> &'static str {
        DEFAULT_OPENMODEL_BASE_URL
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_OPENMODEL_MODEL
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["OPENMODEL_API_KEY"]
    }

    fn provider_config_key(&self) -> &'static str {
        "openmodel"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["open-model", "open_model"]
    }

    fn wire(&self) -> WireFormat {
        WireFormat::AnthropicMessages
    }
}

provider!(
    Zai,
    Zai,
    "zai",
    "Zhipu AI / Z.ai",
    DEFAULT_ZAI_BASE_URL,
    DEFAULT_ZAI_MODEL,
    ["ZAI_API_KEY", "Z_AI_API_KEY", "ZHIPU_API_KEY", "GLM_API_KEY"],
    "zai",
    aliases: ["z-ai", "z_ai", "z.ai", "zhipu", "zhipuai", "bigmodel", "big-model"]
);

provider!(
    Stepfun,
    Stepfun,
    "stepfun",
    "StepFun / StepFlash",
    DEFAULT_STEPFUN_BASE_URL,
    DEFAULT_STEPFUN_MODEL,
    ["STEPFUN_API_KEY", "STEP_API_KEY"],
    "stepfun",
    aliases: ["step-fun", "step_fun", "stepflash", "step-flash", "step_flash"]
);

provider!(
    Minimax,
    Minimax,
    "minimax",
    "MiniMax",
    DEFAULT_MINIMAX_BASE_URL,
    DEFAULT_MINIMAX_MODEL,
    ["MINIMAX_API_KEY"],
    "minimax",
    aliases: ["mini-max", "mini_max"]
);

provider!(
    MinimaxCN,
    MinimaxCN,
    "minimax-cn",
    "MiniMax (China / Token Plan)",
    DEFAULT_MINIMAX_CN_BASE_URL,
    DEFAULT_MINIMAX_MODEL,
    ["MINIMAX_API_KEY"],
    "minimax_cn",
    aliases: [
        "minimax-CN",
        "minimax_CN",
        "minimax-china",
        "minimax_china",
        "minimax-token-plan",
        "minimax_token_plan"
    ]
);

/// MiniMax route that speaks the Anthropic Messages wire protocol.
pub struct MinimaxAnthropic;

impl Provider for MinimaxAnthropic {
    fn id(&self) -> &'static str {
        "minimax-anthropic"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::MinimaxAnthropic
    }

    fn display_name(&self) -> &'static str {
        "MiniMax (Anthropic-compatible)"
    }

    fn default_base_url(&self) -> &'static str {
        DEFAULT_MINIMAX_ANTHROPIC_BASE_URL
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_MINIMAX_MODEL
    }

    fn env_vars(&self) -> &'static [&'static str] {
        &["MINIMAX_API_KEY"]
    }

    fn provider_config_key(&self) -> &'static str {
        "minimax_anthropic"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[
            "minimax_anthropic",
            "mini-max-anthropic",
            "mini_max_anthropic",
        ]
    }

    fn wire(&self) -> WireFormat {
        WireFormat::AnthropicMessages
    }
}

/// MiniMax route hosted at `api.minimaxi.com` that speaks the Anthropic Messages
/// wire protocol. This is the regional / Token Plan sibling of [`MinimaxAnthropic`]:
/// users with `platform.minimaxi.com` keys (Token Plan subscriptions issued under
/// the Chinese platform) point this provider at the same wire endpoint without
/// having to override `base_url` manually.
pub struct MinimaxAnthropicCN;

impl Provider for MinimaxAnthropicCN {
    fn id(&self) -> &'static str {
        "minimax-anthropic-cn"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::MinimaxAnthropicCN
    }

    fn display_name(&self) -> &'static str {
        "MiniMax Anthropic-compatible (China / Token Plan)"
    }

    fn default_base_url(&self) -> &'static str {
        DEFAULT_MINIMAX_ANTHROPIC_CN_BASE_URL
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_MINIMAX_MODEL
    }

    fn env_vars(&self) -> &'static [&'static str] {
        // Token Plan keys share the same env var as pay-as-you-go keys; codewhale
        // does not currently distinguish subscription vs. pay-as-you-go routing
        // — the base URL is what selects the surface.
        &["MINIMAX_API_KEY"]
    }

    fn provider_config_key(&self) -> &'static str {
        "minimax_anthropic_cn"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &[
            "minimax_anthropic_cn",
            "minimax-anthropic-CN",
            "minimax_anthropic_CN",
            "minimax-anthropic-china",
            "minimax-china-anthropic",
            "minimax-token-plan",
            "minimax_token_plan",
        ]
    }

    fn wire(&self) -> WireFormat {
        WireFormat::AnthropicMessages
    }
}

provider!(
    Deepinfra,
    Deepinfra,
    "deepinfra",
    "DeepInfra",
    DEFAULT_DEEPINFRA_BASE_URL,
    DEFAULT_DEEPINFRA_MODEL,
    ["DEEPINFRA_API_KEY", "DEEPINFRA_TOKEN"],
    "deepinfra",
    aliases: ["deep-infra", "deep_infra"]
);

provider!(
    Sakana,
    Sakana,
    "sakana",
    "Sakana AI (Fugu)",
    DEFAULT_SAKANA_BASE_URL,
    DEFAULT_SAKANA_MODEL,
    ["FUGU_API_KEY", "SAKANA_API_KEY"],
    "sakana",
    aliases: ["sakana-ai", "sakana_ai", "fugu"]
);

provider!(
    LongCat,
    LongCat,
    "longcat",
    "Meituan LongCat",
    DEFAULT_LONGCAT_BASE_URL,
    DEFAULT_LONGCAT_MODEL,
    ["LONGCAT_API_KEY"],
    "longcat",
    aliases: ["long-cat", "meituan-longcat", "meituan"]
);

provider!(
    OpencodeGo,
    OpencodeGo,
    "opencode-go",
    "OpenCode Go",
    DEFAULT_OPENCODE_GO_BASE_URL,
    DEFAULT_OPENCODE_GO_MODEL,
    ["OPENCODE_GO_API_KEY"],
    "opencode_go",
    aliases: ["opencode_go", "opencodego"]
);

provider!(
    Meta,
    Meta,
    "meta",
    "Meta Model API",
    DEFAULT_META_BASE_URL,
    DEFAULT_META_MODEL,
    ["META_MODEL_API_KEY", "MODEL_API_KEY"],
    "meta",
    aliases: [
        "meta-ai",
        "meta_ai",
        "meta-model-api",
        "meta_model_api",
        "muse",
        "muse-spark"
    ]
);

provider!(
    Xai,
    Xai,
    "xai",
    "xAI",
    DEFAULT_XAI_BASE_URL,
    DEFAULT_XAI_MODEL,
    ["XAI_API_KEY"],
    "xai",
    aliases: ["x-ai", "x_ai", "grok"]
);

/// User-defined OpenAI-compatible endpoint (#1519).
///
/// A single dynamic provider identity for arbitrary `[providers.<name>]
/// kind="openai-compatible"` config entries. Unlike the built-in providers it
/// carries no real default base URL/model/env var: the concrete endpoint, model
/// id, and auth env var all arrive from the named `[providers.<name>]` config
/// table at route time. The placeholder base URL/model here exist only so the
/// descriptor stays well-formed (non-empty) for conformance; runtime routing
/// always supplies a `base_url_override` and a wire model id, so these
/// placeholders are never used to reach the network.
pub struct Custom;

impl Provider for Custom {
    fn id(&self) -> &'static str {
        "custom"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Custom
    }

    fn display_name(&self) -> &'static str {
        "Custom (OpenAI-compatible)"
    }

    fn default_base_url(&self) -> &'static str {
        // Placeholder only; the real endpoint comes from the named config table
        // via the route's base_url_override. Loopback so a misconfigured custom
        // provider fails closed locally rather than reaching a public host.
        "http://localhost/v1"
    }

    fn default_model(&self) -> &'static str {
        // Placeholder only; the real model id comes from config and is preserved
        // verbatim as the wire model id.
        "custom-model"
    }

    fn env_vars(&self) -> &'static [&'static str] {
        // No built-in env var: the auth env var is named per-entry via
        // `[providers.<name>] api_key_env = "..."`.
        &[]
    }

    fn provider_config_key(&self) -> &'static str {
        "custom"
    }

    fn wire(&self) -> WireFormat {
        WireFormat::ChatCompletions
    }
}

static DEEPSEEK: Deepseek = Deepseek;
static DEEPSEEK_ANTHROPIC: DeepseekAnthropic = DeepseekAnthropic;
static NVIDIA_NIM: NvidiaNim = NvidiaNim;
static OPENAI: Openai = Openai;
static ATLASCLOUD: Atlascloud = Atlascloud;
static WANJIE_ARK: WanjieArk = WanjieArk;
static VOLCENGINE: Volcengine = Volcengine;
static OPENROUTER: Openrouter = Openrouter;
static XIAOMI_MIMO: XiaomiMimo = XiaomiMimo;
static NOVITA: Novita = Novita;
static FIREWORKS: Fireworks = Fireworks;
static SILICONFLOW: Siliconflow = Siliconflow;
static SILICONFLOW_CN: SiliconflowCN = SiliconflowCN;
static ARCEE: Arcee = Arcee;
static MOONSHOT: Moonshot = Moonshot;
static SGLANG: Sglang = Sglang;
static VLLM: Vllm = Vllm;
static OLLAMA: Ollama = Ollama;
static HUGGINGFACE: Huggingface = Huggingface;
static TOGETHER: Together = Together;
static QIANFAN: Qianfan = Qianfan;
static OPENAI_CODEX: OpenaiCodex = OpenaiCodex;
static ANTHROPIC: Anthropic = Anthropic;
static OPENMODEL: Openmodel = Openmodel;
static ZAI: Zai = Zai;
static STEPFUN: Stepfun = Stepfun;
static MINIMAX: Minimax = Minimax;
static MINIMAX_CN: MinimaxCN = MinimaxCN;
static MINIMAX_ANTHROPIC: MinimaxAnthropic = MinimaxAnthropic;
static MINIMAX_ANTHROPIC_CN: MinimaxAnthropicCN = MinimaxAnthropicCN;
static DEEPINFRA: Deepinfra = Deepinfra;
static SAKANA: Sakana = Sakana;
static LONGCAT: LongCat = LongCat;
static OPENCODE_GO: OpencodeGo = OpencodeGo;
static META: Meta = Meta;
static XAI: Xai = Xai;
static CUSTOM: Custom = Custom;

static PROVIDER_REGISTRY: [&dyn Provider; 35] = [
    &DEEPSEEK,
    &DEEPSEEK_ANTHROPIC,
    &NVIDIA_NIM,
    &OPENAI,
    &ATLASCLOUD,
    &WANJIE_ARK,
    &VOLCENGINE,
    &OPENROUTER,
    &XIAOMI_MIMO,
    &NOVITA,
    &FIREWORKS,
    &SILICONFLOW,
    &ARCEE,
    &SILICONFLOW_CN,
    &MOONSHOT,
    &SGLANG,
    &VLLM,
    &OLLAMA,
    &HUGGINGFACE,
    &TOGETHER,
    &QIANFAN,
    &OPENAI_CODEX,
    &ANTHROPIC,
    &OPENMODEL,
    &ZAI,
    &STEPFUN,
    &MINIMAX,
    &MINIMAX_CN,
    &MINIMAX_ANTHROPIC,
    &MINIMAX_ANTHROPIC_CN,
    &DEEPINFRA,
    &SAKANA,
    &LONGCAT,
    &OPENCODE_GO,
    &META,
    &XAI,
    &CUSTOM,
];

/// Return all built-in provider metadata entries in `ProviderKind::ALL` order.
///
/// This insertion order is the stable order used for internal parsing and
/// default selection. It is intentionally NOT the order user-facing UI should
/// render; for browsing/picker surfaces use [`providers_sorted_for_display`].
#[must_use]
pub fn all_providers() -> &'static [&'static dyn Provider] {
    &PROVIDER_REGISTRY
}

/// Return all built-in providers ordered for user-facing display.
///
/// Providers are sorted alphabetically (case-insensitively) by
/// [`Provider::display_name`] so model/provider browsing surfaces present a
/// neutral, predictable list rather than leading with whichever provider
/// happens to sit first in [`ProviderKind::ALL`] (historically DeepSeek). The
/// ordering policy intentionally differs from internal parsing/default order:
///
/// - [`all_providers`] / [`ProviderKind::ALL`] — stable order for internal
///   matching, parsing, and default selection. Do not reorder.
/// - [`providers_sorted_for_display`] — neutral alphabetical order for UI
///   browsing. DeepSeek stays present and searchable but is not hard-coded
///   first; a caller may still highlight/pin the active provider separately.
///
/// Returns an owned `Vec` because the sorted order is computed, not static.
#[must_use]
pub fn providers_sorted_for_display() -> Vec<&'static dyn Provider> {
    let mut providers = all_providers().to_vec();
    providers.sort_by(|a, b| {
        a.display_name()
            .to_ascii_lowercase()
            .cmp(&b.display_name().to_ascii_lowercase())
    });
    providers
}

/// Find a provider by canonical id only.
#[must_use]
pub fn lookup_provider(id: &str) -> Option<&'static dyn Provider> {
    let id = id.trim();
    all_providers()
        .iter()
        .copied()
        .find(|provider| provider.id() == id)
}

/// Resolve a provider by canonical id or supported legacy alias.
#[must_use]
pub fn resolve_provider(id_or_alias: &str) -> Option<&'static dyn Provider> {
    ProviderKind::parse(id_or_alias).map(provider_for_kind)
}

/// Return metadata for a known provider kind.
#[must_use]
pub fn provider_for_kind(kind: ProviderKind) -> &'static dyn Provider {
    PROVIDER_REGISTRY
        .iter()
        .find(|p| p.kind() == kind)
        .copied()
        .expect("ProviderKind variant missing from PROVIDER_REGISTRY")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_help_covers_every_provider_without_guessing_non_key_urls() {
        for provider in all_providers() {
            let help = provider.credential_help();
            assert!(
                !help.guidance.trim().is_empty(),
                "{} credential guidance must not be empty",
                provider.id()
            );

            match help.acquisition {
                CredentialAcquisition::ApiKey | CredentialAcquisition::ApiKeyOrOAuth => {
                    assert!(
                        help.credential_url.is_some(),
                        "{} needs a stable provider-owned credential link",
                        provider.id()
                    );
                }
                CredentialAcquisition::LocalOptional
                | CredentialAcquisition::OAuth
                | CredentialAcquisition::Configuration => assert!(
                    help.credential_url.is_none(),
                    "{} must explain its non-key route instead of inventing a credential link",
                    provider.id()
                ),
            }
        }
    }

    #[test]
    fn kimi_credential_help_uses_the_durable_api_key_console_only() {
        let help = provider_for_kind(ProviderKind::Moonshot).credential_help();

        assert_eq!(help.acquisition, CredentialAcquisition::ApiKey);
        assert_eq!(
            help.credential_url,
            Some("https://platform.kimi.ai/console/api-keys")
        );
        assert_eq!(
            help.docs_url,
            Some("https://platform.kimi.ai/docs/overview")
        );
        assert!(help.guidance.contains("create and copy an API key"));
        assert!(help.guidance.contains("OAuth is not available"));
    }

    #[test]
    fn kimi_code_route_credential_help_is_distinct_from_direct_moonshot() {
        let direct = credential_help_for_route(ProviderKind::Moonshot, DEFAULT_MOONSHOT_BASE_URL);
        let kimi_code =
            credential_help_for_route(ProviderKind::Moonshot, "https://api.kimi.com/coding/v1/");

        assert_eq!(
            direct.credential_url,
            Some("https://platform.kimi.ai/console/api-keys")
        );
        assert_eq!(
            kimi_code.credential_url,
            Some(KIMI_CODE_MEMBERSHIP_PLAN_CONSOLE_URL)
        );
        assert_eq!(kimi_code.docs_url, None);
        assert!(kimi_code.guidance.contains("membership-plan API key"));
        assert!(
            kimi_code
                .guidance
                .contains("does not import Kimi CLI credentials")
        );
        assert!(!is_exact_kimi_code_route(
            ProviderKind::Moonshot,
            "https://api.kimi.com/coding/v1/preview"
        ));

        // Scheme and hostname casing are insignificant, but the endpoint
        // path is a route identifier and must remain exact.
        assert!(is_exact_kimi_code_route(
            ProviderKind::Moonshot,
            "HTTPS://API.KIMI.COM/coding/v1/"
        ));
        for neighboring_route in [
            "https://api.kimi.com/CODING/v1",
            "https://api.kimi.com/coding/V1",
            "http://api.kimi.com/coding/v1",
            "https://api.kimi.com:443/coding/v1",
            "https://api.kimi.com/coding/v1?preview=1",
            "https://api.kimi.com/coding/v1#fragment",
            "https://api.kimi.com/coding/v1//",
        ] {
            assert!(
                !is_exact_kimi_code_route(ProviderKind::Moonshot, neighboring_route),
                "{neighboring_route} must not inherit Kimi Code membership semantics"
            );
        }
    }

    #[test]
    fn direct_moonshot_route_matching_is_exact() {
        assert!(is_exact_moonshot_platform_route(
            ProviderKind::Moonshot,
            "HTTPS://API.MOONSHOT.AI/v1/"
        ));
        for neighboring_route in [
            "https://api.moonshot.ai/V1",
            "http://api.moonshot.ai/v1",
            "https://api.moonshot.ai:443/v1",
            "https://api.moonshot.ai/v1?preview=1",
            "https://api.moonshot.ai/v1#fragment",
            "https://api.moonshot.ai/v1//",
            "https://api.moonshot.ai/v1/chat/completions",
            "https://api.kimi.com/coding/v1",
        ] {
            assert!(
                !is_exact_moonshot_platform_route(ProviderKind::Moonshot, neighboring_route),
                "{neighboring_route} must not inherit direct Moonshot semantics"
            );
        }
        assert!(!is_exact_moonshot_platform_route(
            ProviderKind::Openai,
            DEFAULT_MOONSHOT_BASE_URL
        ));
    }

    #[test]
    fn non_key_and_mixed_routes_are_typed_explicitly() {
        for kind in [
            ProviderKind::Sglang,
            ProviderKind::Vllm,
            ProviderKind::Ollama,
        ] {
            assert_eq!(
                provider_for_kind(kind).credential_help().acquisition,
                CredentialAcquisition::LocalOptional
            );
        }
        assert_eq!(
            provider_for_kind(ProviderKind::OpenaiCodex)
                .credential_help()
                .acquisition,
            CredentialAcquisition::OAuth
        );
        assert_eq!(
            provider_for_kind(ProviderKind::Xai)
                .credential_help()
                .acquisition,
            CredentialAcquisition::ApiKeyOrOAuth
        );
        assert_eq!(
            provider_for_kind(ProviderKind::Custom)
                .credential_help()
                .acquisition,
            CredentialAcquisition::Configuration
        );
    }

    #[test]
    fn live_verified_console_replacements_do_not_regress_to_404_links() {
        let openmodel = provider_for_kind(ProviderKind::Openmodel).credential_help();
        assert_eq!(
            openmodel.credential_url,
            Some("https://console.openmodel.ai/")
        );
        assert_eq!(
            openmodel.docs_url,
            Some("https://docs.openmodel.ai/en/docs/getting-started/authentication")
        );

        let sakana = provider_for_kind(ProviderKind::Sakana).credential_help();
        assert_eq!(
            sakana.credential_url,
            Some("https://console.sakana.ai/api-keys")
        );
        assert_eq!(
            sakana.docs_url,
            Some("https://console.sakana.ai/get-started")
        );
    }

    #[test]
    fn display_order_is_alphabetical_by_display_name() {
        let display = providers_sorted_for_display();
        let names: Vec<String> = display
            .iter()
            .map(|p| p.display_name().to_ascii_lowercase())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "providers_sorted_for_display must be alphabetical (case-insensitive) by display name"
        );
    }

    #[test]
    fn display_order_differs_from_internal_all_order() {
        // The whole point of the helper is that UI ordering is NOT the
        // internal ProviderKind::ALL / all_providers() insertion order.
        let display_ids: Vec<&str> = providers_sorted_for_display()
            .iter()
            .map(|p| p.id())
            .collect();
        let internal_ids: Vec<&str> = all_providers().iter().map(|p| p.id()).collect();
        assert_ne!(
            display_ids, internal_ids,
            "display order should not match internal ALL order"
        );
    }

    #[test]
    fn display_order_is_complete_and_unique() {
        // No provider is dropped or duplicated by the sort.
        let display = providers_sorted_for_display();
        assert_eq!(
            display.len(),
            all_providers().len(),
            "display order must include every built-in provider"
        );
        let mut ids: Vec<&str> = display.iter().map(|p| p.id()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(
            before,
            ids.len(),
            "display order must not contain duplicates"
        );
    }

    #[test]
    fn deepseek_is_present_but_not_first_in_display_order() {
        // Acceptance: DeepSeek stays searchable but is no longer hard-coded
        // first in provider browsing UI. (It is first in internal ALL order.)
        let display = providers_sorted_for_display();
        assert_eq!(
            all_providers()[0].kind(),
            ProviderKind::Deepseek,
            "DeepSeek is expected to remain first in the stable internal order"
        );
        assert!(
            display.iter().any(|p| p.kind() == ProviderKind::Deepseek),
            "DeepSeek must remain present in display order"
        );
        assert_ne!(
            display[0].kind(),
            ProviderKind::Deepseek,
            "DeepSeek must not be hard-coded first in display order"
        );
        // Anthropic ('Anthropic') sorts before 'DeepSeek' alphabetically, so it
        // is a stable check that the neutral ordering actually took effect.
        assert_eq!(
            display[0].display_name(),
            "Anthropic",
            "alphabetical display order should lead with Anthropic"
        );
    }
}
