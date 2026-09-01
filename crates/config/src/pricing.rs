//! Provider/offering-scoped pricing projection with provenance (#3085).
//!
//! Network-free. Maps Models.dev offering `cost` (and live / user-override
//! rows) into pricing rows that carry explicit **provenance**, **currency**, and
//! **effective-at** metadata, plus a pure cost estimator over normalized token
//! usage. UI display (`CostDisplay`) and provider usage-payload parsing live
//! above this layer and are out of scope here.
//!
//! Boundary with the route layer: this models *pricing* — offering-owned,
//! per-token unit prices. The coarse route-facing meter shape already exists as
//! [`crate::route::PricingSku`]
//! (`Token` / `SubscriptionQuota` / `AccountCredits` / `LocalOrNotApplicable` /
//! `UnknownOrStale`); [`OfferingPricing::to_route_sku`] and
//! [`route_pricing_sku`] bridge to it.
//!
//! Honesty rule (#2608 / #3085): pricing is never assumed. A route with no
//! sourced price yields `None` here and `UnknownOrStale` at the route layer —
//! never a fabricated token price, and never an implicit "free" for
//! local/custom/subscription routes.

use serde::{Deserialize, Serialize};

use crate::catalog::{CatalogOffering, CatalogSource};
use crate::models_dev::ModelsDevCost;
use crate::route::PricingSku;

/// Billing currency for a pricing row. Models.dev publishes USD per-million
/// costs; other currencies arrive via provider docs or user overrides.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    #[default]
    Usd,
    Cny,
    /// An ISO-4217-style code CodeWhale does not special-case.
    Other(String),
}

/// Where a pricing row came from. Retained so the UI can show provenance and so
/// stale/unknown prices are never silently treated as authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PricingProvenance {
    /// Seeded from a bundled Models.dev catalog snapshot.
    ModelsDevBundled,
    /// From a provider live `/models` (or pricing) refresh.
    ProviderLive,
    /// From provider documentation / a hand-sourced seed. Set only by callers
    /// constructing rows directly; `from_catalog_offering` never produces this
    /// (Models.dev-sourced rows map to `ModelsDevBundled` / `ProviderLive`).
    ProviderDocs,
    /// User-supplied override (custom endpoint, enterprise terms, local route).
    UserOverride,
    /// No sourced price.
    Unknown,
    /// From the Codewhale-owned signed/bundled catalog.
    CodewhaleCatalog,
}

impl PricingProvenance {
    /// Stable, non-localized identifier for logs, JSON, and scorecards.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::ModelsDevBundled => "models_dev_bundled",
            Self::ProviderLive => "provider_live",
            Self::ProviderDocs => "provider_docs",
            Self::UserOverride => "user_override",
            Self::Unknown => "unknown",
            Self::CodewhaleCatalog => "codewhale_catalog",
        }
    }

    /// Whether this provenance may be presented as an authoritative published
    /// price without further freshness checks.
    ///
    /// [`Self::ProviderLive`] is deliberately excluded: a live row is only
    /// authoritative while it is fresh *and* was fetched from the endpoint the
    /// turn was actually served on. Callers must clear it through
    /// [`OfferingPricing::live_pricing_defect`] first.
    #[must_use]
    pub fn is_authoritative_without_freshness_check(&self) -> bool {
        matches!(
            self,
            Self::ModelsDevBundled
                | Self::ProviderDocs
                | Self::UserOverride
                | Self::CodewhaleCatalog
        )
    }
}

/// Default freshness window for a `ProviderLive` pricing row, in seconds.
///
/// A provider `/models` refresh is a snapshot of a mutable price list. Past
/// this age CodeWhale stops calling the row authoritative rather than billing
/// against a rate the provider may have already changed.
pub const LIVE_PRICING_MAX_AGE_SECS: u64 = 24 * 60 * 60;

