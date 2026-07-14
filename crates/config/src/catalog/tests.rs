//! Behavior tests for the Models.dev-backed catalog cache (#3385).
//!
//! Fixtures use synthetic ids for anti-hardcoding guards, plus the GLM-5.2 and
//! hosted-DeepSeek rows the issue explicitly asks to exercise. No full hosted
//! provider model list is copied here.

use super::*;

/// Zhipu canonical + Zhipu/Z.AI provider offerings, and a hosted DeepSeek row
/// served by an aggregator under a prefixed wire id with an explicit canonical
/// `base_model` join.
const FIXTURE: &str = r#"{
  "models": {
    "zhipuai/glm-5.2": {
      "id": "zhipuai/glm-5.2",
      "family": "glm",
      "reasoning": true,
      "modalities": { "input": ["text"], "output": ["text"] },
      "limit": { "context": 1000000, "output": 131072 }
    }
  },
  "providers": {
    "zhipuai": {
      "id": "zhipuai",
      "models": {
        "glm-5.2": {
          "id": "glm-5.2",
          "family": "glm",
          "default": true,
          "reasoning": true,
          "reasoning_options": [{ "type": "effort", "values": ["high", "max"] }],
          "modalities": { "input": ["text"], "output": ["text"] },
          "limit": { "context": 1000000, "output": 131072 },
          "cost": { "input": 1.4, "output": 4.4, "cache_read": 0.26 }
        },
        "glm-voice": {
          "id": "glm-voice",
          "modalities": { "input": ["text"], "output": ["audio"] }
        }
      }
    },
    "together": {
      "id": "together",
      "models": {
        "deepseek-ai/DeepSeek-V4-Pro": {
          "id": "deepseek-ai/DeepSeek-V4-Pro",
          "base_model": "deepseek-v4-pro",
          "family": "deepseek",
          "reasoning": false,
          "modalities": { "input": ["text"], "output": ["text"] },
          "cost": { "input": 0.9, "output": 0.9 }
        }
      }
    }
  }
}"#;

fn fixture() -> ModelsDevCatalog {
    ModelsDevCatalog::parse_json(FIXTURE).expect("fixture parses")
}

fn find<'a>(rows: &'a [CatalogOffering], provider: &str, wire: &str) -> &'a CatalogOffering {
    rows.iter()
        .find(|r| r.provider == provider && r.wire_model_id == wire)
        .unwrap_or_else(|| panic!("offering {provider}/{wire} not found"))
}

#[test]
fn hydrates_models_dev_offerings_preserving_offering_facts() {
    let rows = bundled_offerings_from_models_dev(&fixture());

    // glm-voice (audio output) is excluded; two chat offerings remain.
    assert_eq!(rows.len(), 2, "audio-only rows are not chat offerings");

    let glm = find(&rows, "zhipuai", "glm-5.2");
    assert!(glm.default_for_provider);
    assert_eq!(glm.family.as_deref(), Some("glm"));
    assert_eq!(glm.reasoning, Some(true));
    // Provider-scoped reasoning options are preserved, not collapsed.
    assert_eq!(glm.reasoning_options.len(), 1);
    assert_eq!(glm.limit.as_ref().and_then(|l| l.context), Some(1_000_000));
    assert_eq!(glm.cost.as_ref().and_then(|c| c.cache_read), Some(0.26));
    // Provider row carried no base_model link → no inferred canonical model.
    assert_eq!(glm.canonical_model, None);
    assert_eq!(glm.source, CatalogSource::Bundled);
}

#[test]
fn hosted_offering_keeps_prefixed_wire_id_and_explicit_canonical_join() {
    let rows = bundled_offerings_from_models_dev(&fixture());
    let hosted = find(&rows, "together", "deepseek-ai/DeepSeek-V4-Pro");

    // The prefixed wire id is preserved verbatim under the serving provider.
    assert_eq!(hosted.wire_model_id, "deepseek-ai/DeepSeek-V4-Pro");
    assert_eq!(hosted.provider, "together");
    // Canonical link comes only from the explicit base_model.
    assert_eq!(hosted.canonical_model.as_deref(), Some("deepseek-v4-pro"));
    assert_eq!(hosted.reasoning, Some(false));
}

