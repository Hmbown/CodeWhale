//! Provider enumeration and helpers for codewhale.

use codewhale_config;

use super::DEFAULT_DEEPSEEKCN_BASE_URL;

pub(crate) const API_KEYRING_SENTINEL: &str = "__KEYRING__";
pub const DEFAULT_ZAI_PROVIDER_MAX_CONCURRENCY: usize = 3;
pub const MAX_PROVIDER_REQUEST_CONCURRENCY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProvider {
    Deepseek,
    DeepseekCN,
    DeepseekAnthropic,
    NvidiaNim,
    Openai,
    Atlascloud,
    WanjieArk,
    Volcengine,
    Openrouter,
    XiaomiMimo,
    Novita,
    Fireworks,
    Siliconflow,
    SiliconflowCn,
    Arcee,
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    Huggingface,
    Together,
    Qianfan,
    OpenaiCodex,
    Anthropic,
    Openmodel,
    Zai,
    Stepfun,
    Minimax,
    Deepinfra,
    Sakana,
    /// User-defined OpenAI-compatible endpoint (#1519).
    ///
    /// Selected when `provider = "<name>"` names a `[providers.<name>]
    /// kind="openai-compatible"` table. A single dynamic identity that maps to
    /// [`codewhale_config::ProviderKind::Custom`] and routes via the OpenAI Chat
    /// Completions wire protocol; the concrete endpoint/model/auth come from the
    /// named config table, not from this variant.
    Custom,
}

impl ApiProvider {
    #[must_use]
    pub fn names_hint() -> String {
        let mut names = Vec::with_capacity(Self::all().len() + 1);
        names.push(Self::Deepseek.as_str());
        names.push(Self::DeepseekCN.as_str());
        names.extend(
            Self::all()
                .iter()
                .filter(|provider| !matches!(provider, Self::Deepseek))
                .map(|provider| provider.as_str()),
        );
        names.join(", ")
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        // ApiProvider-specific: "deepseek-cn" is a legacy variant here,
        // while ProviderKind treats it as a Deepseek alias.
        if trimmed.eq_ignore_ascii_case("deepseek-cn")
            || trimmed.eq_ignore_ascii_case("deepseek_china")
            || trimmed.eq_ignore_ascii_case("deepseekcn")
            || trimmed.eq_ignore_ascii_case("deepseek-china")
        {
            return Some(Self::DeepseekCN);
        }
        codewhale_config::ProviderKind::parse(value).map(Self::from_kind)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self.kind() {
            Some(kind) => kind.as_str(),
            None => "deepseek-cn",
        }
    }