/// Why a `ProviderLive` pricing row cannot be treated as authoritative.
///
/// Each variant is a non-secret receipt: fingerprints are FNV digests of a
/// normalized base URL (see [`crate::catalog::base_url_fingerprint`]), never the
/// URL itself, so these can be logged and serialized freely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "defect", rename_all = "snake_case")]
pub enum LivePricingDefect {
    /// The row is older than the caller's freshness window.
    Stale { age_secs: u64, max_age_secs: u64 },
    /// The row was fetched from a different endpoint than the turn was served
    /// on, so it prices a different billing surface.
    EndpointMismatch {
        row_fingerprint: String,
        route_fingerprint: String,
    },
    /// The row claims live provenance but carries no endpoint fingerprint, so
    /// it cannot be matched to the route that is being priced.
    MissingEndpointFingerprint,
    /// The row claims live provenance but carries no fetch timestamp, so its
    /// age cannot be established.
    MissingTimestamp,
    /// The caller could not establish which endpoint the turn was served on, so
    /// a live row cannot be confirmed to price that route.
    UnknownRouteEndpoint,
}

impl LivePricingDefect {
    /// Stable, non-localized identifier for logs, JSON, and scorecards.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Stale { .. } => "live_pricing_stale",
            Self::EndpointMismatch { .. } => "live_pricing_endpoint_mismatch",
            Self::MissingEndpointFingerprint => "live_pricing_missing_endpoint_fingerprint",
            Self::MissingTimestamp => "live_pricing_missing_timestamp",
            Self::UnknownRouteEndpoint => "live_pricing_unknown_route_endpoint",
        }
    }
}

/// Normalized token usage for a single turn, in canonical billable classes.
///
/// Producing this from provider-specific usage payloads (Chat Completions,
/// Responses, Anthropic) is a separate concern; this layer only consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Non-cached input (prompt) tokens.
    pub input: u64,
    /// Total billable output (completion) tokens.
    ///
    /// Providers report reasoning tokens as a *subset* of the completion token
    /// count (OpenAI `output_tokens_details.reasoning_tokens` ⊆ `output_tokens`,
    /// Chat Completions `completion_tokens_details.reasoning_tokens` ⊆
    /// `completion_tokens`), so a normalizer must never add reasoning tokens on
    /// top of this field — that double-bills every reasoning turn.
    pub output: u64,
    /// Cache-read (cache-hit) input tokens, billed at the cache-read rate.
    pub cache_read: u64,
    /// Cache-write (cache-creation) tokens, billed at the cache-write rate.
    pub cache_write: u64,
}

/// A canonical billable token class.
///
/// Used to report *which* class of a turn's usage lacked a published price, so
/// an unpriced turn can be audited instead of silently dropping out of a total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenClass {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl TokenClass {
    /// Every class, in reporting order.
    pub const ALL: [Self; 4] = [Self::Input, Self::Output, Self::CacheRead, Self::CacheWrite];

    /// Stable, non-localized identifier for logs, JSON, and scorecards.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache_read",
            Self::CacheWrite => "cache_write",
        }
    }

    /// This class's token count within `usage`.
    #[must_use]
    pub fn tokens(self, usage: &TokenUsage) -> u64 {
        match self {
            Self::Input => usage.input,
            Self::Output => usage.output,
            Self::CacheRead => usage.cache_read,
            Self::CacheWrite => usage.cache_write,
        }
    }
}

/// A provider/offering-scoped pricing row.
///
/// Prices are per million tokens in [`Currency`]. Any field may be unknown
/// (`None`); [`OfferingPricing::estimate_cost`] refuses to invent a number for a
/// used class whose price is unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OfferingPricing {
    /// Provider id serving the offering.
    pub provider: String,
    /// Provider-owned wire id the price applies to.
    pub wire_model_id: String,
    /// Canonical model identity, when the offering carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model: Option<String>,
    /// Billing currency.
    pub currency: Currency,
    /// Input price per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_million: Option<f64>,
    /// Output price per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_million: Option<f64>,
    /// Cache-read price per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_per_million: Option<f64>,
    /// Cache-write price per million tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_million: Option<f64>,
    /// Where the price came from.
    pub provenance: PricingProvenance,
    /// Unix seconds the price was fetched / became effective, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_at: Option<u64>,
    /// Fingerprint of the base URL this price was fetched from, for
    /// [`PricingProvenance::ProviderLive`] rows.
    ///
    /// This is the same non-secret SHA-256 digest the catalog cache scopes on
    /// (see [`crate::catalog::base_url_fingerprint`]) — never the URL itself.
    /// It exists so a live row can be proven to price the endpoint a turn was
    /// actually served on; a row whose fingerprint does not match the route
    /// is a different billing surface, not a fresher price for this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_fingerprint: Option<String>,
}

