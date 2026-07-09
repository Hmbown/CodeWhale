//! Provider model offerings (#3084).
//!
//! A [`ProviderModelOffering`] binds a provider to a canonical model, the
//! provider-owned wire id that serves it, and the endpoint key. This is the
//! seam that proves the #2608 invariant: the SAME canonical model can be served
//! by multiple providers under DIFFERENT wire ids (some aggregator-prefixed),
//! and a prefix never implies provider ownership.
//!
//! The hand-curated seed table is gone (#4139 / #3830 P1): catalog-derived
//! offerings from [`crate::catalog::bundled_catalog_offerings`] are the single
//! bundled source of truth. [`bundled_offerings`] remains as an empty seam so
//! the resolver can still prepend curated overrides later without reintroducing
//! a parallel seed list.

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

/// Return the bundled offering seam as owned [`ProviderModelOffering`] rows.
///
/// Empty by design: every former hand-seed row is covered by the bundled
/// Models.dev catalog ([`crate::catalog::bundled_catalog_offerings`]), which
/// carries the same canonical-model joins via `base_model` plus honest limits
/// and pricing the old seeds lacked (#4139 / #3830 P1 OFFERING_SEEDS dedupe).
#[must_use]
pub fn bundled_offerings() -> Vec<ProviderModelOffering> {
    Vec::new()
}