    /// Human-friendly label for picker UIs / status chips.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self.kind() {
            Some(kind) => kind.provider().display_name(),
            None => "DeepSeek (legacy alias)",
        }
    }

    /// Provider metadata from the shared config crate.
    ///
    /// Returns `None` only for the TUI-only legacy `DeepseekCN` variant, which
    /// intentionally keeps its own config table while sharing DeepSeek auth envs.
    #[must_use]
    pub fn metadata(self) -> Option<&'static dyn codewhale_config::provider::Provider> {
        self.kind().map(|kind| kind.provider())
    }

    /// Environment variable candidates for this provider's API key.
    #[must_use]
    pub fn env_vars(self) -> &'static [&'static str] {
        self.metadata().map_or(
            codewhale_config::ProviderKind::Deepseek
                .provider()
                .env_vars(),
            |provider| provider.env_vars(),
        )
    }

    /// Environment variable candidates formatted for UI copy.
    #[must_use]
    pub fn env_vars_label(self) -> String {
        self.env_vars().join(" / ")
    }

    /// Providers ordered for picker/browsing surfaces.
    #[must_use]
    pub fn sorted_for_display() -> Vec<Self> {
        codewhale_config::provider::providers_sorted_for_display()
            .iter()
            .map(|provider| Self::from_kind(provider.kind()))
            .collect()
    }

    /// Default base URL for this provider.
    #[must_use]
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::DeepseekCN => DEFAULT_DEEPSEEKCN_BASE_URL,
            _ => self
                .metadata()
                .expect("ApiProvider variant missing ProviderKind metadata")
                .default_base_url(),
        }
    }

    /// Official provider page for creating or locating credentials.
    #[must_use]
    pub fn credential_url(self) -> Option<&'static str> {
        Some(match self {
            Self::Deepseek | Self::DeepseekCN | Self::DeepseekAnthropic => {
                "https://platform.deepseek.com/api_keys"
            }
            Self::NvidiaNim => "https://build.nvidia.com/settings/api-keys",
            Self::Openai => "https://platform.openai.com/api-keys",
            Self::Atlascloud => "https://atlascloud.ai/docs/en/api-keys",
            Self::WanjieArk => "https://docs.wanjiedata.com/maas/maas-openapi-v1.html",
            Self::Volcengine => "https://console.volcengine.com/ark",
            Self::Openrouter => "https://openrouter.ai/settings/keys",
            Self::XiaomiMimo => "https://platform.xiaomimimo.com/token-plan",
            Self::Novita => "https://novita.ai/docs/guides/quickstart",
            Self::Fireworks => "https://fireworks.ai/account/api-keys",
            Self::Siliconflow | Self::SiliconflowCn => "https://cloud.siliconflow.com/account/ak",
            Self::Arcee => "https://docs.arcee.ai/other/create-your-first-api-key",
            Self::Moonshot => "https://platform.kimi.ai/",
            Self::Huggingface => "https://huggingface.co/settings/tokens",
            Self::Together => "https://api.together.ai/settings/api-keys",
            Self::Qianfan => "https://console.bce.baidu.com/iam/#/iam/accesslist",
            Self::Anthropic => "https://console.anthropic.com/settings/keys",
            Self::Openmodel => "https://docs.openmodel.ai/en/docs/guides/api-key",
            Self::Zai => "https://z.ai/model-api",
            Self::Stepfun => "https://platform.stepfun.ai/",
            Self::Minimax => "https://platform.minimax.io/docs/guides/quickstart-preparation",
            Self::Deepinfra => "https://deepinfra.com/dash/api_keys",
            Self::Sakana => "https://api.sakana.ai/",
            Self::OpenaiCodex | Self::Sglang | Self::Vllm | Self::Ollama => return None,
            // Custom endpoints have no canonical credential page; the user
            // supplies the key via their own `api_key_env`.
            Self::Custom => return None,
        })
    }

    /// All providers in stable `ProviderKind::ALL` order.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::FROM_KIND_LOOKUP
    }

    /// `ApiProvider` discriminant → `ProviderKind` lookup.
    /// Index 1 is `None` for the legacy `DeepseekCN` variant.
    const KIND_LOOKUP: [Option<codewhale_config::ProviderKind>; 31] = [
        Some(codewhale_config::ProviderKind::Deepseek),
        None, // DeepseekCN
        Some(codewhale_config::ProviderKind::DeepseekAnthropic),
        Some(codewhale_config::ProviderKind::NvidiaNim),
        Some(codewhale_config::ProviderKind::Openai),
        Some(codewhale_config::ProviderKind::Atlascloud),
        Some(codewhale_config::ProviderKind::WanjieArk),
        Some(codewhale_config::ProviderKind::Volcengine),
        Some(codewhale_config::ProviderKind::Openrouter),
        Some(codewhale_config::ProviderKind::XiaomiMimo),
        Some(codewhale_config::ProviderKind::Novita),
        Some(codewhale_config::ProviderKind::Fireworks),
        Some(codewhale_config::ProviderKind::Siliconflow),
        Some(codewhale_config::ProviderKind::SiliconflowCN),
        Some(codewhale_config::ProviderKind::Arcee),
        Some(codewhale_config::ProviderKind::Moonshot),
        Some(codewhale_config::ProviderKind::Sglang),
        Some(codewhale_config::ProviderKind::Vllm),
        Some(codewhale_config::ProviderKind::Ollama),
        Some(codewhale_config::ProviderKind::Huggingface),
        Some(codewhale_config::ProviderKind::Together),
        Some(codewhale_config::ProviderKind::Qianfan),
        Some(codewhale_config::ProviderKind::OpenaiCodex),
        Some(codewhale_config::ProviderKind::Anthropic),
        Some(codewhale_config::ProviderKind::Openmodel),
        Some(codewhale_config::ProviderKind::Zai),
        Some(codewhale_config::ProviderKind::Stepfun),
        Some(codewhale_config::ProviderKind::Minimax),
        Some(codewhale_config::ProviderKind::Deepinfra),
        Some(codewhale_config::ProviderKind::Sakana),
        Some(codewhale_config::ProviderKind::Custom),
    ];

    /// `ProviderKind` discriminant → `ApiProvider` lookup.
    const FROM_KIND_LOOKUP: [Self; 30] = [
        Self::Deepseek,
        Self::DeepseekAnthropic,
        Self::NvidiaNim,
        Self::Openai,
        Self::Atlascloud,
        Self::WanjieArk,
        Self::Volcengine,
        Self::Openrouter,
        Self::XiaomiMimo,
        Self::Novita,
        Self::Fireworks,
        Self::Siliconflow,
        Self::Arcee,
        Self::SiliconflowCn,
        Self::Moonshot,
        Self::Sglang,
        Self::Vllm,
        Self::Ollama,
        Self::Huggingface,
        Self::Together,
        Self::Qianfan,
        Self::OpenaiCodex,
        Self::Anthropic,
        Self::Openmodel,
        Self::Zai,
        Self::Stepfun,
        Self::Minimax,
        Self::Deepinfra,
        Self::Sakana,
        Self::Custom,
    ];

    /// Map to the config-level `ProviderKind`.
    /// Returns `None` for the legacy `DeepseekCN` variant.
    #[must_use]
    pub fn kind(self) -> Option<codewhale_config::ProviderKind> {
        Self::KIND_LOOKUP[self as usize]
    }

    /// Construct from a config-level `ProviderKind`.
    #[must_use]
    pub fn from_kind(kind: codewhale_config::ProviderKind) -> Self {
        Self::FROM_KIND_LOOKUP[kind as usize]
    }

    /// Whether this provider is a self-hosted / local runtime.
    ///
    /// These run without hosted authentication and keep traffic on the user's
    /// own infrastructure, so they carry a local/private posture. Used by the
    /// fallback chain to avoid silently routing a local/private primary out to
    /// a cloud provider (#2574) and by the `/provider` dashboard's self-hosted
    /// hint (#3083). Update this list whenever adding a provider whose runtime
    /// is hosted on the user's own infrastructure.
    #[must_use]
    pub fn is_self_hosted(self) -> bool {
        matches!(self, Self::Sglang | Self::Vllm | Self::Ollama)
    }
}

