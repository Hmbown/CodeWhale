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
}

/// Normalized token usage for a single turn, in canonical billable classes.
///
/// Producing this from provider-specific usage payloads (Chat Completions,
/// Responses, Anthropic) is a separate concern; this layer only consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Non-cached input (prompt) tokens.
    pub input: u64,
    /// Output (completion) tokens, including reasoning output.
    pub output: u64,
    /// Cache-read (cache-hit) input tokens, billed at the cache-read rate.
    pub cache_read: u64,
    /// Cache-write (cache-creation) tokens, billed at the cache-write rate.
    pub cache_write: u64,
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

    /// Estimate the cost of `usage` in this row's [`Currency`].
    ///
    /// Returns `None` if any usage class with a non-zero token count has an
    /// unknown price — the estimate would otherwise silently under-report. With
    /// all-zero usage the cost is `Some(0.0)`.
    #[must_use]
    pub fn estimate_cost(&self, usage: &TokenUsage) -> Option<f64> {
        let mut total = 0.0_f64;
        for (tokens, price) in [
            (usage.input, self.input_per_million),
            (usage.output, self.output_per_million),
            (usage.cache_read, self.cache_read_per_million),
            (usage.cache_write, self.cache_write_per_million),
        ] {
            if tokens > 0 {
                let price = price?;
                // Per-turn token counts are far below 2^53, so this cast is
                // exact; revisit if TokenUsage ever aggregates across sessions.
                total += (tokens as f64 / 1_000_000.0) * price;
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

fn provenance_from_source(source: &CatalogSource) -> PricingProvenance {
    match source {
        CatalogSource::Bundled => PricingProvenance::ModelsDevBundled,
        CatalogSource::Live { .. } => PricingProvenance::ProviderLive,
        CatalogSource::UserOverride => PricingProvenance::UserOverride,
    }
}

fn effective_at_from_source(source: &CatalogSource) -> Option<u64> {
    match source {
        CatalogSource::Live { fetched_at, .. } => Some(*fetched_at),
        CatalogSource::Bundled | CatalogSource::UserOverride => None,
    }
}

#[cfg(test)]
mod tests;
