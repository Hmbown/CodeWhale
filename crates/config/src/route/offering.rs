//! Provider model offerings (#3084).
//!
//! A [`ProviderModelOffering`] binds a provider to a canonical model, the
//! provider-owned wire id that serves it, and the endpoint key. This is the
//! seam that proves the #2608 invariant: the SAME canonical model can be served
//! by multiple providers under DIFFERENT wire ids (some aggregator-prefixed),
//! and a prefix never implies provider ownership.
//!
//! Catalog-derived offerings from [`crate::catalog::bundled_catalog_offerings`]
//! remain the general bundled source of truth. [`bundled_offerings`] contains
//! only transport facts that Models.dev cannot express, such as a single
//! provider routing different models over different wire protocols.

use serde::{Deserialize, Serialize};

use super::candidate::PricingSku;
use super::ids::{ModelId, ProviderId, WireModelId};

/// Token limits for one resolved route/offering.
///
/// These are optional because hosted catalogs, local runtimes, and custom
/// endpoints can legitimately omit some or all limit facts. Callers should
/// treat `None` as unknown, not zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteLimits {
    /// Total context window (input + output), in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Input-token limit, when the provider reports it separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Output-token cap for the route/offering, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
}

impl RouteLimits {
    /// Whether at least one limit fact is known.
    #[must_use]
    pub const fn has_known_limit(self) -> bool {
        self.context_tokens.is_some() || self.input_tokens.is_some() || self.output_tokens.is_some()
    }
}

/// One provider's way of serving a (possibly canonical) model.
///
/// `Eq` is intentionally NOT derived: [`PricingSku::Token`] carries `f64` rates,
/// so the offering is only `PartialEq`. No caller keys a set/map on offerings.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderModelOffering {
    /// Provider serving this offering.
    pub provider: ProviderId,
    /// Canonical model identity, if this offering maps to one.
    pub canonical_model: Option<ModelId>,
    /// Provider-owned wire id sent on the request (verbatim).
    pub wire_model_id: WireModelId,
    /// Endpoint key the offering is served on.
    pub endpoint_key: String,
    /// Whether this is the provider's default offering.
    pub default_for_provider: bool,
    /// Provider/offering-scoped token limits, when known.
    pub limits: RouteLimits,
    /// Coarse route-facing pricing meter for this offering (#3085).
    ///
    /// Projected from the offering's sourced cost at the layer that owns it
    /// (`CatalogOffering::to_offering` → [`crate::pricing::route_pricing_sku`]).
    /// The resolver carries this verbatim onto the candidate; it is
    /// [`PricingSku::UnknownOrStale`] whenever no price was sourced — never a
    /// fabricated zero (the #2608 / #3085 honesty rule).
    pub pricing: PricingSku,
}

// Transport snapshot verified against https://opencode.ai/docs/zen on
// 2026-07-17. Gemini rows are intentionally absent because they use Google's
// model-specific wire protocol, which CodeWhale does not currently implement.
pub(crate) const OPENCODE_ZEN_RESPONSES_MODELS: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.5-pro",
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.4-mini",
    "gpt-5.4-nano",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2",
    "gpt-5.2-codex",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex-mini",
    "gpt-5",
    "gpt-5-codex",
    "gpt-5-nano",
];

pub(crate) const OPENCODE_ZEN_MESSAGES_MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-opus-4-5",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
    "qwen3.7-max",
    "qwen3.7-plus",
    "qwen3.6-plus",
    "qwen3.5-plus",
];

pub(crate) const OPENCODE_ZEN_CHAT_MODELS: &[&str] = &[
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "minimax-m3",
    "minimax-m2.7",
    "minimax-m2.5",
    "glm-5.2",
    "glm-5.1",
    "glm-5",
    "kimi-k2.5",
    "kimi-k2.6",
    "kimi-k2.7-code",
    "grok-4.5",
    "grok-build-0.1",
    "big-pickle",
    "mimo-v2.5-free",
    "north-mini-code-free",
    "nemotron-3-ultra-free",
    "deepseek-v4-flash-free",
];

/// Return curated provider/model transport facts as owned offering rows.
///
/// OpenCode Zen's official catalog serves models over three protocol families.
/// These rows intentionally carry no inferred limits, pricing, or canonical
/// identity: their sole claim is the documented wire model and endpoint key.
#[must_use]
pub fn bundled_offerings() -> Vec<ProviderModelOffering> {
    let provider = ProviderId::from("opencode-zen");
    let groups = [
        ("responses", OPENCODE_ZEN_RESPONSES_MODELS),
        ("messages", OPENCODE_ZEN_MESSAGES_MODELS),
        ("chat", OPENCODE_ZEN_CHAT_MODELS),
    ];

    groups
        .into_iter()
        .flat_map(|(endpoint_key, models)| {
            let provider = provider.clone();
            models.iter().map(move |model| ProviderModelOffering {
                provider: provider.clone(),
                canonical_model: None,
                wire_model_id: WireModelId::from(*model),
                endpoint_key: endpoint_key.to_string(),
                default_for_provider: *model == "gpt-5.5",
                limits: RouteLimits::default(),
                pricing: PricingSku::UnknownOrStale,
            })
        })
        .collect()
}