#[test]
fn to_offering_projects_routing_identity_and_limits() {
    let rows = bundled_offerings_from_models_dev(&fixture());
    let glm = find(&rows, "zhipuai", "glm-5.2").to_offering();

    assert_eq!(glm.provider.as_str(), "zhipuai");
    assert_eq!(glm.wire_model_id.as_str(), "glm-5.2");
    assert_eq!(glm.canonical_model, None);
    assert_eq!(glm.endpoint_key, "chat");
    assert_eq!(glm.limits.context_tokens, Some(1_000_000));
    assert_eq!(glm.limits.output_tokens, Some(131_072));
}

#[test]
fn compiler_merges_layers_with_override_precedence() {
    // Bundled default for synthetic provider "acme".
    let bundled = vec![CatalogOffering {
        provider: "acme".into(),
        wire_model_id: "synth-chat-1".into(),
        endpoint_key: "chat".into(),
        default_for_provider: true,
        family: Some("synth".into()),
        source: CatalogSource::Bundled,
        ..Default::default()
    }];
    // Live refresh adds a new row AND restates the bundled one with a cost.
    let live = vec![
        CatalogOffering {
            provider: "acme".into(),
            wire_model_id: "synth-chat-1".into(),
            endpoint_key: "chat".into(),
            cost: Some(ModelsDevCost {
                input: Some(2.0),
                ..Default::default()
            }),
            source: CatalogSource::Live {
                base_url_fingerprint: "fp".into(),
                fetched_at: 100,
            },
            ..Default::default()
        },
        CatalogOffering {
            provider: "acme".into(),
            wire_model_id: "synth-chat-2".into(),
            endpoint_key: "chat".into(),
            source: CatalogSource::Live {
                base_url_fingerprint: "fp".into(),
                fetched_at: 100,
            },
            ..Default::default()
        },
    ];
    // User override pins a custom canonical model on synth-chat-1.
    let overrides = vec![CatalogOffering {
        provider: "acme".into(),
        wire_model_id: "synth-chat-1".into(),
        canonical_model: Some("acme-canonical".into()),
        endpoint_key: "chat".into(),
        source: CatalogSource::UserOverride,
        ..Default::default()
    }];

    let snapshot = CatalogCompiler::new()
        .with_bundled(bundled)
        .with_live(live)
        .with_overrides(overrides)
        .compile();

    // Two distinct (provider, wire) identities survive de-duplication.
    assert_eq!(snapshot.offerings.len(), 2);

    let one = find(&snapshot.offerings, "acme", "synth-chat-1");
    // Highest-precedence layer (override) wins the identity collision.
    assert_eq!(one.source, CatalogSource::UserOverride);
    assert_eq!(one.canonical_model.as_deref(), Some("acme-canonical"));

    let two = find(&snapshot.offerings, "acme", "synth-chat-2");
    assert!(matches!(two.source, CatalogSource::Live { .. }));
}

#[test]
fn cache_scopes_by_provider_and_base_url_fingerprint() {
    let fp_a = base_url_fingerprint("https://api.example.com/v1");
    let fp_b = base_url_fingerprint("https://other.example.com/v1");
    assert_ne!(fp_a, fp_b, "different hosts must not share a fingerprint");

    let mut cache = ProviderCatalogCache::new();
    let row = |id: &str| CatalogOffering {
        provider: "acme".into(),
        wire_model_id: id.into(),
        endpoint_key: "chat".into(),
        ..Default::default()
    };

    // Same provider, two different base URLs.
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp_a.clone(),
            fetched_at: 1_000,
            offerings: vec![row("from-a")],
        },
        3_600,
    );
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp_b.clone(),
            fetched_at: 1_000,
            offerings: vec![row("from-b")],
        },
        3_600,
    );
    // Different provider, SAME base URL as fp_a.
    cache.record_success(
        ProviderCatalogDelta {
            provider: "beta".into(),
            base_url_fingerprint: fp_a.clone(),
            fetched_at: 1_000,
            offerings: vec![row("from-beta")],
        },
        3_600,
    );

    let a = cache.fresh_offerings("acme", &fp_a, 1_100);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].wire_model_id, "from-a");
    // Same provider, different base URL must not leak rows across.
    let b = cache.fresh_offerings("acme", &fp_b, 1_100);
    assert_eq!(b[0].wire_model_id, "from-b");
    // Different provider on the same base URL must not share rows either.
    let beta = cache.fresh_offerings("beta", &fp_a, 1_100);
    assert_eq!(beta[0].wire_model_id, "from-beta");
    assert_eq!(cache.entries.len(), 3);
}

