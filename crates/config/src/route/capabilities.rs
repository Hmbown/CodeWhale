//! Route-scoped capability facts.
//!
//! Capability state is deliberately three-valued: an absent catalog fact is
//! unknown, not unsupported, and must never be promoted to supported by a
//! transport/protocol heuristic. These values travel with the exact provider
//! offering selected by [`super::resolver::RouteResolver`].

use serde::{Deserialize, Serialize};

use crate::ProviderKind;

/// Whether a resolved provider/model offering supports one capability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    /// The selected offering explicitly reports support.
    Supported,
    /// The selected offering explicitly reports no support.
    Unsupported,
    /// The selected offering did not state the fact.
    #[default]
    Unknown,
}

impl CapabilityState {
    /// Preserve a sourced optional boolean as a three-state fact.
    #[must_use]
    pub const fn from_optional_bool(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Supported,
            Some(false) => Self::Unsupported,
            None => Self::Unknown,
        }
    }

    /// Whether the source explicitly reports support.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

/// Return the documented server-side web-search fact for one exact direct
/// provider/model offering.
///
/// This is intentionally a small sourced table, not a protocol or model-family
/// heuristic. Aggregators, custom endpoints, aliases, snapshots, and nearby
/// model names remain [`CapabilityState::Unknown`] until a provider-owned fact
/// exists for that exact offering.
///
/// Sources:
/// - OpenAI Responses web search: <https://developers.openai.com/api/docs/guides/tools-web-search>
/// - Anthropic web search tool: <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool>
/// - xAI web search tool: <https://docs.x.ai/developers/tools/web-search>
/// - Xiaomi MiMo web search: <https://mimo.mi.com/docs/en-US/usage-guide/tool-calling/web-search>
/// - Z.AI Web Search API: <https://docs.z.ai/api-reference/tools/web-search>
/// - Zhipu Web Search API: <https://docs.bigmodel.cn/api-reference/工具-api/网络搜索>
/// - Alibaba Model Studio Token Plan Harness tools: <https://help.aliyun.com/en/model-studio/token-plan-harness-tool>
/// - DeepSeek Responses web search: <https://api-docs.deepseek.com/api/create-response/>
/// - Kimi built-in and Formula web search: <https://platform.kimi.ai/docs/guide/use-web-search>
#[must_use]
pub(crate) fn documented_server_side_web_search(
    provider_id: &str,
    wire_model_id: &str,
) -> CapabilityState {
    let provider_id = provider_id.trim().to_ascii_lowercase();
    let wire_model_id = wire_model_id.trim().to_ascii_lowercase();
    let supported = match provider_id.as_str() {
        "openai" => matches!(
            wire_model_id.as_str(),
            "gpt-5.6" | "gpt-5.5" | "gpt-5.4" | "gpt-4.1" | "gpt-4.1-mini" | "o4-mini"
        ),
        "anthropic" => matches!(
            wire_model_id.as_str(),
            "claude-fable-5"
                | "claude-opus-4-8"
                | "claude-mythos-5"
                | "claude-mythos-preview"
                | "claude-opus-4-7"
                | "claude-opus-4-6"
                | "claude-sonnet-5"
                | "claude-sonnet-4-6"
        ),
        "xai" => matches!(wire_model_id.as_str(), "grok-4.6" | "grok-4.5"),
        "xiaomi-mimo" => matches!(wire_model_id.as_str(), "mimo-v2.5-pro" | "mimo-v2.5"),
        "zai" => matches!(
            wire_model_id.as_str(),
            "glm-5.3" | "glm-5.3-flash" | "glm-5.2" | "glm-5.1" | "glm-5-turbo"
        ),
        "modelstudio-token-plan" => matches!(
            wire_model_id.as_str(),
            "qwen3.8-max" | "qwen3.7-plus" | "qwen3.7-max"
        ),
        "deepseek" => matches!(
            wire_model_id.as_str(),
            "deepseek-v4-flash" | "deepseek-v4-pro" | "deepseek-v4-flash-vision-exp"
        ),
        "moonshot" => matches!(wire_model_id.as_str(), "kimi-k3" | "kimi-k2.6"),
        _ => false,
    };
    if supported {
        CapabilityState::Supported
    } else {
        CapabilityState::Unknown
    }
}

/// Return the Z.AI/Zhipu search fact only for the two exact general API
/// products that expose the structured `/web_search` endpoint.
#[must_use]
pub(crate) fn documented_zai_web_search_for_route(
    provider: ProviderKind,
    wire_model_id: &str,
    base_url: &str,
) -> CapabilityState {
    if provider != ProviderKind::Zai {
        return CapabilityState::Unknown;
    }
    let normalized = base_url.trim().trim_end_matches('/').to_ascii_lowercase();
    if !matches!(
        normalized.as_str(),
        "https://api.z.ai/api/paas/v4" | "https://open.bigmodel.cn/api/paas/v4"
    ) {
        return CapabilityState::Unknown;
    }
    documented_server_side_web_search("zai", wire_model_id)
}

/// Return the native-search fact for exact Moonshot direct and Kimi Code
/// product routes. Adjacent coding paths and cross-product model ids remain
/// unknown even though they share one provider identity.
#[must_use]
pub(crate) fn documented_moonshot_web_search_for_route(
    provider: ProviderKind,
    wire_model_id: &str,
    base_url: &str,
) -> CapabilityState {
    if provider != ProviderKind::Moonshot {
        return CapabilityState::Unknown;
    }
    let model = wire_model_id.trim().to_ascii_lowercase();
    if crate::provider::is_exact_kimi_code_route(provider, base_url)
        && matches!(
            model.as_str(),
            "k3" | "k3-256k" | "kimi-for-coding" | "kimi-for-coding-highspeed"
        )
    {
        return CapabilityState::Supported;
    }
    if crate::provider::is_exact_moonshot_platform_route(provider, base_url) {
        return documented_server_side_web_search("moonshot", &model);
    }
    CapabilityState::Unknown
}

