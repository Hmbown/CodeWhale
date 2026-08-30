//! Process-wide cost-accrual side-channel (#526).
//!
//! Background LLM calls outside the main turn-complete path
//! (compaction summaries) used
//! to drop their token usage on the floor — the dashboard's
//! session-cost only saw the parent turn's tokens, so a long
//! session that triggered compaction under-reported
//! cost by however many tokens those background calls consumed.
//!
//! Mirrors the [`crate::retry_status`] pattern: background callers
//! call [`crate::cost_status::report_effective_route`] after each
//! `client.create_message`, the TUI
//! render loop calls [`drain`] every frame, and any drained amount
//! gets folded into `App::accrue_subagent_cost_estimate`.
//!
//! Why a side-channel and not a plumbed callback: the leaky callers
//! (`compaction.rs`) are
//! engine-internal machinery without a direct handle to `App` or
//! the engine's event channel. A side-channel keeps the change
//! surface tiny — one new `report` line per call site — and any
//! future background caller (summarizers, retrieval helpers) gets
//! accrued for free without further plumbing.
//!
//! ## One pool, not a pile of counters (#4318)
//!
//! Money and the *completeness* of that money are one fact, so they live in one
//! mutex-guarded [`PendingBackgroundCost`] that [`drain`] takes atomically.
//! Splitting them across free-standing atomics made two things go wrong at once:
//! a drain could observe a total without the counters that explain it, and every
//! new global was another piece of state a parallel test had to remember to
//! reset. There is exactly one *drainable cost pool*. The runtime-owner journal
//! below is a separate route/usage copy (never another money counter), and the
//! shared test reset clears both stores.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, Utc};

use crate::config::ApiProvider;
use crate::models::Usage;
use crate::pricing::{CostEstimate, TurnCostAudit};
use crate::route_billing::BillingPresentation;

/// Everything a drained background accrual needs to be explained.
///
/// The money and the coverage/provenance that qualify it are drained together,
/// so `/cost` can never show a background subtotal whose completeness came from
/// a different observation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingBackgroundCost {
    /// Summed cost of the background turns that were priced.
    pub estimate: CostEstimate,
    /// Background turns that produced an authoritative price.
    pub priced_turns: u32,
    /// Background turns that were money-metered (or of unknown basis) but
    /// produced no authoritative price, so their spend is missing.
    pub unpriced_turns: u32,
    /// Money-metered turns authoritatively priced in CNY.
    pub cny_priced_turns: u32,
    /// Money-metered turns missing authoritative CNY pricing.
    pub cny_unpriced_turns: u32,
    /// Stable reason labels for the unpriced turns.
    pub unpriced_reasons: BTreeSet<&'static str>,
    pub cny_unpriced_reasons: BTreeSet<&'static str>,
    /// Token classes used on a background route that carry no published price.
    pub unpriced_classes: BTreeSet<&'static str>,
    /// Provenance labels of the pricing rows that were applied or attempted.
    pub pricing_provenances: BTreeSet<&'static str>,
    /// Live-pricing downgrade receipts, when a live catalog row could not be
    /// verified for the endpoint that served the turn.
    pub live_pricing_defects: BTreeSet<&'static str>,
    /// Live pricing failed and no bundled row could price the turn. Kept
    /// separate so `/cost` never claims a bundled fallback was used when the
    /// result is actually unavailable.
    pub live_pricing_unusable_defects: BTreeSet<&'static str>,
    /// One redacted receipt per distinct background route that reported.
    ///
    /// See [`EffectiveRouteEnvelope::receipt`] for the exact contents; these carry
    /// provider identity, endpoint *fingerprint*, billing surface, wire model,
    /// and currency — never a URL, key, token, or filesystem path.
    pub route_receipts: BTreeSet<String>,
    /// Durable, redacted identities of provider responses folded into this
    /// batch. These travel with the money so a session snapshot can make a
    /// replay idempotent after reload.
    pub usage_source_fingerprints: BTreeSet<String>,
}

/// Immutable, non-secret route evidence captured before a provider request.
/// It contains enough information to audit the eventual usage without reading
/// mutable parent/app config at completion time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct EffectiveRouteEnvelope {
    pub provider: ApiProvider,
    pub provider_identity: String,
    pub model: String,
    pub billing_surface: Option<String>,
    pub endpoint_fingerprint: Option<String>,
    /// Frozen provider-live rates captured from the exact fresh catalog scope
    /// at CodeWhale's pre-permit application-dispatch boundary. Legacy
    /// receipts omit this and therefore cannot meter a reviewed custom route
    /// retroactively.
    #[serde(
        default,
        deserialize_with = "crate::provider_catalog_live::deserialize_optional_provider_live_pricing"
    )]
    pub provider_live_pricing: Option<crate::provider_catalog_live::ProviderLivePricingQuote>,
    #[serde(default)]
    pub billing_mode: RouteBillingMode,
    pub dispatched_at: DateTime<Utc>,
}

impl serde::Serialize for EffectiveRouteEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let route = self.sanitized_for_persistence();
        let mut state = serializer.serialize_struct("EffectiveRouteEnvelope", 8)?;
        state.serialize_field("provider", &route.provider)?;
        state.serialize_field("provider_identity", &route.provider_identity)?;
        state.serialize_field("model", &route.model)?;
        state.serialize_field("billing_surface", &route.billing_surface)?;
        state.serialize_field("endpoint_fingerprint", &route.endpoint_fingerprint)?;
        state.serialize_field("provider_live_pricing", &route.provider_live_pricing)?;
        state.serialize_field("billing_mode", &route.billing_mode)?;
        state.serialize_field("dispatched_at", &route.dispatched_at)?;
        state.end()
    }
}

/// One provider usage payload paired with the immutable route that served it.
///
/// Runtime hosts persist these for model calls made below the parent turn
/// (sub-agents, review/verify/RLM tools, and compaction). Keeping route and
/// usage together makes the record independently auditable and prevents a
/// later provider/model selection from changing its price.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EffectiveRouteUsage {
    pub route: EffectiveRouteEnvelope,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteBillingMode {
    Metered,
    Subscription,
    Local,
    #[default]
    Unknown,
}

impl From<BillingPresentation> for RouteBillingMode {
    fn from(value: BillingPresentation) -> Self {
        match value {
            BillingPresentation::Metered => Self::Metered,
            BillingPresentation::Subscription(_) => Self::Subscription,
            BillingPresentation::Local => Self::Local,
            BillingPresentation::Unknown => Self::Unknown,
        }
    }
}

impl EffectiveRouteEnvelope {
    #[must_use]
    pub fn capture(
        config: Option<&crate::config::Config>,
        provider: ApiProvider,
        provider_identity: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<&str>,
        dispatched_at: DateTime<Utc>,
    ) -> Self {
        let provider_identity = provider_identity.into();
        let model = model.into();
        let billing = config.map_or_else(
            || crate::route_billing::for_endpoint_without_config(provider, base_url),
            |config| crate::route_billing::for_route(config, provider),
        );
        let endpoint_fingerprint = base_url.and_then(endpoint_fingerprint);
        let provider_live_pricing =
            u64::try_from(dispatched_at.timestamp())
                .ok()
                .and_then(|dispatched_at_unix| {
                    endpoint_fingerprint.as_deref().and_then(|fingerprint| {
                        crate::provider_catalog_live::fresh_provider_live_pricing_quote_at(
                            provider,
                            &provider_identity,
                            &model,
                            fingerprint,
                            dispatched_at_unix,
                        )
                    })
                });
        Self {
            provider,
            provider_identity: sanitize_persisted_route_label(&provider_identity),
            model: sanitize_persisted_route_label(&model),
            billing_surface: crate::route_billing::billing_surface_for_dispatch(
                config, provider, base_url,
            )
            .map(str::to_string),
            endpoint_fingerprint,
            provider_live_pricing,
            billing_mode: billing.into(),
            dispatched_at,
        }
    }

    #[must_use]
    pub fn audit(&self, usage: &Usage) -> TurnCostAudit {
        let reviewed_custom_metered = crate::pricing::reviewed_custom_route_is_metered(
            self.provider,
            Some(&self.provider_identity),
            self.endpoint_fingerprint.as_deref(),
        );
        match self.billing_mode {
            RouteBillingMode::Subscription | RouteBillingMode::Local => {
                return TurnCostAudit::unpriced(crate::pricing::UnpricedReason::NotMoneyMetered);
            }
            RouteBillingMode::Unknown if !reviewed_custom_metered => {
                return TurnCostAudit::unpriced(
                    crate::pricing::UnpricedReason::UnknownBillingBasis,
                );
            }
            RouteBillingMode::Metered | RouteBillingMode::Unknown => {}
        }
        crate::pricing::audit_turn_cost_for_route_on_endpoint_for_identity_at(
            self.provider,
            Some(&self.provider_identity),
            &self.model,
            self.billing_surface.as_deref(),
            self.endpoint_fingerprint.as_deref(),
            self.provider_live_pricing.as_ref(),
            usage,
            self.dispatched_at,
        )
    }

    #[must_use]
    pub fn receipt(&self, audit: &TurnCostAudit) -> String {
        let route = self.sanitized_for_persistence();
        route_receipt(
            route.provider,
            Some(&route.provider_identity),
            &route.model,
            route.billing_surface.as_deref(),
            route.endpoint_fingerprint.as_deref(),
            route.billing_mode,
            currency_tag(audit),
        )
    }

    /// Redact filesystem-like labels before a route crosses a persistence or
    /// metadata boundary. Ordinary provider model namespaces such as
    /// `anthropic/claude-*` remain intact; absolute/local path forms do not.
    #[must_use]
    pub fn sanitized_for_persistence(&self) -> Self {
        let mut route = self.clone();
        route.provider_identity = sanitize_persisted_route_label(&route.provider_identity);
        route.model = sanitize_persisted_route_label(&route.model);
        route.billing_surface = route
            .billing_surface
            .as_deref()
            .map(sanitize_persisted_route_label);
        route.endpoint_fingerprint =
            route
                .endpoint_fingerprint
                .as_deref()
                .and_then(|fingerprint| {
                    let fingerprint = fingerprint.trim();
                    (fingerprint.len() == 64
                        && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()))
                    .then(|| fingerprint.to_ascii_lowercase())
                });
        let quote_is_valid = route
            .provider_live_pricing
            .as_ref()
            .zip(route.endpoint_fingerprint.as_deref())
            .and_then(|(quote, fingerprint)| {
                u64::try_from(route.dispatched_at.timestamp())
                    .ok()
                    .and_then(|dispatched_at_unix| {
                        quote.pricing_for_route(
                            route.provider,
                            &route.provider_identity,
                            &route.model,
                            fingerprint,
                            dispatched_at_unix,
                        )
                    })
            })
            .is_some();
        if !quote_is_valid {
            route.provider_live_pricing = None;
        }
        route
    }
}

fn receipt_with_usage_classes(mut receipt: String, usage: &Usage) -> String {
    let classes = crate::pricing::token_usage_for_pricing(usage);
    if classes.cache_write > 0 {
        receipt.push_str(" cache_write=yes");
    }
    if usage.reasoning_tokens.unwrap_or(0) > 0 {
        receipt.push_str(" reasoning=yes");
    }
    receipt
}

/// Canonical redacted route receipt for one exact usage payload.
#[must_use]
pub fn effective_route_usage_receipt(
    route: &EffectiveRouteEnvelope,
    audit: &TurnCostAudit,
    usage: &Usage,
) -> String {
    receipt_with_usage_classes(route.receipt(audit), usage)
}

/// Canonical `child_*` token and route metadata for tools that make their own
/// LLM calls (`review`, `verify`, and `rlm`). Keeping this next to the immutable
/// route envelope prevents the pure model types from depending on app config.
#[must_use]
pub fn child_usage_metadata_fields(
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
) -> serde_json::Map<String, serde_json::Value> {
    let route = route.sanitized_for_persistence();
    let mut fields = serde_json::Map::new();
    fields.insert("child_provider".into(), serde_json::json!(route.provider));
    fields.insert(
        "child_provider_identity".into(),
        serde_json::json!(route.provider_identity),
    );
    fields.insert("child_model".into(), serde_json::json!(route.model));
    fields.insert(
        "child_billing_surface".into(),
        serde_json::json!(route.billing_surface),
    );
    fields.insert(
        "child_endpoint_fingerprint".into(),
        serde_json::json!(route.endpoint_fingerprint),
    );
    fields.insert(
        "child_provider_live_pricing".into(),
        serde_json::json!(route.provider_live_pricing),
    );
    fields.insert(
        "child_billing_mode".into(),
        serde_json::json!(route.billing_mode),
    );
    fields.insert(
        "child_dispatched_at".into(),
        serde_json::json!(route.dispatched_at),
    );
    fields.insert(
        "child_input_tokens".into(),
        serde_json::json!(usage.input_tokens),
    );
    fields.insert(
        "child_output_tokens".into(),
        serde_json::json!(usage.output_tokens),
    );
    fields.insert(
        "child_prompt_cache_hit_tokens".into(),
        serde_json::json!(usage.prompt_cache_hit_tokens),
    );
    fields.insert(
        "child_prompt_cache_miss_tokens".into(),
        serde_json::json!(usage.prompt_cache_miss_tokens),
    );
    fields.insert(
        "child_prompt_cache_write_tokens".into(),
        serde_json::json!(usage.prompt_cache_write_tokens),
    );
    // Informational: reasoning tokens are already included in output tokens.
    fields.insert(
        "child_reasoning_tokens".into(),
        serde_json::json!(usage.reasoning_tokens),
    );
    fields.insert(
        "child_reasoning_replay_tokens".into(),
        serde_json::json!(usage.reasoning_replay_tokens),
    );
    fields.insert(
        "child_server_tool_use".into(),
        serde_json::json!(usage.server_tool_use),
    );
    fields
}