#[test]
fn fingerprint_folds_cosmetic_base_url_differences() {
    let canonical = base_url_fingerprint("https://API.Example.com/v1");
    assert_eq!(
        canonical,
        base_url_fingerprint("https://api.example.com/v1/"),
        "trailing slash + host case must not change the cache scope"
    );
    assert_eq!(
        canonical,
        base_url_fingerprint("  https://api.example.com:443/v1  "),
        "default https port + surrounding whitespace must fold away"
    );
    // Path case is significant (providers can be case-sensitive on the path).
    assert_ne!(
        canonical,
        base_url_fingerprint("https://api.example.com/V1")
    );

    // Port stripping is scheme-aware: :80 is http's default (folds away), but
    // :443 on http is a non-default port and must stay distinct from bare http.
    assert_eq!(
        base_url_fingerprint("http://h.example.com:80/v1"),
        base_url_fingerprint("http://h.example.com/v1"),
        "http default port :80 must fold away"
    );
    assert_ne!(
        base_url_fingerprint("http://h.example.com:443/v1"),
        base_url_fingerprint("http://h.example.com/v1"),
        ":443 is not http's default port and must not fold"
    );
}

#[test]
fn ttl_marks_entries_stale_and_excludes_them_from_fresh() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "synth-chat-1".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        100, // ttl
    );

    // Within TTL: fresh.
    assert_eq!(cache.status("acme", &fp, 1_050), CatalogStatus::Fresh);
    assert_eq!(cache.fresh_offerings("acme", &fp, 1_050).len(), 1);

    // Past TTL: stale, and excluded from fresh offerings.
    match cache.status("acme", &fp, 1_200) {
        CatalogStatus::Stale { age_secs } => assert_eq!(age_secs, 200),
        other => panic!("expected stale, got {other:?}"),
    }
    assert!(cache.fresh_offerings("acme", &fp, 1_200).is_empty());
    // But the rows are still present in the cache for explicit fallback display.
    assert_eq!(cache.get("acme", &fp).unwrap().offerings.len(), 1);
}

#[test]
fn ttl_zero_is_always_stale() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![],
        },
        0,
    );
    assert!(cache.get("acme", &fp).unwrap().is_stale(1_000));
}

#[test]
fn unknown_scope_reports_unknown_status() {
    let cache = ProviderCatalogCache::new();
    let fp = base_url_fingerprint("https://api.example.com");
    assert_eq!(cache.status("acme", &fp, 1_000), CatalogStatus::Unknown);
    assert!(cache.fresh_offerings("acme", &fp, 1_000).is_empty());
}

#[test]
fn refresh_failure_preserves_prior_rows_and_marks_failed() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "synth-chat-1".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        3_600,
    );

    for reason in [
        CatalogRefreshError::Unauthorized,
        CatalogRefreshError::Forbidden,
        CatalogRefreshError::NotFound,
        CatalogRefreshError::RateLimited,
        CatalogRefreshError::InvalidResponse,
        CatalogRefreshError::EmptyList,
        CatalogRefreshError::Network,
    ] {
        cache.record_failure("acme", &fp, reason);
        let entry = cache.get("acme", &fp).expect("entry survives failure");
        // Prior successful rows remain available after a failed refresh.
        assert_eq!(entry.offerings.len(), 1, "{reason:?} dropped prior rows");
        assert_eq!(entry.status, CatalogStatus::Failed { reason });
        // fetched_at is NOT bumped by a failure.
        assert_eq!(entry.fetched_at, 1_000);
        // ...but a Failed entry must NOT contribute to fresh offerings even
        // while still within its TTL window (now=1_100, ttl=3_600). The rows
        // are reachable only via get() for explicit fallback display.
        assert!(
            cache.fresh_offerings("acme", &fp, 1_100).is_empty(),
            "{reason:?}: failed entry served fresh offerings within TTL"
        );
        assert!(cache.all_fresh_offerings(1_100).is_empty());
        assert_eq!(
            cache.status("acme", &fp, 1_100),
            CatalogStatus::Failed { reason }
        );
    }
}