pub(crate) fn normalize_subagent_provider_key(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| match ch {
            '-' | '_' | '.' | ' ' => '_',
            _ => ch,
        })
        .collect()
}

pub(crate) fn subagent_provider_key_matches(key: &str, provider: ApiProvider) -> bool {
    if ApiProvider::parse(key).is_some_and(|candidate| candidate == provider) {
        return true;
    }

    let normalized = normalize_subagent_provider_key(key);
    if normalized == normalize_subagent_provider_key(provider.as_str()) {
        return true;
    }

    match provider {
        ApiProvider::Deepseek => matches!(
            normalized.as_str(),
            "deepseek" | "deepseek_api" | "deepseek_official"
        ),
        ApiProvider::DeepseekCN => matches!(
            normalized.as_str(),
            "deepseek_cn" | "deepseek_china" | "deepseekcn"
        ),
        ApiProvider::DeepseekAnthropic => matches!(
            normalized.as_str(),
            "deepseek_anthropic" | "deepseek_claude" | "deepseek_anthropic_api"
        ),
        ApiProvider::Openrouter => matches!(normalized.as_str(), "openrouter" | "open_router"),
        ApiProvider::OpenaiCodex => matches!(
            normalized.as_str(),
            "openai_codex" | "codex" | "chatgpt" | "openai_chatgpt"
        ),
        ApiProvider::Anthropic => {
            matches!(
                normalized.as_str(),
                "anthropic" | "claude" | "anthropic_api"
            )
        }
        ApiProvider::Zai => matches!(
            normalized.as_str(),
            "zai"
                | "z_ai"
                | "glm"
                | "zai_glm"
                | "z_glm"
                | "zhipu"
                | "zhipuai"
                | "bigmodel"
                | "big_model"
                | "zhipu_glm"
        ),
        _ => false,
    }
}
