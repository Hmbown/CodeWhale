//! Built-in provider metadata.
//!
//! This module is a metadata foundation for collapsing provider drift over
//! time. It deliberately does not mutate request bodies or choose fallback
//! providers; runtime routing remains in `ConfigToml::resolve_runtime_options`.

use super::{
    DEFAULT_ARCEE_BASE_URL, DEFAULT_ARCEE_MODEL, DEFAULT_ATLASCLOUD_BASE_URL,
    DEFAULT_ATLASCLOUD_MODEL, DEFAULT_DEEPSEEK_BASE_URL, DEFAULT_DEEPSEEK_MODEL,
    DEFAULT_FIREWORKS_BASE_URL, DEFAULT_FIREWORKS_MODEL, DEFAULT_HUGGINGFACE_BASE_URL,
    DEFAULT_HUGGINGFACE_MODEL, DEFAULT_MOONSHOT_BASE_URL, DEFAULT_MOONSHOT_MODEL,
    DEFAULT_NOVITA_BASE_URL, DEFAULT_NOVITA_MODEL, DEFAULT_NVIDIA_NIM_BASE_URL,
    DEFAULT_NVIDIA_NIM_MODEL, DEFAULT_OLLAMA_BASE_URL, DEFAULT_OLLAMA_MODEL,
    DEFAULT_OPENAI_BASE_URL, DEFAULT_OPENAI_CODEX_BASE_URL, DEFAULT_OPENAI_CODEX_MODEL,
    DEFAULT_OPENAI_MODEL, DEFAULT_OPENROUTER_BASE_URL, DEFAULT_OPENROUTER_MODEL,
    DEFAULT_SGLANG_BASE_URL, DEFAULT_SGLANG_MODEL, DEFAULT_SILICONFLOW_BASE_URL,
    DEFAULT_SILICONFLOW_CN_BASE_URL, DEFAULT_SILICONFLOW_MODEL, DEFAULT_STEPFUN_BASE_URL,
    DEFAULT_STEPFUN_MODEL, DEFAULT_TOGETHER_BASE_URL, DEFAULT_TOGETHER_MODEL,
    DEFAULT_VLLM_BASE_URL, DEFAULT_VLLM_MODEL, DEFAULT_VOLCENGINE_BASE_URL,
    DEFAULT_VOLCENGINE_MODEL, DEFAULT_WANJIE_ARK_BASE_URL, DEFAULT_WANJIE_ARK_MODEL,
    DEFAULT_XIAOMI_MIMO_BASE_URL, DEFAULT_XIAOMI_MIMO_MODEL, DEFAULT_ZAI_BASE_URL,
    DEFAULT_ZAI_MODEL, ProviderKind,
};

/// Wire protocol spoken by a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFormat {
    /// OpenAI-compatible `/v1/chat/completions` style payloads.
    ChatCompletions,
    /// OpenAI Responses API (`/responses`).
    Responses,
    /// Native Anthropic Messages API (`/v1/messages`).
    AnthropicMessages,
}

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
    aliases: []
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
    aliases: ["moonshot-ai", "kimi", "kimi-k2"]
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
    aliases: ["together-ai", "together_ai"]
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

provider!(
    Zai,
    Zai,
    "zai",
    "Z.ai (GLM Coding)",
    DEFAULT_ZAI_BASE_URL,
    DEFAULT_ZAI_MODEL,
    ["ZAI_API_KEY", "Z_AI_API_KEY"],
    "zai",
    aliases: ["z-ai", "z_ai", "z.ai"]
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

static DEEPSEEK: Deepseek = Deepseek;
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
static OPENAI_CODEX: OpenaiCodex = OpenaiCodex;
static ANTHROPIC: Anthropic = Anthropic;
static ZAI: Zai = Zai;
static STEPFUN: Stepfun = Stepfun;

static PROVIDER_REGISTRY: [&dyn Provider; 23] = [
    &DEEPSEEK,
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
    &OPENAI_CODEX,
    &ANTHROPIC,
    &ZAI,
    &STEPFUN,
];

/// Return all built-in provider metadata entries in `ProviderKind::ALL` order.
#[must_use]
pub fn all_providers() -> &'static [&'static dyn Provider] {
    &PROVIDER_REGISTRY
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