#[test]
fn failure_without_prior_creates_observable_empty_entry() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_failure("acme", &fp, CatalogRefreshError::Unauthorized);

    let entry = cache.get("acme", &fp).expect("failure is observable");
    assert!(entry.offerings.is_empty());
    assert_eq!(
        entry.status,
        CatalogStatus::Failed {
            reason: CatalogRefreshError::Unauthorized
        }
    );
}

#[test]
fn record_success_stamps_live_provenance_on_rows() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    // Row arrives mislabeled as Bundled; ingest must normalize provenance.
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 4_242,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "synth-chat-1".into(),
                endpoint_key: "chat".into(),
                source: CatalogSource::Bundled,
                ..Default::default()
            }],
        },
        3_600,
    );
    let entry = cache.get("acme", &fp).unwrap();
    assert_eq!(
        entry.offerings[0].source,
        CatalogSource::Live {
            base_url_fingerprint: fp,
            fetched_at: 4_242,
        }
    );
}

#[test]
fn cache_serialization_round_trips_and_contains_no_secrets() {
    let fp = base_url_fingerprint("https://api.example.com/v1");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "zhipuai".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_700,
            offerings: bundled_offerings_from_models_dev(&fixture()),
        },
        3_600,
    );

    let json = serde_json::to_string_pretty(&cache).expect("cache serializes");
    let round: ProviderCatalogCache = serde_json::from_str(&json).expect("cache round-trips");
    assert_eq!(round, cache);

    // The persisted shape carries model facts but has no field that could hold
    // a credential. Guard against a future field reintroducing one.
    let lower = json.to_lowercase();
    for needle in [
        "api_key",
        "apikey",
        "api-key",
        "authorization",
        "secret",
        "password",
        "bearer",
        "access_token",
    ] {
        assert!(
            !lower.contains(needle),
            "cache JSON unexpectedly contains `{needle}`"
        );
    }
    // Sanity: it did serialize meaningful provider/model facts.
    assert!(json.contains("glm-5.2"));
    assert!(json.contains("base_url_fingerprint"));
}