impl OfferingPricing {
    /// Derive a pricing row from a catalog offering's `cost`, when priced.
    ///
    /// Returns `None` when the offering carries no cost, or a cost object with
    /// no concrete price field — those routes are *unknown*, not free, and the
    /// caller should render them as such (see [`route_pricing_sku`]).
    ///
    /// Models.dev `cost` values are USD per million tokens, so the currency is
    /// [`Currency::Usd`]; provenance and `effective_at` follow the offering's
    /// [`CatalogSource`].
    #[must_use]
    pub fn from_catalog_offering(offering: &CatalogOffering) -> Option<Self> {
        let cost = offering.cost.as_ref()?;
        // A provider/catalog price is untrusted numeric input.  Reject the
        // entire row when any published class is NaN, infinite, or negative;
        // accepting only the apparently valid fields would turn a malformed
        // row into a silently incomplete (or negative) bill.
        if !catalog_cost_is_valid(cost) {
            return None;
        }
        if cost.input.is_none()
            && cost.output.is_none()
            && cost.cache_read.is_none()
            && cost.cache_write.is_none()
        {
            return None;
        }
        Some(Self {
            provider: offering.provider.clone(),
            wire_model_id: offering.wire_model_id.clone(),
            canonical_model: offering.canonical_model.clone(),
            currency: Currency::Usd,
            input_per_million: cost.input,
            output_per_million: cost.output,
            cache_read_per_million: cost.cache_read,
            cache_write_per_million: cost.cache_write,
            provenance: provenance_from_source(&offering.source),
            effective_at: effective_at_from_source(&offering.source),
            endpoint_fingerprint: endpoint_fingerprint_from_source(&offering.source),
        })
    }

    /// Whether any per-token price is known.
    #[must_use]
    pub fn has_any_price(&self) -> bool {
        self.input_per_million.is_some()
            || self.output_per_million.is_some()
            || self.cache_read_per_million.is_some()
            || self.cache_write_per_million.is_some()
    }

    /// Whether this price is older than `max_age_secs` at `now_unix`.
    ///
    /// Rows without an `effective_at` (bundled snapshot / user override) carry
    /// no fetch clock and are not considered age-stale here; live rows are.
    #[must_use]
    pub fn is_stale(&self, now_unix: u64, max_age_secs: u64) -> bool {
        match self.effective_at {
            Some(t) => now_unix.saturating_sub(t) >= max_age_secs,
            None => false,
        }
    }

    /// Why this row cannot be trusted as an authoritative live price, if so.
    ///
    /// Returns `None` for rows that are not [`PricingProvenance::ProviderLive`]
    /// (a bundled snapshot, a documented hand price, or a user override carries
    /// no fetch clock to go stale against) and for live rows that are both
    /// fresh and fingerprint-matched to `route_endpoint_fingerprint`.
    ///
    /// A live row with any defect must not be labelled `provider_live` nor used
    /// as complete pricing: it is either older than `max_age_secs` or priced for
    /// a different endpoint. Callers fail closed and receipt the returned
    /// defect. `route_endpoint_fingerprint` of `None` means the caller could not
    /// determine the endpoint at all, which is itself a defect — a live row can
    /// never be *confirmed* to price an unknown route.
    #[must_use]
    pub fn live_pricing_defect(
        &self,
        route_endpoint_fingerprint: Option<&str>,
        now_unix: Option<u64>,
        max_age_secs: u64,
    ) -> Option<LivePricingDefect> {
        if self.provenance != PricingProvenance::ProviderLive {
            return None;
        }
        let Some(row_fingerprint) = self.endpoint_fingerprint.as_deref() else {
            return Some(LivePricingDefect::MissingEndpointFingerprint);
        };
        let Some(route_fingerprint) = route_endpoint_fingerprint else {
            return Some(LivePricingDefect::UnknownRouteEndpoint);
        };
        if row_fingerprint != route_fingerprint {
            return Some(LivePricingDefect::EndpointMismatch {
                row_fingerprint: row_fingerprint.to_string(),
                route_fingerprint: route_fingerprint.to_string(),
            });
        }
        let Some(effective_at) = self.effective_at else {
            return Some(LivePricingDefect::MissingTimestamp);
        };
        // Without a clock the age is unknowable, so the row stays unproven
        // rather than being assumed fresh.
        let Some(now_unix) = now_unix else {
            return Some(LivePricingDefect::MissingTimestamp);
        };
        let age_secs = now_unix.saturating_sub(effective_at);
        if age_secs >= max_age_secs {
            return Some(LivePricingDefect::Stale {
                age_secs,
                max_age_secs,
            });
        }
        None
    }

