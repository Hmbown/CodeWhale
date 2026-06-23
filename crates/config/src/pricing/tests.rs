//! Behavior tests for the offering pricing projection (#3085).

use super::*;
use crate::catalog::{CatalogOffering, CatalogSource, bundled_offerings_from_models_dev};
use crate::models_dev::{ModelsDevCatalog, ModelsDevCost};
use crate::route::PricingSku;

/// A DeepSeek-shaped priced offering (input/output/cache-read known,
/// cache-write deliberately unknown) tagged with the given provenance source.
fn priced(source: CatalogSource) -> CatalogOffering {
    CatalogOffering {
        provider: "deepseek".into(),
        wire_model_id: "deepseek-v4-pro".into(),
        canonical_model: Some("deepseek-v4-pro".into()),
        endpoint_key: "chat".into(),
        cost: Some(ModelsDevCost {
            input: Some(0.28),
            output: Some(0.42),
            cache_read: Some(0.028),
            cache_write: None,
        }),
        source,
        ..Default::default()
    }
}

#[test]
fn maps_models_dev_cost_with_bundled_provenance_in_usd() {
    let p =
        OfferingPricing::from_catalog_offering(&priced(CatalogSource::Bundled)).expect("priced");
    assert_eq!(p.currency, Currency::Usd);
    assert_eq!(p.input_per_million, Some(0.28));
    assert_eq!(p.output_per_million, Some(0.42));
    assert_eq!(p.cache_read_per_million, Some(0.028));
    assert_eq!(p.cache_write_per_million, None);
    assert_eq!(p.provenance, PricingProvenance::ModelsDevBundled);
    assert_eq!(p.effective_at, None);
    assert!(p.has_any_price());
}

#[test]
fn live_source_carries_provider_live_provenance_and_effective_at() {
    let src = CatalogSource::Live {
        base_url_fingerprint: "fp".into(),
        fetched_at: 1_700,
    };
    let p = OfferingPricing::from_catalog_offering(&priced(src)).expect("priced");
    assert_eq!(p.provenance, PricingProvenance::ProviderLive);
    assert_eq!(p.effective_at, Some(1_700));
}

#[test]
fn no_cost_or_empty_cost_object_is_unknown() {
    let mut offering = priced(CatalogSource::Bundled);
    offering.cost = None;
    assert!(
        OfferingPricing::from_catalog_offering(&offering).is_none(),
        "absent cost is unknown, not free"
    );

    // A cost object present but with no concrete price is still unknown.
    offering.cost = Some(ModelsDevCost::default());
    assert!(OfferingPricing::from_catalog_offering(&offering).is_none());
}

#[test]
fn estimate_cost_sums_priced_classes() {
    let p = OfferingPricing::from_catalog_offering(&priced(CatalogSource::Bundled)).unwrap();
    // 1M input @0.28 + 0.5M output @0.42 + 2M cache_read @0.028 = 0.546
    let usage = TokenUsage {
        input: 1_000_000,
        output: 500_000,
        cache_read: 2_000_000,
        cache_write: 0,
    };
    let cost = p.estimate_cost(&usage).expect("priced classes estimate");
    assert!((cost - 0.546).abs() < 1e-9, "got {cost}");
}

#[test]
fn estimate_cost_is_none_when_a_used_class_is_unpriced() {
    // cache_write price is unknown; charging cache-write tokens cannot be
    // estimated honestly, so the whole estimate is None rather than under-reported.
    let p = OfferingPricing::from_catalog_offering(&priced(CatalogSource::Bundled)).unwrap();
    let usage = TokenUsage {
        input: 100,
        output: 0,
        cache_read: 0,
        cache_write: 10,
    };
    assert!(p.estimate_cost(&usage).is_none());
}

#[test]
fn estimate_cost_with_zero_usage_is_zero() {
    let p = OfferingPricing::from_catalog_offering(&priced(CatalogSource::Bundled)).unwrap();
    assert_eq!(p.estimate_cost(&TokenUsage::default()), Some(0.0));
}

#[test]
fn route_pricing_sku_is_token_when_priced_and_unknown_otherwise() {
    match route_pricing_sku(&priced(CatalogSource::Bundled)) {
        PricingSku::Token {
            input_per_mtok,
            output_per_mtok,
        } => {
            assert_eq!(input_per_mtok, Some(0.28));
            assert_eq!(output_per_mtok, Some(0.42));
        }
        other => panic!("expected Token, got {other:?}"),
    }

    // No cost → honest UnknownOrStale, never a fabricated zero price.
    let mut unpriced = priced(CatalogSource::Bundled);
    unpriced.cost = None;
    assert!(matches!(
        route_pricing_sku(&unpriced),
        PricingSku::UnknownOrStale
    ));
}

