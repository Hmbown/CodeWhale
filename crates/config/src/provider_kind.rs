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
    #[serde(alias = "orca_router", alias = "orca")]
    Orcarouter,
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
    #[serde(alias = "ollama_cloud")]
    OllamaCloud,
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
    #[serde(alias = "opencode_go", alias = "opencodego")]
    OpencodeGo,
    #[serde(
        alias = "opencode_zen",
        alias = "opencodezen",
        alias = "zen",
        alias = "opencode"
    )]
    OpencodeZen,
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
    /// Mistral AI — la Plateforme (OpenAI-compatible Chat Completions).
    #[serde(
        alias = "mistral-ai",
        alias = "mistral_ai",
        alias = "mistralai",
        alias = "la-plateforme",
        alias = "la_plateforme"
    )]
    Mistral,
    /// Jiangsu Telecom TokenHub (OpenAI-compatible).
    ///
    /// An AI gateway operated by Jiangsu Telecom that speaks the OpenAI Chat
    /// Completions wire protocol and serves a broad model catalog; each API key
    /// may access a different subset of models.
    #[serde(
        alias = "telecom-js",
        alias = "telecom_js",
        alias = "telecomjs-cn",
        alias = "tokenhub"
    )]
    Telecomjs,
    /// Alibaba Cloud Model Studio — Token Plan (OpenAI-compatible Chat Completions).
    ///
    /// Token Plan Personal and Team share the same endpoint. Both the OpenAI
    /// and Anthropic dialects are available; select the Anthropic dialect via
    /// `modelstudio-token-plan-anthropic`. Pay-as-you-go workspace-id templating
    /// is out of scope for v1; use a custom provider for that plan.
    #[serde(
        alias = "modelstudio-token-plan",
        alias = "modelstudio_token_plan",
        alias = "alibaba-token-plan",
        alias = "dashscope-token-plan"
    )]
    ModelstudioTokenPlan,
    /// Alibaba Cloud Model Studio — Token Plan Anthropic-compatible endpoint.
    #[serde(
        alias = "modelstudio-token-plan-anthropic",
        alias = "modelstudio_token_plan_anthropic",
        alias = "alibaba-token-plan-anthropic"
    )]
    ModelstudioTokenPlanAnthropic,
    /// Alibaba Cloud Model Studio — Coding Plan (OpenAI-compatible Chat Completions).
    #[serde(
        alias = "modelstudio-coding-plan",
        alias = "modelstudio_coding_plan",
        alias = "alibaba-coding-plan",
        alias = "dashscope-coding-plan"
    )]
    ModelstudioCodingPlan,
    /// Alibaba Cloud Model Studio — Coding Plan Anthropic-compatible endpoint.
    #[serde(
        alias = "modelstudio-coding-plan-anthropic",
        alias = "modelstudio_coding_plan_anthropic",
        alias = "alibaba-coding-plan-anthropic"
    )]
    ModelstudioCodingPlanAnthropic,
    /// Google Antigravity (`agy` CLI) — consent-gated read-only credential
    /// import only; the cloud-code wire protocol is not implemented and
    /// requests fail closed with an actionable message.
    #[serde(alias = "agy")]
    Antigravity,
    /// Google — Gemini OpenAI-compatible endpoint. Its own backend, not an
    /// OpenAI alias: thought signatures on tool calls are captured and
    /// replayed per Google's contract.
    #[serde(
        alias = "google-gemini",
        alias = "google_gemini",
        alias = "gemini",
        alias = "google-ai",
        alias = "google_ai",
        alias = "ai-studio",
        alias = "aistudio"
    )]
    Google,
    /// Eden AI — OpenAI-compatible AI gateway (aggregator).
    ///
    /// Serves a broad catalog of upstream models under `provider/model`
    /// namespaced wire ids over the OpenAI Chat Completions protocol.
    #[serde(alias = "eden-ai", alias = "eden_ai", alias = "edenai")]
    Edenai,
    /// Concentrate — OpenAI Responses-compatible AI gateway (aggregator).
    ///
    /// Serves a broad catalog of upstream models over the OpenAI Responses
    /// protocol at `/v1/responses` with a bearer Universal API key. Model ids
    /// pass through verbatim: a plain catalog id (`gpt-5.6-sol`) lets the
    /// gateway choose the upstream provider, `provider/model` pins one, and
    /// `concentrate/auto` reaches the gateway's own `auto` router. Opt-in and
    /// BYOK only: the key lives in the local secret store and Codewhale adds
    /// no fee, no managed default, and no resale lane (see docs/PROVIDERS.md).
    #[serde(
        alias = "concentrate-ai",
        alias = "concentrate_ai",
        alias = "concentrateai"
    )]
    Concentrate,
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
    /// Catalog / picker surface: one identity per vendor.
    ///
    /// Dual-wire dialect kinds (`*Anthropic`) and Model Studio plan variants
    /// stay on the enum for serde and `provider_for_kind`, but they are not
    /// first-class catalog rows. Plan is `mode` / base_url; dialect is
    /// `wire = openai|anthropic` on the primary provider config.
    pub const ALL: [Self; 43] = [
        Self::Deepseek,
        Self::NvidiaNim,
        Self::Openai,
        Self::Atlascloud,
        Self::WanjieArk,
        Self::Volcengine,
        Self::Openrouter,
        Self::Orcarouter,
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
        Self::OllamaCloud,
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
        Self::LongCat,
        Self::OpencodeGo,
        Self::OpencodeZen,
        Self::Meta,
        Self::Xai,
        Self::Mistral,
        Self::Telecomjs,
        Self::ModelstudioTokenPlan,
        Self::Google,
        Self::Antigravity,
        Self::Edenai,
        Self::Concentrate,
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

    /// Parse a provider identifier for **config-table identity** — the kind
    /// used to look up credentials, model, and base URL in the user's
    /// `[providers.*]` tables.
    ///
    /// [`parse`](Self::parse) is *catalog* identity: legacy dual-wire
    /// spellings (`deepseek-anthropic`, `minimax-anthropic`, the Model Studio
    /// plan/dialect kinds) are aliases of the vendor primary and collapse
    /// onto it so pickers show one row per vendor. That collapse must not
    /// decide which config table holds the user's credentials: TOML serde
    /// keeps the legacy kind for `provider = "deepseek-anthropic"`, so env
    /// (`CODEWHALE_PROVIDER`) and `config set provider` must resolve the same
    /// way or the user's own named table is orphaned with the key present.
    ///
    /// An exact canonical-id or `provider_config_key` match across the full
    /// registry (including legacy dialect/plan kinds) therefore wins over
    /// alias collapse; everything else falls back to [`parse`](Self::parse).
    /// Wire-endpoint selection is unaffected: it keys off the resolved kind's
    /// `wire` config, not this parse.
    #[must_use]
    pub fn parse_config_identity(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        provider::all_providers()
            .iter()
            .find(|p| {
                trimmed.eq_ignore_ascii_case(p.id())
                    || trimmed.eq_ignore_ascii_case(p.provider_config_key())
            })
            .map(|p| p.kind())
            .or_else(|| Self::parse(trimmed))
    }

    #[must_use]
    pub fn is_siliconflow(self) -> bool {
        matches!(self, Self::Siliconflow | Self::SiliconflowCN)
    }

    /// Canonical durable-credential slot in the local secret store.
    ///
    /// Most providers own a slot named after their id. Variants authenticated
    /// by the SAME account share one slot so a single saved key (or logout)
    /// applies to the whole family:
    ///
    /// - `SiliconflowCN` shares `siliconflow` (historical China-endpoint slot,
    ///   already the TUI/CLI convention).
    /// - The four Alibaba Cloud Model Studio variants share
    ///   `modelstudio-token-plan`: one Model Studio account/key authenticates
    ///   the Token Plan and Coding Plan endpoints in both wire dialects, so
    ///   per-variant slots produced three bogus "missing key" rows whenever
    ///   one variant held the key.
    #[must_use]
    pub fn secret_store_slot(self) -> &'static str {
        match self {
            Self::SiliconflowCN => "siliconflow",
            Self::ModelstudioTokenPlan
            | Self::ModelstudioTokenPlanAnthropic
            | Self::ModelstudioCodingPlan
            | Self::ModelstudioCodingPlanAnthropic => "modelstudio-token-plan",
            _ => self.as_str(),
        }
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