    /// Per-million price for one canonical class, when published.
    #[must_use]
    pub fn price_per_million(&self, class: TokenClass) -> Option<f64> {
        match class {
            TokenClass::Input => self.input_per_million,
            TokenClass::Output => self.output_per_million,
            TokenClass::CacheRead => self.cache_read_per_million,
            TokenClass::CacheWrite => self.cache_write_per_million,
        }
    }

    /// Classes this turn actually used that carry no published price.
    ///
    /// Non-empty means [`Self::estimate_cost`] fails closed for this usage; the
    /// returned classes are exactly the reason why, so callers can report the
    /// gap instead of presenting a silently under-counted total.
    #[must_use]
    pub fn unpriced_used_classes(&self, usage: &TokenUsage) -> Vec<TokenClass> {
        TokenClass::ALL
            .into_iter()
            .filter(|class| class.tokens(usage) > 0 && self.price_per_million(*class).is_none())
            .collect()
    }

    /// Estimate the cost of `usage` in this row's [`Currency`].
    ///
    /// Returns `None` if any usage class with a non-zero token count has an
    /// unknown price — the estimate would otherwise silently under-report. With
    /// all-zero usage the cost is `Some(0.0)`.
    #[must_use]
    pub fn estimate_cost(&self, usage: &TokenUsage) -> Option<f64> {
        let mut total = 0.0_f64;
        for class in TokenClass::ALL {
            let tokens = class.tokens(usage);
            if tokens > 0 {
                let price = self.price_per_million(class)?;
                // Per-turn token counts are far below 2^53, so this cast is
                // exact; revisit if TokenUsage ever aggregates across sessions.
                let component = (tokens as f64 / 1_000_000.0) * price;
                if !component.is_finite() || component < 0.0 {
                    return None;
                }
                total += component;
                if !total.is_finite() || total < 0.0 {
                    return None;
                }
            }
        }
        Some(total)
    }

    /// Project to the coarse route-facing meter shape.
    ///
    /// Returns [`PricingSku::Token`] only when an input or output rate is known.
    /// The route-layer `Token` shape carries only input/output rates, so a row
    /// priced *only* on cache classes would become a `Token` with no visible
    /// rates — misleading at the route layer. Such rows degrade to
    /// [`PricingSku::UnknownOrStale`] here while their cache rates remain usable
    /// through [`OfferingPricing::estimate_cost`].
    #[must_use]
    pub fn to_route_sku(&self) -> PricingSku {
        if self.input_per_million.is_none() && self.output_per_million.is_none() {
            return PricingSku::UnknownOrStale;
        }
        PricingSku::Token {
            input_per_mtok: self.input_per_million,
            output_per_mtok: self.output_per_million,
        }
    }
}

/// The honest route-facing pricing meter for a catalog offering.
///
/// An offering with a usable input/output rate becomes [`PricingSku::Token`];
/// everything else — no cost, a cost object with no concrete price, or a
/// cache-only price — becomes [`PricingSku::UnknownOrStale`] rather than a
/// fabricated zero price. (`from_catalog_offering` collapses the unpriced case
/// to `None`; `to_route_sku` collapses the cache-only case.)
#[must_use]
pub fn route_pricing_sku(offering: &CatalogOffering) -> PricingSku {
    OfferingPricing::from_catalog_offering(offering)
        .map_or(PricingSku::UnknownOrStale, |pricing| pricing.to_route_sku())
}

