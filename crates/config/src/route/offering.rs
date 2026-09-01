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
use super::capabilities::{CapabilityState, RouteCapabilities};
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
    /// Provider/model-scoped capability facts. Unknown is preserved rather
    /// than inferred from the wire protocol.
    pub capabilities: RouteCapabilities,
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
/// Token Plan text models (Text Generation / Reasoning, coding scope).
///
/// Available on both Token Plan Personal and Team. The same model set is also
/// available on the Coding Plan; rows are duplicated per provider id below.
/// Pay-as-you-go workspace-id templating is deferred to a follow-up.
const MODELSTUDIO_TEXT_MODELS: &[&str] = &[
    "qwen3.8-max",
    "qwen3.8-max-preview",
    "qwen3.7-plus",
    "qwen3.7-max",
    "qwen3.6-flash",
    // DeepSeek models served under Model Studio are scoped to this provider;
    // they do not collide with first-party DeepSeek routes.
    "deepseek-v4-pro",
    "deepseek-v4-flash-0731",
    // GLM models served under Model Studio are scoped to this provider;
    // they do not collide with first-party Zhipu / Z.ai routes.
    //
    // glm-5.3 is deliberately absent (2026-08-03): this list is a curated
    // snapshot of what Model Studio's upstream roster actually serves, and
    // Model Studio publishes no glm-5.3 entry. The direct Z.ai / OpenRouter
    // glm-5.3 rows inherit their metadata from glm-5.2, but metadata
    // inheritance is not evidence that a third-party gateway carries the
    // model. Add it here only against a Model Studio console/roster listing.
    "glm-5.2",
];

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
    // Muse Spark via OpenCode Zen gateway — Responses-only (reported
    // 2026-08-29: muse-spark-1.2-contributor-free rejects Chat Completions).
    "muse-spark-1.2",
    "muse-spark-1.2-contributor",
    "muse-spark-1.2-contributor-free",
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
    // glm-5.3 is deliberately absent (2026-08-03): this snapshot tracks the
    // official OpenCode Zen endpoint table, which lists no glm-5.3 row. Zen
    // fails closed on unknown models by design; registering a route Zen does
    // not serve would convert that into a guaranteed upstream 404.
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

/// Logical default plus every documented Zen wire id, for picker fallbacks
/// when Models.dev is stale or failed. `gpt-5.6` is the user-facing default;
/// `gpt-5.6-sol` is the proven Responses wire id.
#[must_use]
pub fn opencode_zen_picker_models() -> Vec<&'static str> {
    let mut models = vec![crate::DEFAULT_OPENCODE_ZEN_MODEL];
    for model in OPENCODE_ZEN_RESPONSES_MODELS
        .iter()
        .chain(OPENCODE_ZEN_MESSAGES_MODELS)
        .chain(OPENCODE_ZEN_CHAT_MODELS)
    {
        if !models
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(model))
        {
            models.push(*model);
        }
    }
    models
}