#[test]
fn currency_round_trips_including_other() {
    for currency in [Currency::Usd, Currency::Cny, Currency::Other("eur".into())] {
        let json = serde_json::to_string(&currency).expect("serialize");
        let back: Currency = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(currency, back);
    }
}

#[test]
fn user_override_pricing_round_trips_and_carries_no_secrets() {
    let pricing = OfferingPricing {
        provider: "custom".into(),
        wire_model_id: "house-model".into(),
        canonical_model: None,
        currency: Currency::Cny,
        input_per_million: Some(8.0),
        output_per_million: Some(16.0),
        cache_read_per_million: None,
        cache_write_per_million: None,
        provenance: PricingProvenance::UserOverride,
        effective_at: None,
    };
    let json = serde_json::to_string_pretty(&pricing).expect("serialize");
    let back: OfferingPricing = serde_json::from_str(&json).expect("round-trip");
    assert_eq!(pricing, back);

    let lower = json.to_lowercase();
    for needle in [
        "api_key",
        "apikey",
        "authorization",
        "secret",
        "password",
        "bearer",
        "access_token",
    ] {
        assert!(!lower.contains(needle), "pricing JSON contains `{needle}`");
    }
}

#[test]
fn staleness_applies_to_live_rows_only() {
    let live = CatalogSource::Live {
        base_url_fingerprint: "fp".into(),
        fetched_at: 1_000,
    };
    let live_price = OfferingPricing::from_catalog_offering(&priced(live)).unwrap();
    assert!(!live_price.is_stale(1_500, 3_600), "within TTL");
    assert!(live_price.is_stale(5_000, 3_600), "past TTL");

    // A bundled price has no fetch clock and is not age-stale.
    let bundled = OfferingPricing::from_catalog_offering(&priced(CatalogSource::Bundled)).unwrap();
    assert!(!bundled.is_stale(u64::MAX, 1));
}

#[test]
fn pricing_flows_from_the_models_dev_parser() {
    let raw = r#"{
      "providers": {
        "zai": {
          "models": {
            "glm-5.2": {
              "id": "glm-5.2",
              "modalities": { "input": ["text"], "output": ["text"] },
              "cost": { "input": 1.4, "output": 4.4, "cache_read": 0.26 }
            }
          }
        }
      }
    }"#;
    let catalog = ModelsDevCatalog::parse_json(raw).expect("fixture parses");
    let rows = bundled_offerings_from_models_dev(&catalog);
    let pricing = OfferingPricing::from_catalog_offering(&rows[0]).expect("zai glm-5.2 is priced");

    assert_eq!(pricing.provider, "zai");
    assert_eq!(pricing.wire_model_id, "glm-5.2");
    assert_eq!(pricing.input_per_million, Some(1.4));
    assert_eq!(pricing.output_per_million, Some(4.4));
    assert_eq!(pricing.cache_read_per_million, Some(0.26));
    assert_eq!(pricing.provenance, PricingProvenance::ModelsDevBundled);
}

#[test]
fn cache_only_offering_is_unknown_at_the_route_layer() {
    // Priced only on cache classes (no input/output): the route Token badge
    // would have no visible rates, so route_pricing_sku degrades to
    // UnknownOrStale — yet the cache rate is still usable for estimate_cost.
    let mut offering = priced(CatalogSource::Bundled);
    offering.cost = Some(ModelsDevCost {
        input: None,
        output: None,
        cache_read: Some(0.028),
        cache_write: None,
    });

    assert!(matches!(
        route_pricing_sku(&offering),
        PricingSku::UnknownOrStale
    ));

    let pricing =
        OfferingPricing::from_catalog_offering(&offering).expect("cache-only row is still priced");
    assert!(pricing.has_any_price());
    let usage = TokenUsage {
        cache_read: 1_000_000,
        ..Default::default()
    };
    assert_eq!(pricing.estimate_cost(&usage), Some(0.028));
}

#[test]
fn user_override_source_maps_through_from_catalog_offering() {
    // Exercises provenance_from_source / effective_at_from_source for the
    // override arm via the hydration path (not direct construction).
    let pricing = OfferingPricing::from_catalog_offering(&priced(CatalogSource::UserOverride))
        .expect("priced");
    assert_eq!(pricing.provenance, PricingProvenance::UserOverride);
    assert_eq!(pricing.effective_at, None);
}

#[test]
fn staleness_is_inclusive_at_the_ttl_boundary() {
    let live = CatalogSource::Live {
        base_url_fingerprint: "fp".into(),
        fetched_at: 1_000,
    };
    let p = OfferingPricing::from_catalog_offering(&priced(live)).unwrap();
    // age == max_age_secs counts as stale (`>=` semantics)...
    assert!(p.is_stale(1_100, 100));
    // ...one second younger is still fresh.
    assert!(!p.is_stale(1_099, 100));
}