#[test]
fn all_fresh_offerings_spans_providers_and_skips_stale() {
    let fp = base_url_fingerprint("https://api.example.com");
    let mut cache = ProviderCatalogCache::new();
    cache.record_success(
        ProviderCatalogDelta {
            provider: "acme".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 1_000,
            offerings: vec![CatalogOffering {
                provider: "acme".into(),
                wire_model_id: "fresh-row".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        3_600,
    );
    cache.record_success(
        ProviderCatalogDelta {
            provider: "beta".into(),
            base_url_fingerprint: fp.clone(),
            fetched_at: 0,
            offerings: vec![CatalogOffering {
                provider: "beta".into(),
                wire_model_id: "stale-row".into(),
                endpoint_key: "chat".into(),
                ..Default::default()
            }],
        },
        10, // tiny ttl → stale at now=1_100
    );

    let fresh = cache.all_fresh_offerings(1_100);
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].wire_model_id, "fresh-row");

    // #4139: pickers still see stale rows; only the fresh helper drops them.
    let visible = cache.all_visible_offerings(1_100);
    assert_eq!(visible.len(), 2);
    assert!(visible.iter().any(|row| row.wire_model_id == "fresh-row"));
    assert!(visible.iter().any(|row| row.wire_model_id == "stale-row"));
}

#[test]
fn snapshot_feeds_route_resolver_offerings() {
    // The compiled snapshot projects into the exact type RouteResolver consumes,
    // proving catalog rows reach routing only through the offering seam.
    let snapshot = CatalogCompiler::new().with_models_dev(&fixture()).compile();
    let offerings = snapshot.to_offerings();

    let glm = offerings
        .iter()
        .find(|o| o.provider.as_str() == "zhipuai" && o.wire_model_id.as_str() == "glm-5.2")
        .expect("GLM offering reaches the route resolver seam");
    assert_eq!(glm.limits.context_tokens, Some(1_000_000));
    assert_eq!(glm.limits.output_tokens, Some(131_072));
    // Audio-only row never becomes a routing offering.
    assert!(
        !offerings
            .iter()
            .any(|o| o.wire_model_id.as_str() == "glm-voice")
    );
}

// ---------------------------------------------------------------------------
// #3385 / #4188: the committed offline/stale bundled Models.dev asset.
// ---------------------------------------------------------------------------

#[test]
fn bundled_asset_parses() {
    // The committed asset must `include_str!`-load and deserialize into the
    // parser's `ModelsDevCatalog` shape. This is the build-time guard that keeps
    // `bundled_models_dev_catalog()` panic-free in shipped builds.
    let catalog = ModelsDevCatalog::parse_json(BUNDLED_MODELS_DEV_JSON)
        .expect("committed bundled asset must be valid Models.dev JSON");
    assert!(
        !catalog.providers.is_empty(),
        "bundled asset must carry provider rows"
    );
    // The helper returns the same parsed catalog.
    assert_eq!(bundled_models_dev_catalog(), catalog);
}

#[test]
fn bundled_asset_meta_describes_offline_fallback_not_competing_truth() {
    // #4188: the asset must document itself as offline/stale fallback, not a
    // competing curated source of truth alongside live Models.dev.
    let raw: serde_json::Value =
        serde_json::from_str(BUNDLED_MODELS_DEV_JSON).expect("bundled JSON");
    let meta = raw
        .get("_meta")
        .and_then(|m| m.as_object())
        .expect("_meta object");
    let role = meta
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        role.to_ascii_lowercase().contains("not a competing"),
        "_meta.role must demote the bundled asset: {role}"
    );
    assert!(
        role.to_ascii_lowercase().contains("live"),
        "_meta.role must point at live Models.dev preference: {role}"
    );
}

#[test]
fn bundled_asset_yields_real_chat_offerings_for_key_models() {
    let rows = bundled_catalog_offerings();
    assert!(
        rows.len() >= 20,
        "expected dozens of bundled chat offerings, got {}",
        rows.len()
    );

    // A GLM and a Kimi row carry their real (non-default) context windows,
    // proving real facts flow rather than `RouteLimits::default()` (unknown).
    let glm = find(&rows, "zai", "GLM-5.2");
    assert_eq!(glm.limit.as_ref().and_then(|l| l.context), Some(1_000_000));
    assert!(glm.default_for_provider);

    let kimi = find(&rows, "moonshot", "kimi-k2.7-code");
    assert_eq!(kimi.limit.as_ref().and_then(|l| l.context), Some(262_144));

    let minimax_m3 = find(&rows, "minimax-anthropic", "MiniMax-M3");
    assert_eq!(
        minimax_m3.limit.as_ref().and_then(|limit| limit.context),
        Some(1_000_000)
    );
    let input_modalities = minimax_m3
        .modalities
        .as_ref()
        .expect("M3 modalities")
        .input
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(input_modalities, ["text", "image", "video"]);
    assert_eq!(
        minimax_m3.reasoning_options[0]
            .get("default")
            .and_then(serde_json::Value::as_str),
        Some("disabled")
    );

    let minimax_m2_7 = find(&rows, "minimax-anthropic", "MiniMax-M2.7");
    assert_eq!(
        minimax_m2_7.limit.as_ref().and_then(|limit| limit.context),
        Some(204_800)
    );
    assert_eq!(
        minimax_m2_7.reasoning_options[0]
            .get("default")
            .and_then(serde_json::Value::as_str),
        Some("always_on")
    );

    // Audio/TTS rows are absent (the asset only ships chat models, but assert
    // the filter contract anyway).
    assert!(
        rows.iter().all(|r| !r.wire_model_id.contains("tts")),
        "no TTS rows should reach the offering layer"
    );
}