/// Return curated provider/model transport facts as owned offering rows.
///
/// OpenCode Zen's official catalog serves models over three protocol families.
/// These rows intentionally carry no inferred limits, pricing, or canonical
/// identity: their sole claim is the documented wire model and endpoint key.
#[must_use]
pub fn bundled_offerings() -> Vec<ProviderModelOffering> {
    // DeepSeek's 2026-07-31 production Flash update added a native Responses
    // endpoint without changing the model id. Pro remains Chat Completions
    // until its announced Responses rollout. These exact-route transport facts
    // cannot be represented by the Models.dev-shaped fallback asset.
    let deepseek = ProviderId::from("deepseek");
    let documented_capabilities = RouteCapabilities {
        image_input: CapabilityState::Unsupported,
        reasoning: CapabilityState::Supported,
        native_tool_calls: CapabilityState::Supported,
        structured_output: CapabilityState::Supported,
        parallel_tool_calls: CapabilityState::Supported,
        streaming: CapabilityState::Supported,
        prompt_caching: CapabilityState::Supported,
        // Search execution uses a separate bounded Responses request rather
        // than replaying `web_search_call` items in the main chat.
        server_side_web_search: CapabilityState::Supported,
        ..RouteCapabilities::default()
    };
    let documented_limits = RouteLimits {
        context_tokens: Some(1_000_000),
        input_tokens: None,
        output_tokens: Some(384_000),
    };
    let mut offerings = vec![
        ProviderModelOffering {
            provider: deepseek.clone(),
            canonical_model: Some(ModelId::from("deepseek-v4-pro")),
            wire_model_id: WireModelId::from("deepseek-v4-pro"),
            endpoint_key: "chat".to_string(),
            default_for_provider: true,
            limits: documented_limits,
            capabilities: documented_capabilities,
            pricing: PricingSku::UnknownOrStale,
        },
        ProviderModelOffering {
            provider: deepseek.clone(),
            canonical_model: Some(ModelId::from("deepseek-v4-flash")),
            wire_model_id: WireModelId::from("deepseek-v4-flash"),
            endpoint_key: "responses".to_string(),
            default_for_provider: false,
            limits: documented_limits,
            capabilities: documented_capabilities,
            pricing: PricingSku::UnknownOrStale,
        },
        // Vision-experimental sibling of v4-flash, verified live on
        // api.deepseek.com /models (2026-08-21). Image input is the one
        // documented difference; limits inherit the v4-flash row until
        // DeepSeek publishes distinct numbers.
        ProviderModelOffering {
            provider: deepseek,
            canonical_model: Some(ModelId::from("deepseek-v4-flash-vision-exp")),
            wire_model_id: WireModelId::from("deepseek-v4-flash-vision-exp"),
            endpoint_key: "chat".to_string(),
            default_for_provider: false,
            limits: documented_limits,
            capabilities: RouteCapabilities {
                image_input: CapabilityState::Supported,
                ..documented_capabilities
            },
            pricing: PricingSku::UnknownOrStale,
        },
    ];

    let provider = ProviderId::from("opencode-zen");
    let groups = [
        ("responses", OPENCODE_ZEN_RESPONSES_MODELS),
        ("messages", OPENCODE_ZEN_MESSAGES_MODELS),
        ("chat", OPENCODE_ZEN_CHAT_MODELS),
    ];

    offerings.extend(groups.into_iter().flat_map(|(endpoint_key, models)| {
        let provider = provider.clone();
        models.iter().map(move |model| ProviderModelOffering {
            provider: provider.clone(),
            // The bundled catalog exposes `gpt-5.6` as the user-facing
            // logical choice and records `gpt-5.6-sol` as its proven Zen
            // wire id. Keep the generic choice honest by resolving it to the
            // documented concrete Responses model rather than sending an
            // unproven generic wire id to Zen.
            canonical_model: (*model == "gpt-5.6-sol").then(|| ModelId::from("gpt-5.6")),
            wire_model_id: WireModelId::from(*model),
            endpoint_key: endpoint_key.to_string(),
            default_for_provider: *model == "gpt-5.6-sol",
            limits: RouteLimits::default(),
            capabilities: RouteCapabilities::default(),
            pricing: PricingSku::UnknownOrStale,
        })
    }));

    // Alibaba Cloud Model Studio — one vendor identity in the hand seam
    // (`modelstudio-token-plan`). Plan (token vs coding) and wire dialect
    // (OpenAI Chat Completions vs Anthropic Messages) are config (`mode` /
    // `wire`), not separate ProviderKinds — same product shape as Z.ai /
    // Xiaomi for plans and a power-user toggle for dialect. Legacy provider
    // ids still get catalog rows so old configs resolve, but the picker
    // catalog surface only lists the primary id.
    //
    // Limits: owner's Token Plan console + curated models_dev rows
    // (2026-08-03): qwen3.8-max is ~1M context / 128K output, NOT 128K
    // total. Empty RouteLimits here used to win identity collisions over
    // the asset catalog and fall through to the 128K legacy default.
    fn ms_capabilities(model: &str) -> RouteCapabilities {
        let image_input = match model {
            "qwen3.8-max" | "qwen3.8-max-preview" | "qwen3.7-plus" | "qwen3.6-flash" => {
                CapabilityState::Supported
            }
            _ => CapabilityState::Unsupported,
        };
        RouteCapabilities {
            reasoning: CapabilityState::Supported,
            native_tool_calls: CapabilityState::Supported,
            structured_output: CapabilityState::Supported,
            streaming: CapabilityState::Supported,
            image_input,
            server_side_web_search: super::documented_server_side_web_search(
                "modelstudio-token-plan",
                model,
            ),
            ..RouteCapabilities::default()
        }
    }
    fn ms_limits(model: &str) -> RouteLimits {
        // Context/output from models_dev.bundled.json Model Studio rows and
        // the owner console (verified 2026-08-03). Keep output separate from
        // context so a 128K generation ceiling is never mistaken for the
        // window.
        let (context_tokens, output_tokens) = match model {
            "qwen3.8-max" | "qwen3.8-max-preview" => (1_000_000, 131_072),
            "qwen3.7-plus" | "qwen3.7-max" => (1_000_000, 65_536),
            "qwen3.6-flash" => (1_000_000, 65_536),
            "deepseek-v4-pro" | "deepseek-v4-flash-0731" => (1_000_000, 384_000),
            "glm-5.2" => (1_000_000, 131_072),
            _ => (1_000_000, 131_072),
        };
        RouteLimits {
            context_tokens: Some(context_tokens),
            input_tokens: None,
            output_tokens: Some(output_tokens),
        }
    }
    // Primary vendor id only in the hand seam. Coding-plan / anthropic
    // dialect endpoint selection is owned by config resolution (mode/wire),
    // which rewrites base_url + request dialect without inventing kinds.
    let plan = ProviderId::from("modelstudio-token-plan");
    offerings.extend(
        MODELSTUDIO_TEXT_MODELS
            .iter()
            .enumerate()
            .map(|(i, model)| ProviderModelOffering {
                provider: plan.clone(),
                canonical_model: None,
                wire_model_id: WireModelId::from(*model),
                endpoint_key: "chat".to_string(),
                default_for_provider: i == 0,
                limits: ms_limits(model),
                capabilities: ms_capabilities(model),
                pricing: PricingSku::UnknownOrStale,
            }),
    );

    offerings
}