/// Merge canonical child usage into a tool metadata object.
pub fn attach_child_usage_metadata(
    metadata: &mut serde_json::Value,
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
) {
    if let Some(object) = metadata.as_object_mut() {
        object.extend(child_usage_metadata_fields(route, usage));
    }
}

/// Maximum number of distinct routed-usage segments accepted from one tool
/// result. RLM reserves against the same bound before dispatch, so a valid
/// producer never has to discard a provider receipt after doing the work.
pub const MAX_CHILD_USAGE_RECORDS: usize = 64;

const CHILD_USAGE_RECORDS_KEY: &str = "child_usage_records";
const CHILD_USAGE_DROP_RECORDS_KEY: &str = "child_usage_drop_records";
const CHILD_USAGE_DROPPED_RECORDS_KEY: &str = "child_usage_dropped_records";

/// Attach a bounded batch of routed child usage to tool metadata.
///
/// The source identity is reduced to a one-way fingerprint before metadata
/// can enter a transcript. Routes pass through their persistence sanitizer,
/// so neither a raw response id nor an endpoint/credential can hitch a ride.
/// New consumers prefer this batch over the legacy single `child_*` fields.
/// Attach a bounded batch containing both exact usage receipts and exact
/// provider-success/missing-usage route receipts.
pub fn attach_child_usage_batch_metadata(
    metadata: &mut serde_json::Value,
    batch: &RuntimeUsageBatch,
) {
    let Some(object) = metadata.as_object_mut() else {
        return;
    };
    let retained_records = batch
        .records
        .iter()
        .take(MAX_CHILD_USAGE_RECORDS)
        .map(|record| {
            serde_json::json!({
                "source_id": format!(
                    "routed:{}",
                    usage_source_fingerprint(&record.source_id)
                ),
                "route": record.usage.route.sanitized_for_persistence(),
                "usage": record.usage.usage,
            })
        })
        .collect::<Vec<_>>();
    let remaining = MAX_CHILD_USAGE_RECORDS.saturating_sub(retained_records.len());
    let retained_drops = batch
        .drop_records
        .iter()
        .take(remaining)
        .map(|record| {
            serde_json::json!({
                "source_id": format!(
                    "routed:{}",
                    usage_source_fingerprint(&record.source_id)
                ),
                "route": record.route.sanitized_for_persistence(),
            })
        })
        .collect::<Vec<_>>();
    object.insert(
        CHILD_USAGE_RECORDS_KEY.into(),
        serde_json::json!(retained_records),
    );
    object.insert(
        CHILD_USAGE_DROP_RECORDS_KEY.into(),
        serde_json::json!(retained_drops),
    );
    let usage_overflow = batch.records.len().saturating_sub(MAX_CHILD_USAGE_RECORDS);
    let dropped_records = batch
        .dropped_records
        .max(u64::try_from(batch.drop_records.len()).unwrap_or(u64::MAX))
        .saturating_add(u64::try_from(usage_overflow).unwrap_or(u64::MAX));
    if dropped_records > 0 {
        object.insert(
            CHILD_USAGE_DROPPED_RECORDS_KEY.into(),
            serde_json::json!(dropped_records),
        );
    } else {
        object.remove(CHILD_USAGE_DROPPED_RECORDS_KEY);
    }
}

/// Parse the preferred routed child-usage batch.
///
/// `None` means the batch key was absent and callers may use the legacy
/// single-record parser. Once the key is present, malformed/overflow entries
/// are represented by `dropped_records` instead of falling back and risking a
/// partial subtotal being presented as complete.
#[must_use]
pub fn child_usage_records_from_metadata(
    metadata: &serde_json::Value,
) -> Option<RuntimeUsageBatch> {
    let value = metadata.get(CHILD_USAGE_RECORDS_KEY)?;
    let drop_values = metadata
        .get(CHILD_USAGE_DROP_RECORDS_KEY)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let declared_dropped = metadata
        .get(CHILD_USAGE_DROPPED_RECORDS_KEY)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let Some(values) = value.as_array() else {
        return Some(RuntimeUsageBatch {
            records: Vec::new(),
            drop_records: Vec::new(),
            dropped_records: declared_dropped.saturating_add(1),
        });
    };

    let overflow = values.len().saturating_sub(MAX_CHILD_USAGE_RECORDS);
    let mut batch = RuntimeUsageBatch {
        records: Vec::with_capacity(values.len().min(MAX_CHILD_USAGE_RECORDS)),
        drop_records: Vec::with_capacity(drop_values.len().min(MAX_CHILD_USAGE_RECORDS)),
        dropped_records: declared_dropped
            .max(u64::try_from(drop_values.len()).unwrap_or(u64::MAX))
            .saturating_add(u64::try_from(overflow).unwrap_or(u64::MAX)),
    };
    for value in values.iter().take(MAX_CHILD_USAGE_RECORDS) {
        let parsed = (|| {
            let source_id = value.get("source_id")?.as_str()?;
            let route =
                serde_json::from_value::<EffectiveRouteEnvelope>(value.get("route")?.clone())
                    .ok()?
                    .sanitized_for_persistence();
            let usage = serde_json::from_value::<Usage>(value.get("usage")?.clone()).ok()?;
            Some(RuntimeUsageRecord {
                // Treat metadata as an untrusted persistence boundary. A
                // stable hash preserves idempotence without retaining the
                // producer's raw identifier.
                source_id: format!("metadata:{}", usage_source_fingerprint(source_id)),
                usage: EffectiveRouteUsage { route, usage },
            })
        })();
        if let Some(record) = parsed {
            batch.records.push(record);
        } else {
            batch.dropped_records = batch.dropped_records.saturating_add(1);
        }
    }
    let remaining = MAX_CHILD_USAGE_RECORDS.saturating_sub(batch.records.len());
    for value in drop_values.iter().take(remaining) {
        let parsed = (|| {
            let source_id = value.get("source_id")?.as_str()?;
            let route =
                serde_json::from_value::<EffectiveRouteEnvelope>(value.get("route")?.clone())
                    .ok()?
                    .sanitized_for_persistence();
            Some(RuntimeUsageDropRecord {
                source_id: format!("metadata:{}", usage_source_fingerprint(source_id)),
                route,
            })
        })();
        if let Some(record) = parsed {
            batch.drop_records.push(record);
        } else if declared_dropped < u64::try_from(drop_values.len()).unwrap_or(u64::MAX) {
            batch.dropped_records = batch.dropped_records.saturating_add(1);
        }
    }
    Some(batch)
}

/// Rehydrate the immutable route envelope emitted with child usage. Legacy or
/// incomplete metadata becomes an explicitly unknown route and never borrows
/// mutable parent-session facts.
#[must_use]
pub fn child_route_envelope_from_metadata(
    metadata: &serde_json::Value,
) -> Option<EffectiveRouteEnvelope> {
    let model = metadata.get("child_model")?.as_str()?.to_string();
    let provider = metadata
        .get("child_provider")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let provider_identity = metadata
        .get("child_provider_identity")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let billing_mode = metadata
        .get("child_billing_mode")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let dispatched_at = metadata
        .get("child_dispatched_at")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());

    let complete = provider.is_some()
        && provider_identity.is_some()
        && billing_mode.is_some()
        && dispatched_at.is_some();
    Some(
        EffectiveRouteEnvelope {
            provider: provider.unwrap_or(ApiProvider::Custom),
            provider_identity: provider_identity.unwrap_or_else(|| "legacy-unreported".to_string()),
            model,
            billing_surface: metadata
                .get("child_billing_surface")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            endpoint_fingerprint: metadata
                .get("child_endpoint_fingerprint")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            provider_live_pricing: metadata
                .get("child_provider_live_pricing")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok()),
            billing_mode: billing_mode
                .filter(|_| complete)
                .unwrap_or(RouteBillingMode::Unknown),
            dispatched_at: dispatched_at.unwrap_or_else(|| {
                DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch is representable")
            }),
        }
        .sanitized_for_persistence(),
    )
}

/// Rehydrate the complete child usage payload emitted by
/// [`attach_child_usage_metadata`]. The presence of a canonical child token
/// field is significant even when every value is zero: a zero-usage provider
/// call still needs a route receipt and coverage classification.
#[must_use]
pub fn child_usage_from_metadata(metadata: &serde_json::Value) -> Option<Usage> {
    const TOKEN_FIELDS: &[&str] = &[
        "child_input_tokens",
        "child_output_tokens",
        "child_prompt_cache_hit_tokens",
        "child_prompt_cache_miss_tokens",
        "child_prompt_cache_write_tokens",
        "child_reasoning_tokens",
        "child_reasoning_replay_tokens",
    ];
    if !TOKEN_FIELDS
        .iter()
        .any(|field| metadata.get(field).is_some())
    {
        return None;
    }

    fn u32_field(metadata: &serde_json::Value, field: &str) -> Option<u32> {
        metadata
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .map(|value| u32::try_from(value).unwrap_or(u32::MAX))
    }

    Some(Usage {
        input_tokens: u32_field(metadata, "child_input_tokens").unwrap_or(0),
        output_tokens: u32_field(metadata, "child_output_tokens").unwrap_or(0),
        prompt_cache_hit_tokens: u32_field(metadata, "child_prompt_cache_hit_tokens"),
        prompt_cache_miss_tokens: u32_field(metadata, "child_prompt_cache_miss_tokens"),
        prompt_cache_write_tokens: u32_field(metadata, "child_prompt_cache_write_tokens"),
        reasoning_tokens: u32_field(metadata, "child_reasoning_tokens"),
        reasoning_replay_tokens: u32_field(metadata, "child_reasoning_replay_tokens"),
        server_tool_use: metadata
            .get("child_server_tool_use")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
    })
}

