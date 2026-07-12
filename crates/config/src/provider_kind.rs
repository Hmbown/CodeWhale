//! The canonical [`ProviderKind`] enum (#3311): the set of built-in provider
//! kinds, their serde aliases, and identity helpers (`all`, `as_str`, `parse`,
//! `provider`). Extracted verbatim from `lib.rs` to separate provider identity
//! from config schema/loading; re-exported at the crate root so
//! `codewhale_config::ProviderKind` is unchanged. Behavior is identical.

use serde::{Deserialize, Serialize};

use crate::provider;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    #[default]
    #[serde(
        alias = "deepseek-cn",
        alias = "deepseek_china",
        alias = "deepseekcn",
        alias = "deepseek-china"
    )]
    Deepseek,
    #[serde(
        alias = "deepseek-anthropic",
        alias = "deepseek_anthropic",
        alias = "deepseek-claude",
        alias = "deepseek_claude"
    )]
    DeepseekAnthropic,
    NvidiaNim,
    #[serde(alias = "open-ai")]
    Openai,
    Atlascloud,
    #[serde(
        alias = "wanjie",
        alias = "wanjie_ark",
        alias = "ark-wanjie",
        alias = "ark_wanjie",
        alias = "wanjie-maas",
        alias = "wanjie_maas"
    )]
    WanjieArk,
    #[serde(alias = "volcengine-ark", alias = "volcengine_ark", alias = "ark")]
    Volcengine,
    Openrouter,
    #[serde(alias = "mimo", alias = "xiaomi", alias = "xiaomi_mimo")]
    XiaomiMimo,
    #[serde(alias = "novita-ai", alias = "novita_ai")]
    Novita,
    #[serde(alias = "fireworks-ai", alias = "fireworks_ai")]
    Fireworks,
    #[serde(alias = "silicon-flow", alias = "silicon_flow")]
    Siliconflow,
    #[serde(alias = "arcee-ai", alias = "arcee_ai")]
    Arcee,
    #[serde(alias = "siliconflow-cn", alias = "siliconflow-CN")]
    SiliconflowCN,
    #[serde(alias = "moonshot-ai", alias = "moonshotai", alias = "moonshot_ai")]
    Moonshot,
    Sglang,
    Vllm,
    Ollama,
    #[serde(alias = "hugging-face", alias = "hugging_face", alias = "hf")]
    Huggingface,
    #[serde(alias = "together-ai", alias = "together_ai", alias = "togetherai")]
    Together,
    #[serde(alias = "baidu-qianfan", alias = "baidu_qianfan", alias = "baidu")]
    Qianfan,
    #[serde(
        alias = "openai-codex",
        alias = "openai_codex",
        alias = "codex",
        alias = "chatgpt",
        alias = "chatgpt-codex",
        alias = "chatgpt_codex"
    )]
    OpenaiCodex,
    #[serde(alias = "claude")]
    Anthropic,
    #[serde(alias = "open-model", alias = "open_model")]
    Openmodel,
    #[serde(
        alias = "z-ai",
        alias = "z_ai",
        alias = "z.ai",
        alias = "zhipu",
        alias = "zhipuai",
        alias = "bigmodel",
        alias = "big-model"
    )]
    Zai,
    #[serde(
        alias = "step-fun",
        alias = "step_fun",
        alias = "stepfun",
        alias = "stepflash",
        alias = "step-flash",
        alias = "step_flash"
    )]
    Stepfun,
    #[serde(alias = "mini-max", alias = "mini_max", alias = "minimax")]
    Minimax,
    #[serde(
        alias = "minimax_anthropic",
        alias = "mini-max-anthropic",
        alias = "mini_max_anthropic"
    )]
    MinimaxAnthropic,
    #[serde(alias = "deep-infra", alias = "deep_infra")]
    Deepinfra,
    #[serde(alias = "sakana-ai", alias = "sakana_ai", alias = "fugu")]
    Sakana,
    #[serde(alias = "long-cat", alias = "meituan-longcat", alias = "meituan")]
    LongCat,
    #[serde(
        alias = "meta-ai",
        alias = "meta_ai",
        alias = "meta-model-api",
        alias = "meta_model_api",
        alias = "muse",
        alias = "muse-spark"
    )]
    Meta,
    #[serde(alias = "x-ai", alias = "x_ai", alias = "grok")]
    Xai,
    /// User-defined OpenAI-compatible endpoint (#1519).
    ///
    /// A single dynamic identity for arbitrary `[providers.<name>]
    /// kind="openai-compatible"` entries. It speaks the OpenAI Chat Completions
    /// wire protocol and carries no built-in base URL/model — the concrete
    /// endpoint and model arrive via config (`base_url` / `model`) and the
    /// route's `base_url_override`, never from this static descriptor.
    Custom,
}

impl ProviderKind {
    pub const ALL: [Self; 34] = [
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
        Self::SiliconflowCN,
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
        Self::MinimaxAnthropic,
        Self::Deepinfra,
        Self::Sakana,
        Self::LongCat,
        Self::Meta,
        Self::Xai,
        Self::Custom,
    ];

    #[must_use]
    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    #[must_use]
    pub fn names_hint() -> String {
        Self::all()
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.provider().id()
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        provider::all_providers()
            .iter()
            .find(|p| {
                trimmed.eq_ignore_ascii_case(p.id())
                    || p.aliases().iter().any(|a| trimmed.eq_ignore_ascii_case(a))
            })
            .map(|p| p.kind())
    }

    #[must_use]
    pub fn is_siliconflow(self) -> bool {
        matches!(self, Self::Siliconflow | Self::SiliconflowCN)
    }

    /// Return the built-in metadata entry for this provider.
    ///
    /// This is a metadata foundation only; runtime routing still resolves
    /// through [`crate::ConfigToml::resolve_runtime_options`].
    #[must_use]
    pub fn provider(self) -> &'static dyn provider::Provider {
        provider::provider_for_kind(self)
    }
}
