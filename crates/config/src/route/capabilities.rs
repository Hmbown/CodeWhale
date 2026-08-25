//! Route-scoped capability facts.
//!
//! Capability state is deliberately three-valued: an absent catalog fact is
//! unknown, not unsupported, and must never be promoted to supported by a
//! transport/protocol heuristic. These values travel with the exact provider
//! offering selected by [`super::resolver::RouteResolver`].

use serde::{Deserialize, Serialize};

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
            "claude-opus-5"
                | "claude-fable-5"
                | "claude-opus-4-8"
                | "claude-opus-4-7"
                | "claude-opus-4-6"
                | "claude-sonnet-5"
                | "claude-sonnet-4-6"
        ),
        "xai" => matches!(wire_model_id.as_str(), "grok-4.6" | "grok-4.5"),
        _ => false,
    };
    if supported {
        CapabilityState::Supported
    } else {
        CapabilityState::Unknown
    }
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

        for (provider, model) in [
            ("openrouter", "openai/gpt-5.6"),
            ("custom", "gpt-5.6"),
            ("openai", "gpt-5.6-sol"),
            ("xai", "grok-4.6-fast"),
            ("xai", "grok-4.6-latest"),
            ("xai", "grok-4.5-fast"),
            ("anthropic", "claude-haiku-4-5"),
        ] {
            assert_eq!(
                documented_server_side_web_search(provider, model),
                CapabilityState::Unknown,
                "{provider}/{model} must not inherit a capability by similarity"
            );
        }
    }
}