impl PendingBackgroundCost {
    /// Whether anything at all was accrued.
    ///
    /// Compared against `Default` rather than checking a subset of fields, so a
    /// field added later cannot be silently left out of the emptiness test.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Default)]
struct ScopedPendingBackgroundCost {
    generation: u64,
    pending: PendingBackgroundCost,
    /// All provider responses accepted in this session generation, including
    /// batches already drained into the live session projection.
    seen_usage_source_fingerprints: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostScopeToken(u64);

#[cfg(not(test))]
static PENDING: OnceLock<Mutex<ScopedPendingBackgroundCost>> = OnceLock::new();

#[cfg(test)]
static TEST_PENDING: OnceLock<
    Mutex<std::collections::HashMap<std::thread::ThreadId, ScopedPendingBackgroundCost>>,
> = OnceLock::new();

fn with_pending_state_mut<R>(f: impl FnOnce(&mut ScopedPendingBackgroundCost) -> R) -> R {
    #[cfg(not(test))]
    {
        let mut pending = PENDING
            .get_or_init(|| Mutex::new(ScopedPendingBackgroundCost::default()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        f(&mut pending)
    }
    #[cfg(test)]
    {
        // Rust tests run concurrently. A test-local collector prevents a UI
        // drain or successful purge in one test from stealing another test's
        // accounting. Tokio's default test runtime is current-thread, so async
        // helpers retain this scope across awaits.
        let mut by_thread = TEST_PENDING
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        f(by_thread.entry(std::thread::current().id()).or_default())
    }
}

/// Runtime accounting gets a cloned, owner-scoped copy of compaction usage.
/// This journal is deliberately separate from the TUI pending-money pool:
/// taking one runtime owner's records cannot steal or reset the foreground
/// session's `/cost` state.
const MAX_RUNTIME_USAGE_RECORDS_PER_OWNER: usize = 64;

#[derive(Default)]
struct OwnerRuntimeUsageJournal {
    records: VecDeque<RuntimeUsageRecord>,
    drop_records: VecDeque<RuntimeUsageDropRecord>,
    dropped_records: u64,
    dropped_source_fingerprints: HashSet<String>,
    dropped_fingerprint_overflowed: bool,
}

type RuntimeUsageJournal = HashMap<String, OwnerRuntimeUsageJournal>;

/// Bounded fallback batch returned when no synchronous runtime sink was
/// available. `dropped_records` is persisted into the turn so aggregates fail
/// closed instead of silently presenting a partial cost as complete.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeUsageBatch {
    pub records: Vec<RuntimeUsageRecord>,
    /// Exact provider-success calls whose usage payload was absent. The
    /// bounded records retain route billing truth; `dropped_records` remains
    /// the authoritative total and may exceed this vector after overflow.
    pub drop_records: Vec<RuntimeUsageDropRecord>,
    pub dropped_records: u64,
}

/// One owner-scoped usage report with the stable provider-call identity used
/// to make durable replay idempotent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeUsageRecord {
    pub source_id: String,
    pub usage: EffectiveRouteUsage,
}

/// One provider-success response that omitted usage metadata.
///
/// The frozen route is required to distinguish money-metered calls from
/// subscription/local calls without consulting mutable completion-time config.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimeUsageDropRecord {
    pub source_id: String,
    pub route: EffectiveRouteEnvelope,
}

pub(crate) type RuntimeUsageSink = Arc<dyn Fn(RuntimeUsageRecord) -> bool + Send + Sync>;
pub(crate) type RuntimeUsageDropSink = Arc<dyn Fn(RuntimeUsageDropRecord) -> bool + Send + Sync>;

struct RuntimeUsageSinkEntry {
    sink: RuntimeUsageSink,
    dropped_sink: Option<RuntimeUsageDropSink>,
    leases: usize,
    terminal: bool,
}

/// Keeps an owner sink alive while a detached child can still report usage.
/// The runtime turn may already be terminal; the last child release retires
/// the sink only after its final provider response has been durably appended.
#[derive(Debug)]
pub(crate) struct RuntimeUsageLease {
    owner: String,
    active: bool,
}

#[cfg(not(test))]
static RUNTIME_USAGE_JOURNAL: OnceLock<Mutex<RuntimeUsageJournal>> = OnceLock::new();

#[cfg(test)]
static TEST_RUNTIME_USAGE_JOURNAL: OnceLock<
    Mutex<std::collections::HashMap<std::thread::ThreadId, RuntimeUsageJournal>>,
> = OnceLock::new();

#[cfg(not(test))]
static RUNTIME_USAGE_SINKS: OnceLock<Mutex<HashMap<String, RuntimeUsageSinkEntry>>> =
    OnceLock::new();

/// Sinks are keyed by owner id, and owner ids in tests are short fixture
/// strings that repeat across tests. Under the default parallel test harness a
/// process-global map let one test's `register_runtime_usage_sink` replace
/// another's live sink, and let one test's `finish_runtime_usage_owner` retire
/// it — turning exactly-once child accounting into an order-dependent race.
/// Scoping by thread matches the pending-cost pool and the runtime journal,
/// which are already thread-scoped for the same reason.
#[cfg(test)]
#[allow(clippy::type_complexity)]
static TEST_RUNTIME_USAGE_SINKS: OnceLock<
    Mutex<HashMap<std::thread::ThreadId, HashMap<String, RuntimeUsageSinkEntry>>>,
> = OnceLock::new();

/// Run `f` against this scope's sink registry.
fn with_runtime_usage_sinks<R>(
    f: impl FnOnce(&mut HashMap<String, RuntimeUsageSinkEntry>) -> R,
) -> R {
    #[cfg(not(test))]
    {
        let mut sinks = RUNTIME_USAGE_SINKS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(&mut sinks)
    }
    #[cfg(test)]
    {
        let mut by_thread = TEST_RUNTIME_USAGE_SINKS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(by_thread.entry(std::thread::current().id()).or_default())
    }
}

/// Like [`with_runtime_usage_sinks`], but does not create the registry when it
/// has never been initialized. Used on drop paths, where allocating a registry
/// to then find it empty would be pointless.
fn with_existing_runtime_usage_sinks<R>(
    f: impl FnOnce(&mut HashMap<String, RuntimeUsageSinkEntry>) -> R,
) -> Option<R> {
    #[cfg(not(test))]
    {
        let sinks = RUNTIME_USAGE_SINKS.get()?;
        let mut sinks = sinks.lock().unwrap_or_else(|error| error.into_inner());
        Some(f(&mut sinks))
    }
    #[cfg(test)]
    {
        let by_thread = TEST_RUNTIME_USAGE_SINKS.get()?;
        let mut by_thread = by_thread.lock().unwrap_or_else(|error| error.into_inner());
        let sinks = by_thread.get_mut(&std::thread::current().id())?;
        Some(f(sinks))
    }
}

fn with_runtime_usage_journal_mut<R>(f: impl FnOnce(&mut RuntimeUsageJournal) -> R) -> R {
    #[cfg(not(test))]
    {
        let mut journal = RUNTIME_USAGE_JOURNAL
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(&mut journal)
    }
    #[cfg(test)]
    {
        let mut by_thread = TEST_RUNTIME_USAGE_JOURNAL
            .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        f(by_thread.entry(std::thread::current().id()).or_default())
    }
}

fn record_runtime_usage(
    owner: &str,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
) {
    let owner = owner.trim();
    if owner.is_empty() {
        return;
    }
    let record = RuntimeUsageRecord {
        source_id: source_id.to_string(),
        usage: EffectiveRouteUsage {
            route: route.sanitized_for_persistence(),
            usage: usage.clone(),
        },
    };
    let sink =
        with_runtime_usage_sinks(|sinks| sinks.get(owner).map(|entry| Arc::clone(&entry.sink)));
    if sink.is_some_and(|sink| sink(record.clone())) {
        return;
    }
    with_runtime_usage_journal_mut(|journal| {
        let owner_journal = journal.entry(owner.to_string()).or_default();
        if owner_journal.records.len() == MAX_RUNTIME_USAGE_RECORDS_PER_OWNER {
            owner_journal.records.pop_front();
            owner_journal.dropped_records = owner_journal.dropped_records.saturating_add(1);
        }
        owner_journal.records.push_back(record);
    });
}

fn record_runtime_usage_drop(owner: &str, source_id: &str, route: &EffectiveRouteEnvelope) {
    let owner = owner.trim();
    if owner.is_empty() {
        return;
    }
    let fingerprint = usage_source_fingerprint(source_id);
    let sink = with_runtime_usage_sinks(|sinks| {
        sinks
            .get(owner)
            .and_then(|entry| entry.dropped_sink.as_ref().map(Arc::clone))
    });
    let record = RuntimeUsageDropRecord {
        source_id: source_id.to_string(),
        route: route.sanitized_for_persistence(),
    };
    if sink.is_some_and(|sink| sink(record.clone())) {
        return;
    }
    with_runtime_usage_journal_mut(|journal| {
        let owner_journal = journal.entry(owner.to_string()).or_default();
        if owner_journal
            .dropped_source_fingerprints
            .contains(&fingerprint)
        {
            return;
        }
        if owner_journal.dropped_source_fingerprints.len() < MAX_RUNTIME_USAGE_RECORDS_PER_OWNER {
            owner_journal
                .dropped_source_fingerprints
                .insert(fingerprint);
            owner_journal.drop_records.push_back(record);
            owner_journal.dropped_records = owner_journal.dropped_records.saturating_add(1);
        } else if !owner_journal.dropped_fingerprint_overflowed {
            // Preserve a bounded fail-closed overflow marker. Once the exact
            // identity ledger is full, further unknown ids share this one
            // marker so replays cannot grow the count without bound.
            owner_journal.dropped_fingerprint_overflowed = true;
            owner_journal.dropped_records = owner_journal.dropped_records.saturating_add(1);
        }
    });
}

fn record_runtime_usage_drop_count(owner: &str, source_id: &str, count: u64) {
    let owner = owner.trim();
    if owner.is_empty() || count == 0 {
        return;
    }
    let fingerprint = usage_source_fingerprint(source_id);
    with_runtime_usage_journal_mut(|journal| {
        let owner_journal = journal.entry(owner.to_string()).or_default();
        if owner_journal
            .dropped_source_fingerprints
            .contains(&fingerprint)
        {
            return;
        }
        if owner_journal.dropped_source_fingerprints.len() < MAX_RUNTIME_USAGE_RECORDS_PER_OWNER {
            owner_journal
                .dropped_source_fingerprints
                .insert(fingerprint);
            owner_journal.dropped_records = owner_journal.dropped_records.saturating_add(count);
        } else if !owner_journal.dropped_fingerprint_overflowed {
            owner_journal.dropped_fingerprint_overflowed = true;
            owner_journal.dropped_records = owner_journal.dropped_records.saturating_add(1);
        }
    });
}

/// Install a synchronous durability sink for one active runtime turn.
/// Compaction calls invoke this before they return to the engine, so a process
/// crash cannot erase already-reported usage from an in-memory journal.
#[cfg(test)]
pub(crate) fn register_runtime_usage_sink(owner: &str, sink: RuntimeUsageSink) {
    register_runtime_usage_sink_with_drop(owner, sink, None);
}

pub(crate) fn register_runtime_usage_sink_with_drop(
    owner: &str,
    sink: RuntimeUsageSink,
    dropped_sink: Option<RuntimeUsageDropSink>,
) {
    let owner = owner.trim();
    if owner.is_empty() {
        return;
    }
    with_runtime_usage_sinks(|sinks| {
        sinks.insert(
            owner.to_string(),
            RuntimeUsageSinkEntry {
                sink,
                dropped_sink,
                leases: 0,
                terminal: false,
            },
        );
    });
}

/// Redacted durable identity shared by runtime-turn, worker, and interactive
/// session accounting. Raw response ids never need to be persisted merely to
/// make replay idempotent.
#[must_use]
pub(crate) fn usage_source_fingerprint(source_id: &str) -> String {
    codewhale_config::catalog::base_url_fingerprint(source_id.trim())
}

/// Install the interactive session's synchronous runtime sink. A detached
/// child may report after the parent mailbox has sealed; its owner lease keeps
/// this sink alive, while the captured scope prevents a later session from
/// inheriting the spend.
#[cfg(test)]
pub(crate) fn register_interactive_runtime_usage_sink(owner: &str, scope: CostScopeToken) {
    register_runtime_usage_sink_with_drop(
        owner,
        Arc::new(move |record| record_interactive_runtime_usage(scope, record)),
        Some(Arc::new(move |record| {
            record_interactive_runtime_usage_drop(scope, record)
        })),
    );
}

/// Install an interactive sink whose stale-scope fallback is an origin-session
/// sidecar. `/new` may close the foreground pool while a detached provider call
/// is still running; the sidecar keeps that exact response with the old saved
/// session instead of either dropping it or contaminating the new one.
pub(crate) fn register_persistent_interactive_runtime_usage_sink(
    owner: &str,
    scope: CostScopeToken,
    session_id: &str,
    turn_id: &str,
) {
    let Ok(manager) = crate::session_manager::SessionManager::default_location() else {
        register_runtime_usage_sink_with_drop(
            owner,
            Arc::new(move |record| record_interactive_runtime_usage(scope, record)),
            Some(Arc::new(move |record| {
                record_interactive_runtime_usage_drop(scope, record)
            })),
        );
        return;
    };
    register_persistent_interactive_runtime_usage_sink_at(
        owner,
        scope,
        session_id,
        turn_id,
        manager.sessions_dir().to_path_buf(),
    );
}

fn register_persistent_interactive_runtime_usage_sink_at(
    owner: &str,
    scope: CostScopeToken,
    session_id: &str,
    turn_id: &str,
    sessions_dir: std::path::PathBuf,
) {
    let usage_session_id = session_id.to_string();
    let usage_turn_id = turn_id.to_string();
    let drop_session_id = usage_session_id.clone();
    let drop_turn_id = usage_turn_id.clone();
    let usage_sessions_dir = sessions_dir.clone();
    register_runtime_usage_sink_with_drop(
        owner,
        Arc::new(move |record| {
            if record_interactive_runtime_usage(scope, record.clone()) {
                return true;
            }
            crate::session_manager::SessionManager::new(usage_sessions_dir.clone())
                .and_then(|manager| {
                    manager.persist_late_runtime_usage(&usage_session_id, &usage_turn_id, &record)
                })
                .unwrap_or(false)
        }),
        Some(Arc::new(move |record| {
            if record_interactive_runtime_usage_drop(scope, record.clone()) {
                return true;
            }
            crate::session_manager::SessionManager::new(sessions_dir.clone())
                .and_then(|manager| {
                    manager.persist_late_runtime_drop(&drop_session_id, &drop_turn_id, &record)
                })
                .unwrap_or(false)
        })),
    );
}

/// Acquire an owner lease for a root sub-agent runtime. Runtime clones inherit
/// the lease, so top-level detached children can outlive the parent mailbox
/// without losing their accounting path.
pub(crate) fn acquire_runtime_usage_lease(owner: &str) -> Option<RuntimeUsageLease> {
    let owner = owner.trim();
    if owner.is_empty() {
        return None;
    }
    with_runtime_usage_sinks(|sinks| {
        let entry = sinks.get_mut(owner)?;
        entry.leases = entry.leases.saturating_add(1);
        Some(RuntimeUsageLease {
            owner: owner.to_string(),
            active: true,
        })
    })
}

impl RuntimeUsageLease {
    #[must_use]
    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }
}

impl Clone for RuntimeUsageLease {
    fn clone(&self) -> Self {
        if self.active {
            let cloned = with_runtime_usage_sinks(|sinks| {
                sinks.get_mut(&self.owner).map(|entry| {
                    entry.leases = entry.leases.saturating_add(1);
                })
            });
            if cloned.is_some() {
                return Self {
                    owner: self.owner.clone(),
                    active: true,
                };
            }
        }
        Self {
            owner: self.owner.clone(),
            active: false,
        }
    }
}

impl Drop for RuntimeUsageLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        with_existing_runtime_usage_sinks(|sinks| {
            let should_remove = sinks.get_mut(&self.owner).is_some_and(|entry| {
                entry.leases = entry.leases.saturating_sub(1);
                entry.terminal && entry.leases == 0
            });
            if should_remove {
                sinks.remove(&self.owner);
            }
        });
    }
}

/// Mark the parent turn terminal. An owner with detached children stays live
/// until their cloned leases drop; owners without children retire now.
pub(crate) fn finish_runtime_usage_owner(owner: &str) {
    with_existing_runtime_usage_sinks(|sinks| {
        let should_remove = sinks.get_mut(owner).is_some_and(|entry| {
            entry.terminal = true;
            entry.leases == 0
        });
        if should_remove {
            sinks.remove(owner);
        }
    });
}

/// Take only the background usage assigned to one runtime turn.
/// Other runtime turns and the TUI pending pool remain untouched.
#[must_use]
pub fn take_runtime_usage(owner: &str) -> RuntimeUsageBatch {
    with_runtime_usage_journal_mut(|journal| {
        journal
            .remove(owner)
            .map_or_else(RuntimeUsageBatch::default, |entry| RuntimeUsageBatch {
                records: entry.records.into_iter().collect(),
                drop_records: entry.drop_records.into_iter().collect(),
                dropped_records: entry.dropped_records,
            })
    })
}

/// Capture the current session/run generation before starting a background
/// provider request. The same token must be supplied when its usage returns.
#[must_use]
pub fn scope_token() -> CostScopeToken {
    with_pending_state_mut(|state| CostScopeToken(state.generation))
}

/// Atomically close the current cost scope and start a fresh generation.
/// Reports from old in-flight requests are rejected after this returns, so
/// `/new` and session load cannot inherit another session's spend.
#[must_use]
pub fn close_current_scope() -> PendingBackgroundCost {
    with_pending_state_mut(|state| {
        let pending = std::mem::take(&mut state.pending);
        state.generation = state.generation.wrapping_add(1);
        state.seen_usage_source_fingerprints.clear();
        pending
    })
}