/// Capability facts owned by one provider/model route offering.
///
/// Fields without a current authoritative catalog source remain `Unknown`.
/// They are present now so live/provider-native facts can be added without
/// changing the candidate contract or guessing from request protocol.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteCapabilities {
    #[serde(default)]
    pub attachments: CapabilityState,
    /// Whether the exact offering explicitly accepts image input.
    #[serde(default)]
    pub image_input: CapabilityState,
    #[serde(default)]
    pub reasoning: CapabilityState,
    #[serde(default)]
    pub native_tool_calls: CapabilityState,
    #[serde(default)]
    pub structured_output: CapabilityState,
    #[serde(default)]
    pub parallel_tool_calls: CapabilityState,
    #[serde(default)]
    pub streaming: CapabilityState,
    #[serde(default)]
    pub prompt_caching: CapabilityState,
    #[serde(default)]
    pub server_side_web_search: CapabilityState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_KIMI_CODE_BASE_URL;

    #[test]
    fn optional_boolean_preserves_unknown_and_false() {
        assert_eq!(
            CapabilityState::from_optional_bool(None),
            CapabilityState::Unknown
        );
        assert_eq!(
            CapabilityState::from_optional_bool(Some(false)),
            CapabilityState::Unsupported
        );
        assert_eq!(
            CapabilityState::from_optional_bool(Some(true)),
            CapabilityState::Supported
        );
    }

    #[test]
    fn unsourced_route_capabilities_default_to_unknown() {
        let capabilities = RouteCapabilities::default();
        assert_eq!(capabilities.streaming, CapabilityState::Unknown);
        assert_eq!(
            capabilities.server_side_web_search,
            CapabilityState::Unknown
        );
    }

    #[test]
    fn documented_web_search_is_exact_and_provider_owned() {
        assert_eq!(
            documented_server_side_web_search("xai", "grok-4.6"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("xai", "grok-4.5"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("openai", "gpt-5.6"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("anthropic", "claude-sonnet-4-6"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("xiaomi-mimo", "mimo-v2.5-pro"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("zai", "GLM-5.3"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("modelstudio-token-plan", "qwen3.8-max"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("deepseek", "deepseek-v4-flash"),
            CapabilityState::Supported
        );
        assert_eq!(
            documented_server_side_web_search("moonshot", "kimi-k3"),
            CapabilityState::Supported
        );

        for (provider, model) in [
            ("openrouter", "openai/gpt-5.6"),
            ("custom", "gpt-5.6"),
            ("openai", "gpt-5.6-sol"),
            ("xai", "grok-4.6-fast"),
            ("xai", "grok-4.6-latest"),
            ("xai", "grok-4.5-fast"),
            ("anthropic", "claude-haiku-4-5"),
            ("xiaomi-mimo", "mimo-v2.5-pro-ultraspeed"),
            ("zai", "glm-5.3-preview"),
            ("modelstudio-coding-plan", "qwen3.8-max"),
            ("modelstudio-token-plan", "qwen3.8-max-preview"),
            ("deepseek", "deepseek-v4-flash-preview"),
            ("moonshot", "kimi-k2.7-code"),
        ] {
            assert_eq!(
                documented_server_side_web_search(provider, model),
                CapabilityState::Unknown,
                "{provider}/{model} must not inherit a capability by similarity"
            );
        }
    }

    #[test]
    fn zai_route_fact_rejects_coding_and_neighboring_endpoints() {
        for base_url in [
            "https://api.z.ai/api/paas/v4",
            "https://open.bigmodel.cn/api/paas/v4/",
        ] {
            assert_eq!(
                documented_zai_web_search_for_route(ProviderKind::Zai, "GLM-5.3", base_url),
                CapabilityState::Supported
            );
        }
        for base_url in [
            "https://api.z.ai/api/coding/paas/v4",
            "https://open.bigmodel.cn/api/paas/v4/preview",
            "https://gateway.example.test/v4",
        ] {
            assert_eq!(
                documented_zai_web_search_for_route(ProviderKind::Zai, "GLM-5.3", base_url),
                CapabilityState::Unknown
            );
        }
    }

    #[test]
    fn moonshot_route_fact_is_exact_to_product_and_model() {
        for (model, base_url) in [
            ("kimi-k3", "https://api.moonshot.ai/v1"),
            ("kimi-k3", "https://api.moonshot.cn/v1"),
            ("kimi-k2.6", "https://api.moonshot.ai/v1"),
            ("k3", DEFAULT_KIMI_CODE_BASE_URL),
            ("kimi-for-coding", DEFAULT_KIMI_CODE_BASE_URL),
        ] {
            assert_eq!(
                documented_moonshot_web_search_for_route(ProviderKind::Moonshot, model, base_url,),
                CapabilityState::Supported
            );
        }
        for (model, base_url) in [
            ("kimi-k3", "https://api.kimi.com/coding/v2"),
            ("kimi-k2.6", "https://api.kimi.com/coding/v1/preview"),
            ("k3", "https://api.moonshot.ai/v1"),
        ] {
            assert_eq!(
                documented_moonshot_web_search_for_route(ProviderKind::Moonshot, model, base_url,),
                CapabilityState::Unknown
            );
        }
    }
}