/// The honest route-facing pricing meter for a raw Models.dev `cost` block.
///
/// Same honesty rule as [`route_pricing_sku`], but for callers that hold a
/// [`ModelsDevCost`] directly (the route-offering builders in
/// [`crate::models_dev`]) rather than a full [`CatalogOffering`]. An absent or
/// concretely-empty cost, or a cache-only cost, yields
/// [`PricingSku::UnknownOrStale`]; only a usable input/output rate yields
/// [`PricingSku::Token`].
#[must_use]
pub(crate) fn route_pricing_sku_from_cost(cost: Option<&ModelsDevCost>) -> PricingSku {
    let Some(cost) = cost else {
        return PricingSku::UnknownOrStale;
    };
    if !catalog_cost_is_valid(cost) {
        return PricingSku::UnknownOrStale;
    }
    if cost.input.is_none() && cost.output.is_none() {
        // No input/output rate: a cache-only or empty cost would render as a
        // rate-less `Token` at the route layer, so it stays honestly unknown.
        return PricingSku::UnknownOrStale;
    }
    PricingSku::Token {
        input_per_mtok: cost.input,
        output_per_mtok: cost.output,
    }
}

/// Upper bound, per million tokens, on a price CodeWhale will treat as real.
///
/// Published frontier rates are in the single-to-triple digits per million.
/// A value four orders of magnitude above that is not an expensive model, it is
/// a unit error — a per-token price parsed as per-million, or a minor-unit
/// integer (cents, fen) read as a major unit. Both mistakes bill the user
/// 10^6 or 10^2 times over, so the row is rejected rather than believed.
///
/// The bound is deliberately generous: it exists to catch impossible
/// magnitudes, not to second-guess a provider's pricing.
pub const MAX_PLAUSIBLE_PRICE_PER_MILLION: f64 = 100_000.0;

/// Whether every numeric field in a catalog price is finite, non-negative, and
/// of a plausible magnitude.
///
/// Kept at the catalog boundary so every projection (routing SKU and runtime
/// cost audit) applies the same validation rule. Catalog prices are untrusted
/// numeric input: they arrive from a bundled snapshot, a live provider
/// `/models` response, or a user override file, and any of the three can carry
/// a malformed value. The whole row is rejected on a single bad field —
/// accepting the fields that happen to parse would turn a malformed row into a
/// silently under-counted bill, which is worse than no price at all.
#[must_use]
pub fn catalog_cost_is_valid(cost: &ModelsDevCost) -> bool {
    [cost.input, cost.output, cost.cache_read, cost.cache_write]
        .into_iter()
        .flatten()
        .all(|price| price.is_finite() && (0.0..=MAX_PLAUSIBLE_PRICE_PER_MILLION).contains(&price))
}

fn provenance_from_source(source: &CatalogSource) -> PricingProvenance {
    match source {
        CatalogSource::Bundled | CatalogSource::ModelsDevLive { .. } => {
            PricingProvenance::ModelsDevBundled
        }
        CatalogSource::Live { .. } => PricingProvenance::ProviderLive,
        CatalogSource::ConfigOverride | CatalogSource::UserOverride => {
            PricingProvenance::UserOverride
        }
        CatalogSource::CodewhaleBundled { .. } | CatalogSource::CodewhaleLive { .. } => {
            PricingProvenance::CodewhaleCatalog
        }
    }
}

fn effective_at_from_source(source: &CatalogSource) -> Option<u64> {
    match source {
        CatalogSource::Live { fetched_at, .. }
        | CatalogSource::ModelsDevLive { fetched_at }
        | CatalogSource::CodewhaleLive { fetched_at, .. } => Some(*fetched_at),
        CatalogSource::Bundled
        | CatalogSource::ConfigOverride
        | CatalogSource::UserOverride
        | CatalogSource::CodewhaleBundled { .. } => None,
    }
}

fn endpoint_fingerprint_from_source(source: &CatalogSource) -> Option<String> {
    match source {
        CatalogSource::Live {
            base_url_fingerprint,
            ..
        } => Some(base_url_fingerprint.clone()),
        CatalogSource::Bundled
        | CatalogSource::ModelsDevLive { .. }
        | CatalogSource::ConfigOverride
        | CatalogSource::UserOverride
        | CatalogSource::CodewhaleBundled { .. }
        | CatalogSource::CodewhaleLive { .. } => None,
    }
}

#[cfg(test)]
mod tests;