/// Restore the durable response identities belonging to the newly loaded
/// session. Callers close the previous scope before loading, so replacing the
/// set cannot make another session's usage visible here.
pub(crate) fn restore_usage_source_fingerprints(fingerprints: impl IntoIterator<Item = String>) {
    with_pending_state_mut(|state| {
        state.seen_usage_source_fingerprints = fingerprints.into_iter().collect();
    })
}

/// Whether this session generation already accepted a provider response.
/// Used by mailbox delivery to avoid pricing a response that the synchronous
/// runtime sink already owns.
#[must_use]
pub(crate) fn usage_source_seen(source_id: &str) -> bool {
    let fingerprint = usage_source_fingerprint(source_id);
    with_pending_state_mut(|state| state.seen_usage_source_fingerprints.contains(&fingerprint))
}

/// The non-secret identity of a background LLM call's route.
///
/// Background helpers run off a bare client with no app `Config`, so they cannot
/// resolve credential-derived billing. They *can* report what they actually know
/// — which provider, which configured route, which wire model, which endpoint —
/// and this type carries exactly that, so the pricing decision is made from
/// evidence instead of from a provider name.
#[derive(Debug, Clone, Copy)]
#[cfg(test)]
pub struct BackgroundRoute<'a> {
    /// Provider kind serving the call.
    pub provider: ApiProvider,
    /// Configured route identity (the `[providers.<name>]` key), when the
    /// caller has one. This is a user-chosen label, not a credential.
    pub provider_identity: Option<&'a str>,
    /// Wire model id as sent on the request.
    pub wire_model: &'a str,
    /// Concrete base URL the request went to, when the client exposes one.
    ///
    /// Only ever used to derive a billing-surface classification and a
    /// SHA-256 fingerprint; the URL itself never leaves this struct.
    pub base_url: Option<&'a str>,
}

#[cfg(test)]
impl<'a> BackgroundRoute<'a> {
    /// A route with no endpoint information.
    #[must_use]
    pub fn new(provider: ApiProvider, wire_model: &'a str) -> Self {
        Self {
            provider,
            provider_identity: None,
            wire_model,
            base_url: None,
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: Option<&'a str>) -> Self {
        self.base_url = base_url;
        self
    }

    /// Attach the configured route label (`[providers.<name>]` key).
    ///
    /// Background helpers do not currently resolve one; the foreground turn path
    /// supplies it through [`route_receipt`] directly.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_identity(mut self, provider_identity: Option<&'a str>) -> Self {
        self.provider_identity = provider_identity;
        self
    }

    /// Non-secret billing-surface classification for this endpoint.
    #[must_use]
    pub fn billing_surface(&self) -> Option<&'static str> {
        crate::pricing::billing_surface_for_route(self.provider, self.base_url)
    }

    /// SHA-256 fingerprint of the normalized base URL, or `None` when unknown.
    ///
    /// This is the same digest the catalog scopes live rows on, so a live
    /// pricing row can be proven to price *this* endpoint.
    #[must_use]
    pub fn endpoint_fingerprint(&self) -> Option<String> {
        self.base_url.and_then(endpoint_fingerprint)
    }

    /// Billing presentation derivable without app config.
    #[must_use]
    pub fn billing(&self) -> BillingPresentation {
        crate::route_billing::for_endpoint_without_config(self.provider, self.base_url)
    }

    /// A redacted, stable receipt describing this route.
    #[must_use]
    pub fn receipt(&self, currency: &str) -> String {
        route_receipt(
            self.provider,
            self.provider_identity,
            self.wire_model,
            self.billing_surface(),
            self.endpoint_fingerprint().as_deref(),
            self.billing().into(),
            currency,
        )
    }
}

/// Format one redacted route receipt.
///
/// Contains only: provider kind, configured route label, wire model,
/// billing-surface classification, endpoint fingerprint, billing mode, and the currency the
/// estimate is denominated in. It deliberately contains no URL, no credential,
/// and no filesystem path, so it is safe to persist into a saved session and to
/// log. This is the single formatter, so the foreground turn path and the
/// background pool cannot describe the same route two different ways.
#[must_use]
pub fn route_receipt(
    provider: ApiProvider,
    provider_identity: Option<&str>,
    wire_model: &str,
    billing_surface: Option<&str>,
    endpoint_fingerprint: Option<&str>,
    billing_mode: RouteBillingMode,
    currency: &str,
) -> String {
    format!(
        "provider={} identity={} model={} surface={} endpoint_fp={} billing_mode={} currency={currency}",
        provider.as_str(),
        safe_receipt_field(provider_identity.unwrap_or("-")),
        safe_receipt_field(wire_model),
        safe_receipt_field(billing_surface.unwrap_or("unreported")),
        safe_receipt_field(endpoint_fingerprint.unwrap_or("unreported")),
        match billing_mode {
            RouteBillingMode::Metered => "metered",
            RouteBillingMode::Subscription => "subscription",
            RouteBillingMode::Local => "local",
            RouteBillingMode::Unknown => "unknown",
        },
    )
}

const MAX_RECEIPT_FIELD_CHARS: usize = 96;

fn safe_receipt_field(raw: &str) -> String {
    let sanitized = sanitize_persisted_route_label(raw);
    let mut out = String::with_capacity(raw.len().min(MAX_RECEIPT_FIELD_CHARS));
    let mut previous_separator = false;
    for ch in sanitized.chars() {
        if out.chars().count() >= MAX_RECEIPT_FIELD_CHARS {
            break;
        }
        let safe = if ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/' | ':' | '+') {
            ch
        } else {
            '_'
        };
        let separator = safe == '_';
        if separator && previous_separator {
            continue;
        }
        out.push(safe);
        previous_separator = separator;
    }
    if out.is_empty() { "-".to_string() } else { out }
}

pub(crate) fn sanitize_persisted_route_label(raw: &str) -> String {
    const MAX_PERSISTED_ROUTE_LABEL_CHARS: usize = 256;
    let value = raw.trim();
    let lower = value.to_ascii_lowercase();

    if value.is_empty() {
        return "-".to_string();
    }

    // URLs are not route labels. Endpoints have a dedicated, validated hash
    // field; persisting a URL here risks leaking userinfo, query credentials,
    // or fragments through a custom provider/model name.
    if value.contains("://") {
        return "redacted-url".to_string();
    }

    let authorization_value = ["bearer ", "basic ", "digest ", "token ", "apikey "]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        || lower.contains("authorization:")
        || lower.contains("proxy-authorization:");
    if authorization_value {
        return "redacted-credential".to_string();
    }

    // Reject credential assignments regardless of common casing or separator:
    // FOO_API_KEY=..., access-token:..., password = ....
    for (index, ch) in value.char_indices() {
        if !matches!(ch, '=' | ':') {
            continue;
        }
        let name = lower[..index]
            .trim()
            .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '{' | '[' | ','));
        let name = name.rsplit([' ', ',', ';']).next().unwrap_or(name);
        let normalized = name.replace('-', "_");
        if normalized.ends_with("api_key")
            || normalized.ends_with("token")
            || normalized.ends_with("secret")
            || normalized.ends_with("password")
            || normalized.ends_with("passwd")
        {
            return "redacted-credential".to_string();
        }
    }

    // Common credential token prefixes. These are intentionally checked at
    // word boundaries so model ids containing an incidental "sk" survive.
    let credential_prefix = lower
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '=' | ':' | ',' | ';' | '"' | '\''))
        .filter(|part| !part.is_empty())
        .any(|part| {
            [
                "sk-",
                "sk_",
                "rk-",
                "pk-",
                "ghp_",
                "gho_",
                "ghu_",
                "ghs_",
                "github_pat_",
                "hf_",
                "glpat-",
                "xoxb-",
                "xoxp-",
                "xoxa-",
                "akia",
                "aiza",
                "eyj",
            ]
            .iter()
            .any(|prefix| part.starts_with(prefix))
        });
    if credential_prefix {
        return "redacted-credential".to_string();
    }

    let windows_absolute = value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .get(2)
            .is_some_and(|separator| matches!(separator, b'/' | b'\\'));
    let contains_local_root = [
        "/users/",
        "/volumes/",
        "/home/",
        "/private/",
        "\\users\\",
        "file://",
        "/.ssh/",
        "\\.ssh\\",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let looks_like_relative_path = value.contains('\\')
        || lower.starts_with(".ssh/")
        || lower.starts_with(".ssh\\")
        || lower.split('/').any(|segment| {
            matches!(
                segment,
                "." | ".."
                    | ".ssh"
                    | ".config"
                    | "secrets"
                    | "secret"
                    | "credentials"
                    | "credential"
                    | "relative"
                    | "workspace"
                    | "tmp"
            )
        });
    if std::path::Path::new(value).is_absolute()
        || windows_absolute
        || value.starts_with("~/")
        || value.starts_with("./")
        || value.starts_with("../")
        || contains_local_root
        || looks_like_relative_path
    {
        return "redacted-local-path".to_string();
    }
    let bounded: String = value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_PERSISTED_ROUTE_LABEL_CHARS)
        .collect();
    if bounded.is_empty() {
        "-".to_string()
    } else {
        bounded
    }
}