#[test]
fn bundled_asset_pricing_is_honest() {
    let rows = bundled_catalog_offerings();

    // DeepSeek-native rows are intentionally unpriced here (priced via the
    // time-aware DeepSeek table elsewhere); pricing them would also break the
    // route layer's `unpriced_offering_stays_unknown` invariant.
    let deepseek = find(&rows, "deepseek", "deepseek-v4-pro");
    assert!(
        deepseek.cost.is_none(),
        "DeepSeek-native rows must stay unpriced in the bundled asset"
    );

    // Any row that *does* carry a cost must expose a usable input/output rate
    // (the honesty rule: no cache-only / empty cost objects that would render as
    // a rate-less Token at the route layer).
    for row in &rows {
        if let Some(cost) = row.cost.as_ref() {
            assert!(
                cost.input.is_some() || cost.output.is_some(),
                "{}/{}: priced row must have an input or output rate",
                row.provider,
                row.wire_model_id
            );
        }
    }

    // A sampled priced row matches the in-repo USD table (crates/tui pricing):
    // GLM-5.1 at the 2026-07-09 Z.ai published rates.
    let glm51 = find(&rows, "zai", "glm-5.1");
    let cost = glm51.cost.as_ref().expect("glm-5.1 is priced");
    assert_eq!(cost.input, Some(1.40));
    assert_eq!(cost.output, Some(4.40));
    assert_eq!(cost.cache_read, Some(0.26));

    // M3 has input-length and service tiers that the flat catalog cost shape
    // cannot represent, so the bundled route row stays honestly unpriced.
    let minimax_m3 = find(&rows, "minimax-anthropic", "MiniMax-M3");
    assert!(minimax_m3.cost.is_none());

    let minimax_m2_7 = find(&rows, "minimax-anthropic", "MiniMax-M2.7");
    let cost = minimax_m2_7.cost.as_ref().expect("M2.7 is priced");
    assert_eq!(cost.input, Some(0.30));
    assert_eq!(cost.output, Some(1.20));
    assert_eq!(cost.cache_read, Some(0.06));
    assert_eq!(cost.cache_write, Some(0.375));
}

#[test]
fn live_offerings_normalize_models_dev_provider_aliases() {
    // Live Models.dev ids that must map onto CodeWhale kinds (#4186/#4187).
    let raw = r#"{
      "models": {},
      "providers": {
        "moonshotai": {
          "id": "moonshotai",
          "models": {
            "kimi-k2.5": {
              "id": "kimi-k2.5",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        },
        "togetherai": {
          "id": "togetherai",
          "models": {
            "deepseek-ai/DeepSeek-V4-Pro": {
              "id": "deepseek-ai/DeepSeek-V4-Pro",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        },
        "zhipuai": {
          "id": "zhipuai",
          "models": {
            "glm-5.2": {
              "id": "glm-5.2",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        },
        "brand-new-gateway": {
          "id": "brand-new-gateway",
          "models": {
            "x-1": {
              "id": "x-1",
              "modalities": { "input": ["text"], "output": ["text"] }
            }
          }
        }
      }
    }"#;
    let catalog = ModelsDevCatalog::parse_json(raw).expect("fixture parses");
    let rows = live_offerings_from_models_dev(&catalog, "fp-models-dev", 1_700);

    assert_eq!(
        find(&rows, "moonshot", "kimi-k2.5").source,
        CatalogSource::Live {
            base_url_fingerprint: "fp-models-dev".into(),
            fetched_at: 1_700,
        }
    );
    find(&rows, "together", "deepseek-ai/DeepSeek-V4-Pro");
    find(&rows, "zai", "glm-5.2");
    // Unknown upstream providers keep their Models.dev id.
    find(&rows, "brand-new-gateway", "x-1");
    assert!(rows.iter().all(|r| r.provider != "moonshotai"));
    assert!(rows.iter().all(|r| r.provider != "togetherai"));
    assert!(rows.iter().all(|r| r.provider != "zhipuai"));
}