/// Validate and canonicalize an endpoint before producing the cryptographic
/// fingerprint persisted in a receipt. Secret-bearing/malformed URLs receive
/// no fingerprint at all; userinfo, query strings, and fragments are never fed
/// to the hash function.
#[must_use]
pub fn endpoint_fingerprint(base_url: &str) -> Option<String> {
    let mut parsed = reqwest::Url::parse(base_url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.host_str().is_none()
    {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    let canonical = parsed.as_str().trim_end_matches('/');
    Some(codewhale_config::catalog::base_url_fingerprint(canonical))
}

/// Currency tag for a receipt, derived from authoritative currency coverage —
/// not from a positive amount, because a zero-usage priced turn is still a
/// valid zero in its published currency.
#[must_use]
pub fn currency_tag(audit: &TurnCostAudit) -> &'static str {
    match (audit.usd_priced, audit.cny_priced) {
        (true, true) => "usd+cny",
        (true, false) => "usd",
        (false, true) => "cny",
        (false, false) => "unpriced",
    }
}

/// Background callers report their LLM usage here.
///
/// The route is priced through the same [`crate::pricing::audit_turn_cost_for_route_on_endpoint`]
/// the foreground turn path uses, so a background turn cannot be counted under
/// different rules than a parent turn. Adds no money when the route is exactly
/// non-metered (a local runtime, an OAuth broker, a named plan endpoint), and
/// counts the turn as *missing spend* whenever it is money-metered or of unknown
/// basis but could not be priced — an unknown basis is never waved through as a
/// subscription (#4318).
#[cfg(test)]
pub fn report(scope: CostScopeToken, route: &BackgroundRoute<'_>, usage: &Usage) {
    let billing_surface = route.billing_surface();
    let fingerprint = route.endpoint_fingerprint();
    let audit = crate::pricing::audit_turn_cost_for_route_on_endpoint(
        route.provider,
        route.wire_model,
        billing_surface,
        fingerprint.as_deref(),
        usage,
        chrono::Utc::now(),
        route.billing(),
    );
    record(scope, route.receipt(currency_tag(&audit)), &audit, usage);
}

/// Report usage against an immutable route envelope captured before the call.
/// This is the background equivalent of foreground/subagent accrual and avoids
/// response aliases or completion-time clocks changing billing identity.
pub fn report_effective_route(
    scope: CostScopeToken,
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
) {
    let audit = route.audit(usage);
    record(scope, route.receipt(&audit), &audit, usage);
}

/// Report background usage to exactly one accounting owner.
///
/// Runtime-owned calls go only to the durable runtime sink. Calls without a
/// runtime owner belong to the interactive TUI pool. Mixing both paths would
/// count one provider response twice in hosts that expose both projections.
pub fn report_effective_route_for_runtime(
    scope: CostScopeToken,
    runtime_owner: Option<&str>,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
) {
    if let Some(owner) = runtime_owner {
        record_runtime_usage(owner, source_id, route, usage);
    } else {
        record_interactive_runtime_usage(
            scope,
            RuntimeUsageRecord {
                source_id: source_id.to_string(),
                usage: EffectiveRouteUsage {
                    route: route.sanitized_for_persistence(),
                    usage: usage.clone(),
                },
            },
        );
    }
}

/// Report an interactive auxiliary response against its immutable origin.
/// A stale foreground scope is not an error: it means `/new` or session load
/// already moved on, so the exact receipt is appended to the old session's
/// durable sidecar instead of being redirected to the active session.
pub(crate) fn report_effective_route_for_interactive_origin(
    scope: CostScopeToken,
    session_id: &str,
    turn_id: &str,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
) {
    let record = RuntimeUsageRecord {
        source_id: source_id.to_string(),
        usage: EffectiveRouteUsage {
            route: route.sanitized_for_persistence(),
            usage: usage.clone(),
        },
    };
    if record_interactive_runtime_usage(scope, record.clone()) {
        return;
    }
    let persisted =
        crate::session_manager::SessionManager::default_location().is_ok_and(|manager| {
            manager
                .persist_late_runtime_usage(session_id, turn_id, &record)
                .unwrap_or(false)
        });
    if !persisted {
        tracing::warn!("late interactive usage could not be persisted for its origin session");
    }
}

#[cfg(test)]
fn report_effective_route_for_interactive_origin_with_manager(
    scope: CostScopeToken,
    session_id: &str,
    turn_id: &str,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
    usage: &Usage,
    manager: &crate::session_manager::SessionManager,
) -> bool {
    let record = RuntimeUsageRecord {
        source_id: source_id.to_string(),
        usage: EffectiveRouteUsage {
            route: route.sanitized_for_persistence(),
            usage: usage.clone(),
        },
    };
    if record_interactive_runtime_usage(scope, record.clone()) {
        return true;
    }
    manager
        .persist_late_runtime_usage(session_id, turn_id, &record)
        .unwrap_or(false)
}

pub(crate) fn report_unreceipted_for_interactive_origin(
    scope: CostScopeToken,
    session_id: &str,
    turn_id: &str,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
) {
    let record = RuntimeUsageDropRecord {
        source_id: source_id.to_string(),
        route: route.sanitized_for_persistence(),
    };
    if record_interactive_runtime_usage_drop(scope, record.clone()) {
        return;
    }
    let persisted =
        crate::session_manager::SessionManager::default_location().is_ok_and(|manager| {
            manager
                .persist_late_runtime_drop(session_id, turn_id, &record)
                .unwrap_or(false)
        });
    if !persisted {
        tracing::warn!(
            "late interactive missing-usage receipt could not be persisted for its origin session"
        );
    }
}

#[cfg(test)]
fn report_unreceipted_for_interactive_origin_with_manager(
    scope: CostScopeToken,
    session_id: &str,
    turn_id: &str,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
    manager: &crate::session_manager::SessionManager,
) -> bool {
    let record = RuntimeUsageDropRecord {
        source_id: source_id.to_string(),
        route: route.sanitized_for_persistence(),
    };
    if record_interactive_runtime_usage_drop(scope, record.clone()) {
        return true;
    }
    manager
        .persist_late_runtime_drop(session_id, turn_id, &record)
        .unwrap_or(false)
}

/// Record one provider-success response whose usage payload was absent.
///
/// Callers must supply the same fixed-length, non-secret source identity they
/// would use for a normal routed usage receipt. Runtime owners persist one
/// bounded dropped-coverage marker; ownerless/interactive calls add one
/// unpriced coverage turn to the captured session scope. Replays are
/// idempotent, and a stale scope cannot contaminate a later session.
pub(crate) fn report_unreceipted_provider_success(
    scope: CostScopeToken,
    runtime_owner: Option<&str>,
    source_id: &str,
    route: &EffectiveRouteEnvelope,
) {
    if let Some(owner) = runtime_owner {
        record_runtime_usage_drop(owner, source_id, route);
    } else {
        record_interactive_runtime_usage_drop(
            scope,
            RuntimeUsageDropRecord {
                source_id: source_id.to_string(),
                route: route.sanitized_for_persistence(),
            },
        );
    }
}

/// Settle one bounded routed-usage batch without repricing or losing exact
/// missing-usage route evidence. Replaying the same batch is idempotent by the
/// stable per-response source ids. Any residual count whose exact record was
/// truncated remains an explicit fail-closed coverage gap.
pub(crate) fn report_runtime_usage_batch(
    scope: CostScopeToken,
    runtime_owner: Option<&str>,
    batch: &RuntimeUsageBatch,
) {
    for record in &batch.records {
        report_effective_route_for_runtime(
            scope,
            runtime_owner,
            &record.source_id,
            &record.usage.route,
            &record.usage.usage,
        );
    }
    for record in &batch.drop_records {
        report_unreceipted_provider_success(scope, runtime_owner, &record.source_id, &record.route);
    }

    let residual = batch
        .dropped_records
        .saturating_sub(u64::try_from(batch.drop_records.len()).unwrap_or(u64::MAX));
    if residual == 0 {
        return;
    }
    let mut identities = batch
        .records
        .iter()
        .map(|record| usage_source_fingerprint(&record.source_id))
        .chain(
            batch
                .drop_records
                .iter()
                .map(|record| usage_source_fingerprint(&record.source_id)),
        )
        .take(MAX_RUNTIME_USAGE_RECORDS_PER_OWNER)
        .collect::<Vec<_>>();
    identities.sort_unstable();
    let residual_source = format!(
        "runtime-usage-batch-residual:{}",
        usage_source_fingerprint(&format!(
            "{}:{}:{}:{}",
            batch.records.len(),
            batch.drop_records.len(),
            batch.dropped_records,
            identities.join(":")
        ))
    );
    if let Some(owner) = runtime_owner {
        record_runtime_usage_drop_count(owner, &residual_source, residual);
    } else {
        record_interactive_runtime_usage_drop_count(scope, &residual_source, residual);
    }
}

#[must_use]
pub(crate) fn background_cost_for_runtime_usage(
    record: &RuntimeUsageRecord,
) -> PendingBackgroundCost {
    let mut pending = PendingBackgroundCost::default();
    let fingerprint = usage_source_fingerprint(&record.source_id);
    let audit = record.usage.route.audit(&record.usage.usage);
    let receipt = record.usage.route.receipt(&audit);
    pending.usage_source_fingerprints.insert(fingerprint);
    fold_audit_into_pending(&mut pending, receipt, &audit, &record.usage.usage);
    pending
}

#[must_use]
pub(crate) fn background_cost_for_runtime_drop(
    record: &RuntimeUsageDropRecord,
) -> PendingBackgroundCost {
    let mut pending = PendingBackgroundCost::default();
    pending
        .usage_source_fingerprints
        .insert(usage_source_fingerprint(&record.source_id));
    if !matches!(
        record.route.billing_mode,
        RouteBillingMode::Subscription | RouteBillingMode::Local
    ) {
        pending.unpriced_turns = 1;
        pending.cny_unpriced_turns = 1;
        pending
            .unpriced_reasons
            .insert("provider_success_missing_usage");
        pending
            .cny_unpriced_reasons
            .insert("provider_success_missing_usage");
    }
    pending
}

/// Fold one already-computed audit into the pending pool.
fn record(scope: CostScopeToken, route_receipt: String, audit: &TurnCostAudit, usage: &Usage) {
    with_pending_state_mut(|state| {
        if state.generation != scope.0 {
            return;
        }
        fold_audit_into_pending(&mut state.pending, route_receipt, audit, usage);
    });
}

fn record_interactive_runtime_usage(scope: CostScopeToken, record: RuntimeUsageRecord) -> bool {
    with_pending_state_mut(|state| {
        if state.generation != scope.0 {
            return false;
        }
        let fingerprint = usage_source_fingerprint(&record.source_id);
        if !state
            .seen_usage_source_fingerprints
            .insert(fingerprint.clone())
        {
            return true;
        }
        let audit = record.usage.route.audit(&record.usage.usage);
        let receipt = record.usage.route.receipt(&audit);
        state.pending.usage_source_fingerprints.insert(fingerprint);
        fold_audit_into_pending(&mut state.pending, receipt, &audit, &record.usage.usage);
        true
    })
}

fn record_interactive_runtime_usage_drop(
    scope: CostScopeToken,
    record: RuntimeUsageDropRecord,
) -> bool {
    with_pending_state_mut(|state| {
        if state.generation != scope.0 {
            return false;
        }
        let fingerprint = usage_source_fingerprint(&record.source_id);
        if !state
            .seen_usage_source_fingerprints
            .insert(fingerprint.clone())
        {
            return true;
        }
        state.pending.usage_source_fingerprints.insert(fingerprint);
        if matches!(
            record.route.billing_mode,
            RouteBillingMode::Subscription | RouteBillingMode::Local
        ) {
            return true;
        }
        state.pending.unpriced_turns = state.pending.unpriced_turns.saturating_add(1);
        state.pending.cny_unpriced_turns = state.pending.cny_unpriced_turns.saturating_add(1);
        state
            .pending
            .unpriced_reasons
            .insert("provider_success_missing_usage");
        state
            .pending
            .cny_unpriced_reasons
            .insert("provider_success_missing_usage");
        true
    })
}

fn record_interactive_runtime_usage_drop_count(
    scope: CostScopeToken,
    source_id: &str,
    count: u64,
) -> bool {
    with_pending_state_mut(|state| {
        if state.generation != scope.0 {
            return false;
        }
        let fingerprint = usage_source_fingerprint(source_id);
        if !state
            .seen_usage_source_fingerprints
            .insert(fingerprint.clone())
        {
            return true;
        }
        state.pending.usage_source_fingerprints.insert(fingerprint);
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        state.pending.unpriced_turns = state.pending.unpriced_turns.saturating_add(count);
        state.pending.cny_unpriced_turns = state.pending.cny_unpriced_turns.saturating_add(count);
        state
            .pending
            .unpriced_reasons
            .insert("routed_usage_receipt_missing");
        state
            .pending
            .cny_unpriced_reasons
            .insert("routed_usage_receipt_missing");
        true
    })
}

fn fold_audit_into_pending(
    pending: &mut PendingBackgroundCost,
    route_receipt: String,
    audit: &TurnCostAudit,
    usage: &Usage,
) {
    if let Some(provenance) = audit.provenance.as_ref() {
        pending.pricing_provenances.insert(provenance.label());
    }
    if let Some(defect) = audit.live_pricing_defect.as_ref() {
        if audit.estimate.is_some() {
            pending.live_pricing_defects.insert(defect.label());
        } else {
            pending.live_pricing_unusable_defects.insert(defect.label());
        }
    }
    if let Some(cost) = audit.estimate {
        pending.estimate = pending.estimate.saturating_add(cost);
    }

    // Only money-metered/unknown-basis turns belong in missing-money coverage
    // or its reason list. A subscription/local receipt is still audited below,
    // but `not_money_metered` must never be presented as a gap in a subtotal.
    if audit.counts_toward_money_coverage() {
        if audit.usd_priced {
            pending.priced_turns = pending.priced_turns.saturating_add(1);
        } else {
            pending.unpriced_turns = pending.unpriced_turns.saturating_add(1);
        }
        if audit.cny_priced {
            pending.cny_priced_turns = pending.cny_priced_turns.saturating_add(1);
        } else {
            pending.cny_unpriced_turns = pending.cny_unpriced_turns.saturating_add(1);
        }
        for class in &audit.unpriced_classes {
            pending.unpriced_classes.insert(class.label());
        }
        if !audit.usd_priced
            && let Some(reason) = audit.unpriced_reason
        {
            pending.unpriced_reasons.insert(reason.label());
        }
        if !audit.cny_priced {
            pending.cny_unpriced_reasons.insert(
                audit
                    .unpriced_reason
                    .map_or("currency_not_published", |reason| reason.label()),
            );
        }
    }

    // Record which token classes this route actually billed on, so a receipt
    // shows whether cache-write/reasoning telemetry was even present.
    pending
        .route_receipts
        .insert(receipt_with_usage_classes(route_receipt, usage));
}

/// Drain the pending pool, returning it and resetting to zero.
///
/// Money and its completeness leave together, so a caller can never fold a
/// subtotal into a session total without the counters that qualify it.
#[must_use]
pub fn drain() -> PendingBackgroundCost {
    with_pending_state_mut(|state| std::mem::take(&mut state.pending))
}

/// Reset the pool to zero without consuming. Test-only helper for
/// suites that share the static and need to start from a known
/// state. Production code should always use [`drain`].
#[cfg(test)]
pub fn reset_for_tests() {
    with_pending_state_mut(|state| {
        state.pending = PendingBackgroundCost::default();
        state.seen_usage_source_fingerprints.clear();
    });
    with_runtime_usage_journal_mut(HashMap::clear);
}

#[cfg(test)]
pub(crate) struct TestCostScope;

#[cfg(test)]
impl Drop for TestCostScope {
    fn drop(&mut self) {
        reset_for_tests();
    }
}

#[cfg(test)]
pub(crate) fn test_scope() -> TestCostScope {
    reset_for_tests();
    TestCostScope
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ProviderCatalogTestReset;

    impl Drop for ProviderCatalogTestReset {
        fn drop(&mut self) {
            crate::provider_catalog_live::reset_cache_for_test();
            crate::provider_lake::clear_live_snapshot();
        }
    }

    fn priced_provider_delta(
        provider: &str,
        model: &str,
        fingerprint: &str,
        fetched_at: u64,
    ) -> codewhale_config::catalog::ProviderCatalogDelta {
        priced_provider_delta_with_rates(provider, model, fingerprint, fetched_at, 1.25, 5.0)
    }

    fn priced_provider_delta_with_rates(
        provider: &str,
        model: &str,
        fingerprint: &str,
        fetched_at: u64,
        input: f64,
        output: f64,
    ) -> codewhale_config::catalog::ProviderCatalogDelta {
        codewhale_config::catalog::ProviderCatalogDelta {
            provider: provider.to_string(),
            base_url_fingerprint: fingerprint.to_string(),
            fetched_at,
            offerings: vec![codewhale_config::catalog::CatalogOffering {
                provider: provider.to_string(),
                wire_model_id: model.to_string(),
                endpoint_key: "chat".to_string(),
                cost: Some(codewhale_config::models_dev::ModelsDevCost {
                    input: Some(input),
                    output: Some(output),
                    cache_read: Some(0.25),
                    cache_write: None,
                }),
                ..Default::default()
            }],
        }
    }

    fn custom_usage_envelope(
        identity: &str,
        model: &str,
        fingerprint: &str,
        billing_mode: RouteBillingMode,
        dispatched_at: DateTime<Utc>,
    ) -> EffectiveRouteEnvelope {
        provider_live_usage_envelope(
            ApiProvider::Custom,
            identity,
            model,
            fingerprint,
            Some(crate::pricing::UNCLASSIFIED_BILLING_SURFACE),
            billing_mode,
            dispatched_at,
        )
    }

    fn provider_live_usage_envelope(
        provider: ApiProvider,
        identity: &str,
        model: &str,
        fingerprint: &str,
        billing_surface: Option<&str>,
        billing_mode: RouteBillingMode,
        dispatched_at: DateTime<Utc>,
    ) -> EffectiveRouteEnvelope {
        let provider_live_pricing =
            u64::try_from(dispatched_at.timestamp())
                .ok()
                .and_then(|dispatched_at_unix| {
                    crate::provider_catalog_live::fresh_provider_live_pricing_quote_at(
                        provider,
                        identity,
                        model,
                        fingerprint,
                        dispatched_at_unix,
                    )
                });
        EffectiveRouteEnvelope {
            provider,
            provider_identity: identity.to_string(),
            model: model.to_string(),
            billing_surface: billing_surface.map(str::to_string),
            endpoint_fingerprint: Some(fingerprint.to_string()),
            provider_live_pricing,
            billing_mode,
            dispatched_at,
        }
    }

    fn small_usage() -> Usage {
        Usage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Default::default()
        }
    }

    #[test]
    fn baseten_usage_prices_only_the_reviewed_identity_on_the_official_endpoint() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let now = Utc::now();
        let fetched_at = u64::try_from(now.timestamp()).expect("nonnegative timestamp");
        let model = "synthetic-baseten-priced-model";
        let fingerprint =
            codewhale_config::catalog::base_url_fingerprint(codewhale_config::BASETEN_BASE_URL);
        crate::provider_catalog_live::record_success(priced_provider_delta(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            fetched_at,
        ));
        let usage = Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        };

        let exact = custom_usage_envelope(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            RouteBillingMode::Unknown,
            now,
        )
        .audit(&usage);
        assert!(exact.is_priced(), "{exact:?}");
        assert_eq!(
            exact.provenance,
            Some(codewhale_config::pricing::PricingProvenance::ProviderLive)
        );
        assert_eq!(exact.estimate.expect("priced").usd, 1.25);

        // A reviewed schema alias remains a distinct custom ownership scope.
        // It becomes billable only after that exact identity refreshed its own
        // catalog; it cannot borrow the canonical `baseten` partition above.
        let alias = "base-ten";
        crate::provider_catalog_live::record_success(priced_provider_delta(
            alias,
            model,
            &fingerprint,
            fetched_at,
        ));
        let alias_audit =
            custom_usage_envelope(alias, model, &fingerprint, RouteBillingMode::Unknown, now)
                .audit(&usage);
        assert!(alias_audit.is_priced(), "{alias_audit:?}");
        assert_eq!(
            alias_audit.provenance,
            Some(codewhale_config::pricing::PricingProvenance::ProviderLive)
        );
        assert_eq!(alias_audit.estimate.expect("priced").usd, 1.25);

        let generic = custom_usage_envelope(
            "custom-lab",
            model,
            &fingerprint,
            RouteBillingMode::Metered,
            now,
        )
        .audit(&usage);
        assert!(!generic.is_priced(), "{generic:?}");
        assert_eq!(
            generic.unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnknownBillingBasis)
        );

        let wrong_fingerprint =
            codewhale_config::catalog::base_url_fingerprint("https://proxy.example/v1");
        let wrong_endpoint = custom_usage_envelope(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &wrong_fingerprint,
            RouteBillingMode::Metered,
            now,
        )
        .audit(&usage);
        assert!(!wrong_endpoint.is_priced(), "{wrong_endpoint:?}");
        assert_eq!(
            wrong_endpoint.unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnknownBillingBasis)
        );
    }

    #[test]
    fn baseten_usage_rejects_unknown_stale_and_failed_live_catalogs() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let now = Utc::now();
        let now_unix = u64::try_from(now.timestamp()).expect("nonnegative timestamp");
        let model = "synthetic-baseten-status-model";
        let fingerprint =
            codewhale_config::catalog::base_url_fingerprint(codewhale_config::BASETEN_BASE_URL);
        let unknown_route = custom_usage_envelope(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            RouteBillingMode::Unknown,
            now,
        );
        assert!(unknown_route.provider_live_pricing.is_none());
        let usage = Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        };

        // A same-model price owned by another custom partition cannot price a
        // Baseten receipt whose exact catalog was never refreshed.
        crate::provider_catalog_live::record_success(priced_provider_delta(
            "other-custom",
            model,
            &fingerprint,
            now_unix,
        ));
        let unknown = unknown_route.audit(&usage);
        assert!(!unknown.is_priced(), "{unknown:?}");
        assert_eq!(
            unknown.unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
        );

        let stale_at = now_unix
            .saturating_sub(crate::provider_catalog_live::DEFAULT_PROVIDER_CATALOG_TTL_SECS)
            .saturating_sub(1);
        crate::provider_catalog_live::record_success(priced_provider_delta(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            stale_at,
        ));
        let stale_route = custom_usage_envelope(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            RouteBillingMode::Unknown,
            now,
        );
        assert!(stale_route.provider_live_pricing.is_none());
        let stale = stale_route.audit(&usage);
        assert!(!stale.is_priced(), "{stale:?}");
        assert_eq!(
            stale.unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
        );

        crate::provider_catalog_live::record_success(priced_provider_delta(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            now_unix,
        ));
        crate::provider_catalog_live::record_failure(
            codewhale_config::BASETEN_TEMPLATE_ID,
            &fingerprint,
            codewhale_config::catalog::CatalogRefreshError::Network,
        );
        let failed_route = custom_usage_envelope(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            RouteBillingMode::Unknown,
            now,
        );
        assert!(failed_route.provider_live_pricing.is_none());
        let failed = failed_route.audit(&usage);
        assert!(!failed.is_priced(), "{failed:?}");
        assert_eq!(
            failed.unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
        );
    }

    #[test]
    fn reviewed_provider_live_quotes_survive_same_second_refresh_and_key_state_changes() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let now = Utc::now();
        let fetched_at = u64::try_from(now.timestamp()).expect("nonnegative timestamp");
        let cases = [
            (
                ApiProvider::Openrouter,
                ApiProvider::Openrouter.as_str(),
                "synthetic-openrouter-frozen-price",
                codewhale_config::catalog::base_url_fingerprint(
                    crate::config::DEFAULT_OPENROUTER_BASE_URL,
                ),
                crate::pricing::AGGREGATOR_BILLING_SURFACE,
                RouteBillingMode::Metered,
            ),
            (
                ApiProvider::Custom,
                codewhale_config::BASETEN_TEMPLATE_ID,
                "synthetic-baseten-frozen-price",
                codewhale_config::catalog::base_url_fingerprint(codewhale_config::BASETEN_BASE_URL),
                crate::pricing::UNCLASSIFIED_BILLING_SURFACE,
                RouteBillingMode::Unknown,
            ),
        ];
        let usage = Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        };

        for (provider, identity, model, fingerprint, surface, mode) in cases {
            crate::provider_catalog_live::record_success(priced_provider_delta_with_rates(
                identity,
                model,
                &fingerprint,
                fetched_at,
                1.25,
                5.0,
            ));
            let first = provider_live_usage_envelope(
                provider,
                identity,
                model,
                &fingerprint,
                Some(surface),
                mode,
                now,
            );
            let first_quote = first
                .provider_live_pricing
                .as_ref()
                .expect("fresh exact scope freezes a quote");

            // A second refresh in the same Unix second must still be a distinct
            // catalog revision and must not retroactively change `first`.
            crate::provider_catalog_live::record_success(priced_provider_delta_with_rates(
                identity,
                model,
                &fingerprint,
                fetched_at,
                9.5,
                19.0,
            ));
            let second = provider_live_usage_envelope(
                provider,
                identity,
                model,
                &fingerprint,
                Some(surface),
                mode,
                now,
            );
            let second_quote = second
                .provider_live_pricing
                .as_ref()
                .expect("replacement fresh scope freezes a quote");
            assert_ne!(
                first_quote.catalog_revision, second_quote.catalog_revision,
                "same-second price changes need distinct revisions"
            );

            crate::provider_catalog_live::record_failure(
                identity,
                &fingerprint,
                codewhale_config::catalog::CatalogRefreshError::Unauthorized,
            );
            if provider == ApiProvider::Custom {
                // Baseten's same URL can represent another account after a key
                // switch. Starting that refresh clears the mutable old scope.
                let _new_key_refresh = crate::provider_catalog_live::begin_refresh(identity);
            }

            let first_audit = first.audit(&usage);
            let second_audit = second.audit(&usage);
            assert_eq!(first_audit.estimate.expect("first quote priced").usd, 1.25);
            assert_eq!(second_audit.estimate.expect("second quote priced").usd, 9.5);

            let after_mutation = provider_live_usage_envelope(
                provider,
                identity,
                model,
                &fingerprint,
                Some(surface),
                mode,
                now,
            );
            assert!(
                after_mutation.provider_live_pricing.is_none(),
                "failed or cleared mutable state cannot mint a new quote"
            );
        }
    }

    #[test]
    fn legacy_no_quote_receipts_cannot_be_retro_priced_by_a_later_refresh() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let now = Utc::now();
        let fetched_at = u64::try_from(now.timestamp()).expect("nonnegative timestamp");
        let routes = [
            provider_live_usage_envelope(
                ApiProvider::Openrouter,
                ApiProvider::Openrouter.as_str(),
                "synthetic-openrouter-legacy",
                &codewhale_config::catalog::base_url_fingerprint(
                    crate::config::DEFAULT_OPENROUTER_BASE_URL,
                ),
                Some(crate::pricing::AGGREGATOR_BILLING_SURFACE),
                RouteBillingMode::Metered,
                now,
            ),
            custom_usage_envelope(
                codewhale_config::BASETEN_TEMPLATE_ID,
                "synthetic-baseten-legacy",
                &codewhale_config::catalog::base_url_fingerprint(
                    codewhale_config::BASETEN_BASE_URL,
                ),
                RouteBillingMode::Unknown,
                now,
            ),
        ];
        assert!(
            routes
                .iter()
                .all(|route| route.provider_live_pricing.is_none())
        );

        for route in &routes {
            crate::provider_catalog_live::record_success(priced_provider_delta(
                &route.provider_identity,
                &route.model,
                route.endpoint_fingerprint.as_deref().expect("fingerprint"),
                fetched_at,
            ));
            let audit = route.audit(&Usage {
                input_tokens: 1_000_000,
                ..Usage::default()
            });
            assert_eq!(
                audit.unpriced_reason,
                Some(if route.provider == ApiProvider::Openrouter {
                    crate::pricing::UnpricedReason::NoPricingRow
                } else {
                    crate::pricing::UnpricedReason::UnverifiedLivePricing
                }),
                "a completion-time refresh must not price {route:?}"
            );
        }
    }

    #[test]
    fn openrouter_offline_bundled_price_is_immutable_after_dispatch() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let dispatched_at = Utc::now();
        let fetched_at = u64::try_from(dispatched_at.timestamp()).expect("timestamp");
        let model = "qwen/qwen3.8-flash";
        let fingerprint = codewhale_config::catalog::base_url_fingerprint(
            crate::config::DEFAULT_OPENROUTER_BASE_URL,
        );
        let route = provider_live_usage_envelope(
            ApiProvider::Openrouter,
            ApiProvider::Openrouter.as_str(),
            model,
            &fingerprint,
            Some(crate::pricing::AGGREGATOR_BILLING_SURFACE),
            RouteBillingMode::Metered,
            dispatched_at,
        );
        assert!(route.provider_live_pricing.is_none());

        let usage = Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        };
        let offline = route.audit(&usage);
        assert_eq!(
            offline.estimate.expect("bundled OpenRouter price").usd,
            0.16
        );
        assert_eq!(
            offline.provenance,
            Some(codewhale_config::pricing::PricingProvenance::ModelsDevBundled)
        );

        // A later mutable refresh cannot change a turn that had no quote at
        // the application-dispatch boundary.
        crate::provider_catalog_live::record_success(priced_provider_delta_with_rates(
            ApiProvider::Openrouter.as_str(),
            model,
            &fingerprint,
            fetched_at,
            19.0,
            29.0,
        ));
        let after_refresh = route.audit(&usage);
        assert_eq!(after_refresh, offline);

        // Admission without provider usage does not create a charge.
        let no_usage = route.audit(&Usage::default());
        let no_usage_estimate = no_usage.estimate.expect("known zero usage is priced");
        assert_eq!(no_usage_estimate.usd, 0.0);
        assert_eq!(no_usage_estimate.cny, 0.0);
    }

    #[test]
    fn provider_live_quotes_reject_future_prices_and_every_route_binding_mismatch() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let dispatched_at = Utc::now();
        let dispatch_unix = u64::try_from(dispatched_at.timestamp()).expect("timestamp");
        let future_at = dispatched_at + chrono::Duration::seconds(1);
        let future_unix = dispatch_unix.saturating_add(1);
        let cases = [
            (
                ApiProvider::Openrouter,
                ApiProvider::Openrouter.as_str(),
                "synthetic-openrouter-future",
                codewhale_config::catalog::base_url_fingerprint(
                    crate::config::DEFAULT_OPENROUTER_BASE_URL,
                ),
                crate::pricing::AGGREGATOR_BILLING_SURFACE,
                RouteBillingMode::Metered,
            ),
            (
                ApiProvider::Custom,
                codewhale_config::BASETEN_TEMPLATE_ID,
                "synthetic-baseten-future",
                codewhale_config::catalog::base_url_fingerprint(codewhale_config::BASETEN_BASE_URL),
                crate::pricing::UNCLASSIFIED_BILLING_SURFACE,
                RouteBillingMode::Unknown,
            ),
        ];
        let usage = Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        };

        for (provider, identity, model, fingerprint, surface, mode) in cases {
            crate::provider_catalog_live::record_success(priced_provider_delta(
                identity,
                model,
                &fingerprint,
                future_unix,
            ));
            let no_future_quote = provider_live_usage_envelope(
                provider,
                identity,
                model,
                &fingerprint,
                Some(surface),
                mode,
                dispatched_at,
            );
            assert!(no_future_quote.provider_live_pricing.is_none());
            assert_eq!(
                no_future_quote.audit(&usage).unpriced_reason,
                Some(if provider == ApiProvider::Openrouter {
                    crate::pricing::UnpricedReason::NoPricingRow
                } else {
                    crate::pricing::UnpricedReason::UnverifiedLivePricing
                })
            );

            let captured = provider_live_usage_envelope(
                provider,
                identity,
                model,
                &fingerprint,
                Some(surface),
                mode,
                future_at,
            );
            assert!(captured.provider_live_pricing.is_some());

            let mut future_relative_to_dispatch = captured.clone();
            future_relative_to_dispatch.dispatched_at = dispatched_at;
            assert_eq!(
                future_relative_to_dispatch.audit(&usage).unpriced_reason,
                Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
            );
            let persisted = serde_json::to_value(&future_relative_to_dispatch)
                .expect("invalid future quote serializes only as absent");
            assert!(persisted["provider_live_pricing"].is_null());

            let mut wrong_model = captured.clone();
            wrong_model.model.push_str("-other");
            assert_eq!(
                wrong_model.audit(&usage).unpriced_reason,
                Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
            );

            let mut wrong_identity = captured.clone();
            wrong_identity.provider_identity.push_str("-other");
            assert_eq!(
                wrong_identity.audit(&usage).unpriced_reason,
                Some(if provider == ApiProvider::Custom {
                    crate::pricing::UnpricedReason::UnknownBillingBasis
                } else {
                    crate::pricing::UnpricedReason::UnverifiedLivePricing
                })
            );

            let mut wrong_endpoint = captured;
            wrong_endpoint.endpoint_fingerprint = Some(
                codewhale_config::catalog::base_url_fingerprint("https://proxy.example/v1"),
            );
            assert_eq!(
                wrong_endpoint.audit(&usage).unpriced_reason,
                Some(if provider == ApiProvider::Custom {
                    crate::pricing::UnpricedReason::UnknownBillingBasis
                } else {
                    crate::pricing::UnpricedReason::UnverifiedLivePricing
                })
            );
        }
    }

    #[test]
    fn provider_live_quote_serialization_is_secret_free_and_legacy_compatible() {
        let _env = crate::test_support::lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("test home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let _reset = ProviderCatalogTestReset;
        crate::provider_catalog_live::reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let now = Utc::now();
        let fetched_at = u64::try_from(now.timestamp()).expect("nonnegative timestamp");
        let model = "synthetic-baseten-serialized-quote";
        let fingerprint =
            codewhale_config::catalog::base_url_fingerprint(codewhale_config::BASETEN_BASE_URL);
        crate::provider_catalog_live::record_success(priced_provider_delta(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            fetched_at,
        ));
        let route = custom_usage_envelope(
            codewhale_config::BASETEN_TEMPLATE_ID,
            model,
            &fingerprint,
            RouteBillingMode::Unknown,
            now,
        );
        assert!(route.provider_live_pricing.is_some());

        let serialized = serde_json::to_string(&route).expect("serialize frozen route");
        assert!(serialized.contains("provider_live_pricing"));
        assert!(serialized.contains("catalog_revision"));
        assert!(serialized.contains("input_per_million"));
        for secret in [codewhale_config::BASETEN_BASE_URL, "api_key", "Bearer "] {
            // The assertion message must not itself become a logging sink for
            // the credential fragment it checks for — name the check, not the
            // secret.
            assert!(
                !serialized.contains(secret),
                "frozen route serialization leaked a credential fragment"
            );
        }

        let mut child = serde_json::json!({});
        attach_child_usage_metadata(&mut child, &route, &Usage::default());
        let child_route = child_route_envelope_from_metadata(&child).expect("child route");
        assert_eq!(child_route, route.sanitized_for_persistence());

        let mut legacy: serde_json::Value =
            serde_json::from_str(&serialized).expect("route JSON value");
        legacy
            .as_object_mut()
            .expect("route object")
            .remove("provider_live_pricing");
        let legacy: EffectiveRouteEnvelope =
            serde_json::from_value(legacy).expect("legacy route remains readable");
        assert!(legacy.provider_live_pricing.is_none());
        let audit = legacy.audit(&Usage {
            input_tokens: 1_000_000,
            ..Usage::default()
        });
        assert_eq!(
            audit.unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
        );

        let mut wrong_model = route.clone();
        wrong_model.model.push_str("-other");
        assert_eq!(
            wrong_model.audit(&Usage::default()).unpriced_reason,
            Some(crate::pricing::UnpricedReason::UnverifiedLivePricing)
        );
    }

    #[test]
    fn routed_child_batch_is_preferred_bounded_and_sanitized() {
        let route = deepseek_envelope();
        let records = vec![
            RuntimeUsageRecord {
                source_id: "raw-provider-response-id-one".to_string(),
                usage: EffectiveRouteUsage {
                    route: route.clone(),
                    usage: Usage {
                        input_tokens: 11,
                        ..Usage::default()
                    },
                },
            },
            RuntimeUsageRecord {
                source_id: "raw-provider-response-id-two".to_string(),
                usage: EffectiveRouteUsage {
                    route: route.clone(),
                    usage: Usage {
                        output_tokens: 7,
                        ..Usage::default()
                    },
                },
            },
        ];
        let mut metadata = serde_json::json!({});
        attach_child_usage_metadata(&mut metadata, &route, &Usage::default());
        attach_child_usage_batch_metadata(
            &mut metadata,
            &RuntimeUsageBatch {
                records,
                drop_records: Vec::new(),
                dropped_records: 0,
            },
        );

        let serialized = serde_json::to_string(&metadata).expect("batch metadata");
        assert!(!serialized.contains("raw-provider-response-id"));
        let batch = child_usage_records_from_metadata(&metadata).expect("preferred batch");
        assert_eq!(batch.records.len(), 2);
        assert_eq!(batch.records[0].usage.usage.input_tokens, 11);
        assert_eq!(batch.records[1].usage.usage.output_tokens, 7);
        assert_eq!(batch.dropped_records, 0);

        metadata[CHILD_USAGE_RECORDS_KEY] = serde_json::json!([{"bad": true}]);
        let malformed = child_usage_records_from_metadata(&metadata).expect("batch key wins");
        assert!(malformed.records.is_empty());
        assert_eq!(malformed.dropped_records, 1);
    }

    fn deepseek() -> BackgroundRoute<'static> {
        BackgroundRoute::new(ApiProvider::Deepseek, "deepseek-v4-flash")
            .with_base_url(Some(crate::config::DEFAULT_DEEPSEEK_BASE_URL))
    }

    fn deepseek_envelope() -> EffectiveRouteEnvelope {
        EffectiveRouteEnvelope::capture(
            None,
            ApiProvider::Deepseek,
            "deepseek-primary",
            "deepseek-v4-flash",
            Some(crate::config::DEFAULT_DEEPSEEK_BASE_URL),
            Utc::now(),
        )
    }

    #[test]
    fn child_metadata_round_trip_preserves_zero_and_reasoning_usage() {
        let route = deepseek_envelope();
        let usage = Usage {
            input_tokens: 0,
            output_tokens: 9,
            reasoning_tokens: Some(7),
            reasoning_replay_tokens: Some(3),
            ..Usage::default()
        };
        let mut metadata = serde_json::json!({"tool": "rlm_eval"});
        attach_child_usage_metadata(&mut metadata, &route, &usage);

        assert_eq!(child_route_envelope_from_metadata(&metadata), Some(route));
        assert_eq!(child_usage_from_metadata(&metadata), Some(usage));

        let mut zero_metadata = serde_json::json!({});
        let zero = Usage::default();
        attach_child_usage_metadata(&mut zero_metadata, &deepseek_envelope(), &zero);
        assert_eq!(child_usage_from_metadata(&zero_metadata), Some(zero));
    }

    #[test]
    fn runtime_owned_usage_is_isolated_from_tui_pool() {
        let _g = test_scope();
        let route = deepseek_envelope();
        let usage = small_usage();
        report_effective_route_for_runtime(
            scope_token(),
            Some("turn-a"),
            "response-a",
            &route,
            &usage,
        );
        report_effective_route_for_runtime(
            scope_token(),
            Some("turn-b"),
            "response-b",
            &route,
            &usage,
        );

        assert_eq!(take_runtime_usage("turn-a").records.len(), 1);
        assert!(take_runtime_usage("turn-a").records.is_empty());
        assert_eq!(take_runtime_usage("turn-b").records.len(), 1);
        assert!(
            drain().is_empty(),
            "runtime-owned usage must not enter TUI cost"
        );

        report_effective_route_for_runtime(scope_token(), None, "response-tui", &route, &usage);
        assert_eq!(drain().priced_turns, 1, "ownerless usage belongs to TUI");
    }

    /// Every piece of shared cost accounting is scoped to the test that owns
    /// it, including the durability sink registry.
    ///
    /// Sinks are keyed by owner id, and owner ids in tests are short fixture
    /// strings that repeat. A process-global registry let one test's
    /// `register_runtime_usage_sink` overwrite another's live sink, and let one
    /// test's `finish_runtime_usage_owner` retire it mid-flight — so a passing
    /// exactly-once assertion depended on which tests happened to run
    /// concurrently. This pins the isolation directly: a sink registered on
    /// another thread must be invisible here, and usage reported here must not
    /// reach it.
    #[test]
    fn runtime_usage_sinks_do_not_leak_across_test_threads() {
        let _g = test_scope();
        let owner = "shared-owner";
        let other_thread_deliveries = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // A concurrent test, standing in for any other test in the binary that
        // happens to use the same owner id.
        let deliveries = Arc::clone(&other_thread_deliveries);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let other = std::thread::spawn(move || {
            register_runtime_usage_sink(
                owner,
                Arc::new(move |_record| {
                    deliveries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    true
                }),
            );
            ready_tx.send(()).expect("signal registration");
            // Hold the registration open across this thread's assertions.
            done_rx.recv().expect("wait for the other test to finish");
            // The other thread's own reports still reach its own sink.
            report_effective_route_for_runtime(
                scope_token(),
                Some(owner),
                "response-other",
                &deepseek_envelope(),
                &small_usage(),
            );
        });
        ready_rx.recv().expect("other test registered its sink");

        // This thread never registered a sink, so its usage must fall through
        // to this thread's journal — not into the other test's sink.
        report_effective_route_for_runtime(
            scope_token(),
            Some(owner),
            "response-mine",
            &deepseek_envelope(),
            &small_usage(),
        );
        assert_eq!(
            other_thread_deliveries.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "another test's sink received this test's usage"
        );
        let mine = take_runtime_usage(owner);
        assert_eq!(mine.records.len(), 1);
        assert_eq!(mine.records[0].source_id, "response-mine");
        assert_eq!(mine.dropped_records, 0);

        // Retiring the owner here must not retire the other test's sink.
        finish_runtime_usage_owner(owner);
        done_tx.send(()).expect("release the other test");
        other.join().expect("other test thread");
        assert_eq!(
            other_thread_deliveries.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the other test's sink was retired by an unrelated test"
        );
    }

    #[test]
    fn runtime_usage_fallback_is_bounded_and_reports_truncation() {
        let _g = test_scope();
        let route = deepseek_envelope();
        for index in 0..(MAX_RUNTIME_USAGE_RECORDS_PER_OWNER + 3) {
            report_effective_route_for_runtime(
                scope_token(),
                Some("turn-bounded"),
                &format!("response-{index}"),
                &route,
                &small_usage(),
            );
        }

        let batch = take_runtime_usage("turn-bounded");
        assert_eq!(batch.records.len(), MAX_RUNTIME_USAGE_RECORDS_PER_OWNER);
        assert_eq!(batch.dropped_records, 3);
        assert!(drain().is_empty(), "runtime fallback must stay out of TUI");
    }

    #[test]
    fn route_labels_redact_local_paths_but_preserve_model_namespaces() {
        let route = EffectiveRouteEnvelope {
            provider: ApiProvider::Openrouter,
            provider_identity: "/Users/alice/.config/provider-secret".to_string(),
            model: "/Volumes/private/checkpoints/model.gguf".to_string(),
            billing_surface: None,
            endpoint_fingerprint: None,
            provider_live_pricing: None,
            billing_mode: RouteBillingMode::Metered,
            dispatched_at: Utc::now(),
        };
        let sanitized = route.sanitized_for_persistence();
        assert_eq!(sanitized.provider_identity, "redacted-local-path");
        assert_eq!(sanitized.model, "redacted-local-path");
        let receipt = route.receipt(&TurnCostAudit::unpriced(
            crate::pricing::UnpricedReason::NoPricingRow,
        ));
        assert!(!receipt.contains("alice"));
        assert!(!receipt.contains("Volumes"));

        assert_eq!(
            sanitize_persisted_route_label("anthropic/claude-sonnet-5"),
            "anthropic/claude-sonnet-5"
        );
    }

    #[test]
    fn route_label_sanitizer_rejects_credentials_urls_and_relative_paths() {
        for credential in [
            "Bearer secret-token",
            "Authorization: Basic abc123",
            "OPENAI_API_KEY=sk-secret",
            "service_token: ghp_secret",
            "hf_secret-token",
            "glpat-secret-token",
            "db-password=hunter2",
            "sk-live-secret",
            "https://alice:password@example.test/v1?api_key=secret#fragment",
        ] {
            let sanitized = sanitize_persisted_route_label(credential);
            assert!(
                sanitized.starts_with("redacted-"),
                "credential was not redacted: {credential:?} -> {sanitized:?}"
            );
        }
        for path in [
            ".ssh/id_ed25519",
            "../secrets/provider.key",
            "workspace/.ssh/config",
            "relative/path/to/credential",
            r"relative\path\credential",
        ] {
            assert_eq!(
                sanitize_persisted_route_label(path),
                "redacted-local-path",
                "path was not redacted: {path:?}"
            );
        }
        assert_eq!(
            sanitize_persisted_route_label("moonshot/kimi-k3"),
            "moonshot/kimi-k3"
        );
    }

    #[test]
    fn serialized_route_envelopes_records_and_child_receipts_are_secret_free() {
        let route = EffectiveRouteEnvelope {
            provider: ApiProvider::Custom,
            provider_identity: "Authorization: Bearer provider-secret".to_string(),
            model: "MODEL_API_KEY=sk-model-secret".to_string(),
            billing_surface: Some(
                "https://alice:password@example.test/v1?token=secret#fragment".to_string(),
            ),
            endpoint_fingerprint: Some("../.ssh/provider_key".to_string()),
            provider_live_pricing: None,
            billing_mode: RouteBillingMode::Metered,
            dispatched_at: Utc::now(),
        };
        let usage = Usage {
            input_tokens: 7,
            output_tokens: 3,
            ..Usage::default()
        };

        let envelope_json = serde_json::to_string(&route).expect("serialize envelope");
        let record_json = serde_json::to_string(&EffectiveRouteUsage {
            route: route.clone(),
            usage: usage.clone(),
        })
        .expect("serialize route usage");
        let child_json = serde_json::to_string(&child_usage_metadata_fields(&route, &usage))
            .expect("serialize child receipt");
        for serialized in [&envelope_json, &record_json, &child_json] {
            for secret in [
                "provider-secret",
                "sk-model-secret",
                "alice",
                "password",
                "token=secret",
                ".ssh",
            ] {
                assert!(
                    !serialized.contains(secret),
                    "serialized route leaked {secret:?}: {serialized}"
                );
            }
        }
    }

    #[test]
    fn report_adds_to_pool_and_drain_returns_then_resets() {
        let _g = test_scope();
        report(scope_token(), &deepseek(), &small_usage());
        let first = drain();
        assert!(
            first.estimate.usd > 0.0,
            "expected positive USD cost, got {first:?}"
        );
        assert!(
            first.estimate.cny > 0.0,
            "expected positive CNY cost, got {first:?}"
        );
        assert_eq!(first.priced_turns, 1);
        assert_eq!(first.unpriced_turns, 0);
        assert_eq!(first.cny_priced_turns, 1);
        assert_eq!(first.cny_unpriced_turns, 0);
        // The receipt names the route without leaking the endpoint URL.
        assert_eq!(first.route_receipts.len(), 1);
        let receipt = first.route_receipts.iter().next().expect("receipt");
        assert!(receipt.contains("provider=deepseek"), "{receipt}");
        assert!(receipt.contains("model=deepseek-v4-flash"), "{receipt}");
        assert!(receipt.contains("currency=usd+cny"), "{receipt}");
        assert!(!receipt.contains("http"), "{receipt}");

        let second = drain();
        assert!(second.is_empty(), "drain must zero the pool: {second:?}");
    }

    #[test]
    fn reports_from_a_closed_session_scope_are_discarded() {
        let _g = test_scope();
        let old_scope = scope_token();
        let settled = close_current_scope();
        assert!(settled.is_empty());

        report(old_scope, &deepseek(), &small_usage());
        assert!(drain().is_empty(), "old session usage crossed the boundary");

        report(scope_token(), &deepseek(), &small_usage());
        assert_eq!(drain().priced_turns, 1);
    }

    #[test]
    fn detached_advisor_and_translation_receipts_survive_new_exactly_once() {
        let _g = test_scope();
        let tmp = tempfile::tempdir().expect("tempdir");
        let manager = crate::session_manager::SessionManager::new(tmp.path().join("sessions"))
            .expect("session manager");
        let old_session_id = "origin-session";
        let new_session_id = "replacement-session";
        for session_id in [old_session_id, new_session_id] {
            let session = crate::session_manager::create_saved_session_with_id_and_mode(
                session_id.to_string(),
                &[],
                "deepseek-v4-flash",
                tmp.path(),
                0,
                None,
                Some("agent"),
            );
            manager.save_session(&session).expect("save session");
        }

        let origin_scope = scope_token();
        let owner = "interactive:origin-session:origin-turn";
        register_persistent_interactive_runtime_usage_sink_at(
            owner,
            origin_scope,
            old_session_id,
            "origin-turn",
            manager.sessions_dir().to_path_buf(),
        );
        let advisor_lease = acquire_runtime_usage_lease(owner).expect("advisor owner lease");
        finish_runtime_usage_owner(owner);

        // `/new` closes the old foreground generation while the detached
        // advisor and translation requests are still in flight.
        assert!(close_current_scope().is_empty());
        let route = deepseek_envelope();
        let usage = Usage {
            input_tokens: 17,
            output_tokens: 5,
            ..Usage::default()
        };
        for _ in 0..2 {
            report_effective_route_for_runtime(
                origin_scope,
                Some(owner),
                "advisor:origin-turn:response",
                &route,
                &usage,
            );
            report_unreceipted_provider_success(
                origin_scope,
                Some(owner),
                "advisor:origin-turn:missing-usage",
                &route,
            );
            assert!(report_effective_route_for_interactive_origin_with_manager(
                origin_scope,
                old_session_id,
                "origin-turn",
                "translation:origin-turn:assistant",
                &route,
                &usage,
                &manager,
            ));
            assert!(report_unreceipted_for_interactive_origin_with_manager(
                origin_scope,
                old_session_id,
                "origin-turn",
                "translation:origin-turn:thinking-missing-usage",
                &route,
                &manager,
            ));
        }
        drop(advisor_lease);

        let fallback = take_runtime_usage(owner);
        assert!(fallback.records.is_empty());
        assert!(fallback.drop_records.is_empty());
        assert_eq!(fallback.dropped_records, 0);
        assert!(drain().is_empty(), "late receipts polluted the new scope");

        let old = manager
            .load_session_snapshot(old_session_id)
            .expect("load origin session");
        assert_eq!(old.metadata.total_tokens, 44);
        assert_eq!(old.metadata.cost.priced_turns, 2);
        assert_eq!(old.metadata.cost.unpriced_turns, 2);
        assert_eq!(old.metadata.cost.cny_unpriced_turns, 2);
        assert_eq!(old.metadata.cost.usage_source_fingerprints.len(), 4);

        let replay = manager
            .load_session_snapshot(old_session_id)
            .expect("replay origin session");
        assert_eq!(replay.metadata.total_tokens, 44);
        assert_eq!(replay.metadata.cost.usage_source_fingerprints.len(), 4);

        let replacement = manager
            .load_session_snapshot(new_session_id)
            .expect("load replacement session");
        assert_eq!(replacement.metadata.total_tokens, 0);
        assert_eq!(replacement.metadata.cost.priced_turns, 0);
        assert_eq!(replacement.metadata.cost.unpriced_turns, 0);
        assert!(
            replacement
                .metadata
                .cost
                .usage_source_fingerprints
                .is_empty()
        );
    }

    #[test]
    fn report_counts_unknown_models_as_missing_spend_not_as_free() {
        let _g = test_scope();
        // NIM-hosted models intentionally have no DeepSeek pricing, but the
        // route *is* money-metered — so the turn is missing spend, not absent.
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::NvidiaNim, "deepseek-ai/deepseek-v4-pro"),
            &small_usage(),
        );
        let drained = drain();
        assert_eq!(drained.estimate, CostEstimate::default());
        assert_eq!(drained.priced_turns, 0);
        assert_eq!(drained.unpriced_turns, 1);
        assert!(!drained.unpriced_reasons.is_empty());
    }

    #[test]
    fn report_skips_codex_oauth_pricing_without_calling_it_incomplete() {
        let _g = test_scope();
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::OpenaiCodex, "gpt-5.5"),
            &small_usage(),
        );
        let drained = drain();
        assert_eq!(drained.estimate, CostEstimate::default());
        // Exactly non-metered: not counted in either coverage bucket.
        assert_eq!(drained.priced_turns, 0);
        assert_eq!(drained.unpriced_turns, 0);
        assert!(drained.unpriced_reasons.is_empty());
        assert!(drained.cny_unpriced_reasons.is_empty());
    }

    #[test]
    fn report_skips_stepfun_without_billing_surface() {
        let _g = test_scope();
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::Stepfun, "step-3.7-flash"),
            &small_usage(),
        );
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::Openrouter, "step-3.7-flash"),
            &small_usage(),
        );
        let drained = drain();
        assert_eq!(drained.estimate, CostEstimate::default());
        // Both are metered-or-unknown routes that could not be priced, so both
        // are reported as missing rather than dropped.
        assert_eq!(drained.unpriced_turns, 2);
    }

    /// A local runtime and a plan endpoint must never be guessed into public
    /// per-token dollars just because the provider also sells a paid API.
    #[test]
    fn local_and_plan_endpoints_are_never_treated_as_public_payg() {
        let _g = test_scope();
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::Ollama, "llama3.2"),
            &small_usage(),
        );
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::Zai, "glm-5.2")
                .with_base_url(Some("https://api.z.ai/api/coding/paas/v4")),
            &small_usage(),
        );
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::Moonshot, "kimi-for-coding")
                .with_base_url(Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL)),
            &small_usage(),
        );
        let drained = drain();
        assert_eq!(drained.estimate, CostEstimate::default());
        assert_eq!(drained.priced_turns, 0);
        assert_eq!(
            drained.unpriced_turns, 0,
            "exactly non-metered routes are not missing dollars: {drained:?}"
        );
        assert!(drained.unpriced_reasons.is_empty());
        assert!(drained.cny_unpriced_reasons.is_empty());
        assert!(
            drained
                .route_receipts
                .iter()
                .any(|receipt| receipt.contains("surface=zai-coding-plan")),
            "{drained:?}"
        );
        assert!(
            drained
                .route_receipts
                .iter()
                .any(|receipt| receipt.contains("surface=local-no-bill")),
            "{drained:?}"
        );
        assert!(
            drained
                .route_receipts
                .iter()
                .any(|receipt| receipt.contains("surface=moonshot-kimi-code")),
            "{drained:?}"
        );
    }

    /// The receipt carries an endpoint *fingerprint*, never the URL.
    #[test]
    fn route_receipts_fingerprint_the_endpoint_and_keep_secrets_out() {
        let _g = test_scope();
        let base_url = "https://api.deepseek.com/v1";
        report(
            scope_token(),
            &deepseek().with_base_url(Some(base_url)),
            &small_usage(),
        );
        let drained = drain();
        let receipt = drained.route_receipts.iter().next().expect("receipt");
        let expected_fp = endpoint_fingerprint(base_url).expect("valid endpoint fingerprint");
        assert!(
            receipt.contains(&format!("endpoint_fp={expected_fp}")),
            "{receipt}"
        );
        for needle in ["http", "api.deepseek.com", "sk-", "/Users/", "/home/"] {
            assert!(!receipt.contains(needle), "{needle} leaked into {receipt}");
        }
    }

    #[test]
    fn receipt_fields_are_bounded_and_secret_bearing_urls_are_not_hashed() {
        let hostile = format!("model\nAuthorization: bearer {}", "x".repeat(400));
        let receipt = route_receipt(
            ApiProvider::Deepseek,
            Some("identity\r\nforged=yes"),
            &hostile,
            Some(crate::pricing::FIRST_PARTY_PAYG_BILLING_SURFACE),
            None,
            RouteBillingMode::Metered,
            "usd+cny",
        );
        assert!(!receipt.contains('\n'), "{receipt}");
        assert!(!receipt.contains('\r'), "{receipt}");
        assert!(
            receipt.len() < 420,
            "receipt was not bounded: {}",
            receipt.len()
        );

        for secret_url in [
            "https://user:secret@api.example.com/v1",
            "https://api.example.com/v1?api_key=secret",
            "https://api.example.com/v1#secret",
        ] {
            assert_eq!(endpoint_fingerprint(secret_url), None, "{secret_url}");
        }
        assert_eq!(
            endpoint_fingerprint("https://API.Example.com/v1/")
                .expect("valid endpoint")
                .len(),
            64
        );
    }

    #[test]
    fn report_accumulates_across_multiple_calls() {
        let _g = test_scope();
        report(scope_token(), &deepseek(), &small_usage());
        report(scope_token(), &deepseek(), &small_usage());
        let total = drain();
        // Two equal reports — total must be 2× a single report.
        let single = crate::pricing::calculate_turn_cost_estimate_from_usage(
            "deepseek-v4-flash",
            &small_usage(),
        )
        .unwrap();
        assert!((total.estimate.usd - 2.0 * single.usd).abs() < 1e-12);
        assert!((total.estimate.cny - 2.0 * single.cny).abs() < 1e-12);
        assert_eq!(total.priced_turns, 2);
        // Identical routes collapse to one receipt rather than growing without
        // bound across a long session.
        assert_eq!(total.route_receipts.len(), 1);
    }

    /// A cache-write turn on a route with no published write rate must show up
    /// as missing spend naming the class, not as a discounted total.
    #[test]
    fn unpriced_cache_write_class_is_reported_not_absorbed() {
        let _g = test_scope();
        let write_heavy = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            prompt_cache_hit_tokens: Some(200_000),
            prompt_cache_write_tokens: Some(100_000),
            ..Default::default()
        };
        report(
            scope_token(),
            &BackgroundRoute::new(ApiProvider::Moonshot, "kimi-k2.7-code")
                .with_base_url(Some("https://api.moonshot.ai/v1")),
            &write_heavy,
        );
        let drained = drain();
        assert_eq!(drained.estimate, CostEstimate::default());
        assert_eq!(drained.unpriced_turns, 1);
        assert!(drained.unpriced_reasons.contains("missing_class_price"));
        assert!(drained.unpriced_classes.contains("cache_write"));
        assert!(
            drained
                .route_receipts
                .iter()
                .any(|receipt| receipt.contains("cache_write=yes")),
            "{drained:?}"
        );
    }
}
