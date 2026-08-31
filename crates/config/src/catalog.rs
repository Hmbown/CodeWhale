//! Models.dev-backed provider catalog snapshots and a secret-free live cache
//! (#3385, feeding EPIC #2608 and #3383).
//!
//! This module is **network-free** by construction. Callers supply parsed
//! [`crate::models_dev::ModelsDevCatalog`] JSON (bundled snapshot or live
//! refresh) and live [`ProviderCatalogDelta`]s; the HTTP `/models` fetch layer
//! lives above this module. Nothing here performs I/O or reads credentials.
//!
//! Layering (lowest precedence first):
//!
//! ```text
//! bundled Models.dev snapshot         (legacy seed, not competing truth)
//!   < bundled Codewhale catalog       (Codewhale-owned offline snapshot)
//!   < live Models.dev
//!   < live provider `/v1/models`      (credential-scoped workspace list)
//!   < live Codewhale signed catalog   (authority when present)
//!   < config.toml / user overrides
//! ```
//!
//! After #4187, live Models.dev rows are preferred whenever present. The bundled
//! asset remains so offline startup and failed refreshes still resolve defaults.
//!
//! Invariants preserved from #2608 / #3497:
//! - A catalog row is **not** an executable route. Rows still compile through
//!   `RouteResolver` into a `ReadyRouteCandidate` before execution.
//! - `wire_model_id` is kept separate from `canonical_model`; a provider row may
//!   not expose a canonical `base_model` join, and a prefix never proves
//!   canonical ownership.
//! - Unknown / custom / local rows are supported with explicit provenance and a
//!   `None` canonical model.
//!
//! The on-disk cache format intentionally uses plain `String` identity fields
//! rather than the internal route newtypes, so the persisted shape is decoupled
//! from internal types and trivially auditable for "no secrets" (see
//! [`ProviderCatalogCache`] tests).

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models_dev::{ModelsDevCatalog, ModelsDevCost, ModelsDevLimit, ModelsDevModalities};
use crate::route::{ModelId, ProviderId, ProviderModelOffering, RouteLimits, WireModelId};

/// Provenance of a catalog row. Drives layer precedence and UI provenance.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogSource {
    /// Offline/stale bundled seed (Models.dev-shaped snapshot). Not competing
    /// truth — live Models.dev rows override this layer (#4188).
    #[default]
    Bundled,
    /// A provider live `/models` row, scoped to a base-URL fingerprint and the
    /// unix timestamp it was fetched at.
    Live {
        base_url_fingerprint: String,
        fetched_at: u64,
    },
    /// A user / custom override (custom endpoint, pinned model, explicit facts).
    UserOverride,
    /// Live models.dev refresh (layer 10). Distinct from provider `/v1/models`.
    ModelsDevLive { fetched_at: u64 },
    /// `config.toml` `[providers.*]` override (layer 30).
    ConfigOverride,
    /// Codewhale-owned bundled catalog snapshot (offline authority seed).
    CodewhaleBundled { revision: String },
    /// Live Codewhale signed catalog fetched from CWC / `CODEWHALE_CATALOG_URL`.
    CodewhaleLive { revision: String, fetched_at: u64 },
}

/// One catalog-layer offering row.
///
/// This carries the routing identity (provider + wire id + optional canonical
/// model + endpoint) plus the offering-owned Models.dev facts CodeWhale wants to
/// preserve (family, limits, cost, reasoning support/options). It is a superset
/// of [`ProviderModelOffering`]; use [`CatalogOffering::to_offering`] to project
/// the minimal routing identity the resolver consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CatalogOffering {
    /// Provider id serving this offering.
    pub provider: String,
    /// Provider-owned wire id sent on the request (verbatim).
    pub wire_model_id: String,
    /// Canonical model identity, only when an explicit join exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_model: Option<String>,
    /// Endpoint key the offering is served on (e.g. `chat`).
    pub endpoint_key: String,
    /// Whether this is the provider's default offering.
    #[serde(default)]
    pub default_for_provider: bool,
    /// Model family/series as exposed for this offering (e.g. `glm`, `deepseek`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Token limits for this offering, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<ModelsDevLimit>,
    /// Provider-scoped pricing, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<ModelsDevCost>,
    /// Input/output modalities for this offering, when known. Carried as the
    /// raw Models.dev shape so a factual `text` vs `multimodal` label can be
    /// derived without guessing; `None` means the layer did not state it (an
    /// unknown, not "text-only").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelsDevModalities>,
    /// Whether this provider offering accepts attachments, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment: Option<bool>,
    /// Whether this offering supports reasoning, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    /// Whether tool calling is supported, when known (#4115).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<bool>,
    /// Whether structured output is supported, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
    /// Provider-scoped reasoning controls / accepted effort metadata. Kept as
    /// raw JSON so the same model family served through different gateways can
    /// expose different effort vocabularies without lossy collapsing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_options: Vec<Value>,
    /// Where this row came from.
    pub source: CatalogSource,
}

impl CatalogOffering {
    /// The provider id as a route newtype.
    #[must_use]
    pub fn provider_id(&self) -> ProviderId {
        ProviderId::from(self.provider.clone())
    }

    /// The wire model id as a route newtype.
    #[must_use]
    pub fn wire_id(&self) -> WireModelId {
        WireModelId::from(self.wire_model_id.clone())
    }

    /// Project the minimal routing identity the resolver consumes.
    ///
    /// The catalog deliberately carries richer facts than routing needs; this
    /// drops most of them so `RouteResolver::from_offerings` stays the single
    /// seam. The route-facing pricing meter is the exception: it is projected
    /// here (where the offering's sourced `cost` is in scope) via
    /// [`crate::pricing::route_pricing_sku`] so a resolved candidate can carry
    /// honest pricing without the route layer ever seeing raw cost (#3085).
    #[must_use]
    pub fn to_offering(&self) -> ProviderModelOffering {
        ProviderModelOffering {
            provider: self.provider_id(),
            canonical_model: self.canonical_model.clone().map(ModelId::from),
            wire_model_id: self.wire_id(),
            endpoint_key: self.endpoint_key.clone(),
            default_for_provider: self.default_for_provider,
            limits: self
                .limit
                .as_ref()
                .map(RouteLimits::from)
                .unwrap_or_default(),
            capabilities: crate::route::RouteCapabilities {
                attachments: crate::route::CapabilityState::from_optional_bool(self.attachment),
                image_input: crate::models_dev::image_input_support(self.modalities.as_ref()),
                reasoning: crate::route::CapabilityState::from_optional_bool(self.reasoning),
                native_tool_calls: crate::route::CapabilityState::from_optional_bool(
                    self.tool_call,
                ),
                structured_output: crate::route::CapabilityState::from_optional_bool(
                    self.structured_output,
                ),
                server_side_web_search: crate::route::documented_server_side_web_search(
                    &self.provider,
                    &self.wire_model_id,
                ),
                ..crate::route::RouteCapabilities::default()
            },
            pricing: crate::pricing::route_pricing_sku(self),
        }
    }

    /// Stable identity key for de-duplication and layer merging.
    fn merge_key(&self) -> (String, String) {
        (self.provider.clone(), self.wire_model_id.clone())
    }
}

/// Committed offline/stale Models.dev-shaped catalog snapshot (#3385 / #4188).
///
/// This is **not** a competing curated source of truth. Preferred metadata comes
/// from the live Models.dev catalog (#4187). The bundled asset is a compact
/// network-free seed of verified in-repo defaults (context/output from
/// `crates/tui/src/models.rs`, USD pricing from `crates/tui/src/pricing.rs`) so
/// [`crate::route::RouteResolver::new`] and pickers still work offline or after
/// a failed refresh. See the asset's `_meta.role` / `_meta.source` and the
/// honesty rule on omitted pricing (`UnknownOrStale`, never a fabricated zero).
pub const BUNDLED_MODELS_DEV_JSON: &str = include_str!("../assets/models_dev.bundled.json");

/// Parse-once cache for the committed bundled Models.dev snapshot.
///
/// The bundled asset is compile-time constant (`include_str!`), so its parsed
/// form is immutable and safe to share process-wide. Before this cache, every
/// call site parsed the full snapshot independently — the client route path,
/// pickers, provider lake, and fleet identity each paid a full serde parse of
/// ~50KB on their own first use (perf-attributed during the 0.9.x perf
/// gauntlet: `ModelsDevCost` serde frames in startup profiles).
static BUNDLED_MODELS_DEV_CATALOG: OnceLock<ModelsDevCatalog> = OnceLock::new();

/// Parse the committed bundled Models.dev snapshot.
///
/// The first call parses; later calls return the shared parsed catalog.
///
/// # Panics
/// Panics only if the committed asset is not valid Models.dev JSON. The
/// `tests::bundled_asset_parses` guard makes that a build-time failure, so this
/// never panics in shipped builds.
#[must_use]
pub fn bundled_models_dev_catalog() -> &'static ModelsDevCatalog {
    BUNDLED_MODELS_DEV_CATALOG.get_or_init(|| {
        ModelsDevCatalog::parse_json(BUNDLED_MODELS_DEV_JSON)
            .expect("committed bundled Models.dev asset must be valid JSON")
    })
}

/// Bundled-layer [`CatalogOffering`] rows from the offline snapshot (#4188).
///
/// Lowest-precedence catalog layer: every text-chat row from
/// [`BUNDLED_MODELS_DEV_JSON`], tagged [`CatalogSource::Bundled`]. Live Models.dev
/// rows override these on `(provider, wire_model_id)` when available.
#[must_use]
pub fn bundled_catalog_offerings() -> Vec<CatalogOffering> {
    bundled_offerings_from_models_dev(bundled_models_dev_catalog())
}

/// Hydrate bundled [`CatalogOffering`] rows from a parsed Models.dev catalog.
///
/// Only text-chat offerings are emitted (TTS/audio-only rows stay in the parsed
/// catalog but are excluded from route candidates, matching
/// [`ModelsDevCatalog::provider_offerings`]). Each row is tagged
/// [`CatalogSource::Bundled`]. No canonical model is inferred from a prefix; the
/// canonical link is set only from an explicit `base_model`.
///
/// Provider ids are kept verbatim from the Models.dev payload (the committed
/// bundled asset already uses CodeWhale ids). Live refresh normalizes aliases
/// via [`live_offerings_from_models_dev`].
#[must_use]
pub fn bundled_offerings_from_models_dev(catalog: &ModelsDevCatalog) -> Vec<CatalogOffering> {
    offerings_from_models_dev(catalog, CatalogSource::Bundled, false)
}

/// Hydrate live [`CatalogOffering`] rows from a fetched Models.dev catalog (#4187).
///
/// Same text-chat filter as [`bundled_offerings_from_models_dev`], but each row is
/// tagged [`CatalogSource::Live`] with the Models.dev URL fingerprint and fetch
/// timestamp. Provider keys are normalized onto CodeWhale [`crate::ProviderKind`]
/// ids when an alias match exists (`moonshotai` → `moonshot`, `togetherai` →
/// `together`, `zhipuai` → `zai`, …); unknown Models.dev providers keep their
/// upstream id so they stay discoverable without becoming executable routes.
#[must_use]
pub fn live_offerings_from_models_dev(
    catalog: &ModelsDevCatalog,
    base_url_fingerprint: &str,
    fetched_at: u64,
) -> Vec<CatalogOffering> {
    offerings_from_models_dev(
        catalog,
        CatalogSource::Live {
            base_url_fingerprint: base_url_fingerprint.to_string(),
            fetched_at,
        },
        true,
    )
}

fn offerings_from_models_dev(
    catalog: &ModelsDevCatalog,
    source: CatalogSource,
    normalize_provider_ids: bool,
) -> Vec<CatalogOffering> {
    let mut out = Vec::new();
    for (provider_key, provider) in &catalog.providers {
        let raw_id = if provider.id.trim().is_empty() {
            provider_key.trim()
        } else {
            provider.id.trim()
        };
        if raw_id.is_empty() {
            continue;
        }
        let provider_id = if normalize_provider_ids {
            // Normalize Models.dev provider ids onto CodeWhale kinds when known
            // (#4186). Unknown upstream ids are kept verbatim for catalog browsing.
            crate::ProviderKind::parse(raw_id)
                .map(|kind| kind.as_str().to_string())
                .unwrap_or_else(|| raw_id.to_string())
        } else {
            raw_id.to_string()
        };
        for model in provider.models.values() {
            if !model.supports_text_chat() {
                continue;
            }
            out.push(CatalogOffering {
                provider: provider_id.clone(),
                wire_model_id: model.id.clone(),
                canonical_model: model.base_model.clone(),
                endpoint_key: "chat".to_string(),
                default_for_provider: model.default_for_provider,
                family: model.family.clone(),
                limit: model.limit.clone(),
                cost: model.cost.clone(),
                modalities: model.modalities.clone(),
                attachment: model.attachment,
                reasoning: model.reasoning,
                tool_call: model.tool_call,
                structured_output: model.structured_output,
                reasoning_options: model.reasoning_options.clone(),
                source: source.clone(),
            });
        }
    }
    out
}

/// A provider's live `/models` refresh result, scoped to a base-URL fingerprint.
///
/// Returned as a delta rather than mutating any global model state directly, per
/// the #3385 architecture contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderCatalogDelta {
    /// Provider this delta belongs to.
    pub provider: String,
    /// Fingerprint of the base URL the rows were fetched from.
    pub base_url_fingerprint: String,
    /// Unix seconds the rows were fetched at.
    pub fetched_at: u64,
    /// Live offering rows. Sources are normalized to `Live` on ingest.
    pub offerings: Vec<CatalogOffering>,
}

/// Why a provider live catalog refresh did not produce usable rows.
///
/// Every variant must leave previously cached / bundled / configured rows
/// available; a refresh failure is never fatal to model selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRefreshError {
    /// 401 — auth missing or invalid.
    Unauthorized,
    /// 403 — auth present but not permitted.
    Forbidden,
    /// 404 — provider does not expose `/models` at this base URL.
    NotFound,
    /// 429 — rate limited.
    RateLimited,
    /// Response was not parseable as a model listing.
    InvalidResponse,
    /// Provider returned an empty model list.
    EmptyList,
    /// Transport / network failure.
    Network,
}

/// Freshness / health of a provider's cached live catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogStatus {
    /// Cached rows are within their TTL.
    Fresh,
    /// Cached rows exist but are past their TTL.
    Stale { age_secs: u64 },
    /// The last refresh failed; any rows present are from an earlier success.
    Failed { reason: CatalogRefreshError },
    /// No refresh has been attempted for this provider + base URL.
    Unknown,
}

/// A secret-free cached provider catalog for one provider + base-URL fingerprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedProviderCatalog {
    /// Provider id.
    pub provider: String,
    /// Base-URL fingerprint the rows were fetched from.
    pub base_url_fingerprint: String,
    /// Unix seconds of the last successful fetch (unchanged on failure).
    pub fetched_at: u64,
    /// Time-to-live, in seconds, after which rows are considered stale.
    pub ttl_secs: u64,
    /// Cached live offering rows (possibly empty after a failure with no prior).
    pub offerings: Vec<CatalogOffering>,
    /// Last known status of this entry.
    pub status: CatalogStatus,
}

impl CachedProviderCatalog {
    /// Age in seconds relative to `now_unix`, saturating at zero for clock skew.
    #[must_use]
    pub fn age_secs(&self, now_unix: u64) -> u64 {
        now_unix.saturating_sub(self.fetched_at)
    }

    /// Whether the cached rows are past their TTL at `now_unix`.
    ///
    /// A `ttl_secs` of zero means "always stale" (never serve as fresh).
    #[must_use]
    pub fn is_stale(&self, now_unix: u64) -> bool {
        self.age_secs(now_unix) >= self.ttl_secs
    }

    /// Whether this entry may contribute live offerings at `now_unix`.
    ///
    /// An entry is fresh only when it is within its TTL **and** its last
    /// recorded refresh succeeded. A `Failed` entry is never fresh even inside
    /// its TTL window — its rows survive a failed refresh for explicit fallback
    /// display via [`ProviderCatalogCache::get`], but they are not served as
    /// current live data.
    #[must_use]
    pub fn is_fresh(&self, now_unix: u64) -> bool {
        !self.is_stale(now_unix) && !matches!(self.status, CatalogStatus::Failed { .. })
    }
}

/// A secret-free store of cached provider catalogs, keyed by provider + base-URL
/// fingerprint.
///
/// Scoping rule (#3385): the SAME provider on DIFFERENT base URLs must not share
/// rows, and DIFFERENT providers on the same base URL must not share rows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderCatalogCache {
    /// Entries keyed by [`ProviderCatalogCache::cache_key`].
    #[serde(default)]
    pub entries: BTreeMap<String, CachedProviderCatalog>,
}

impl ProviderCatalogCache {
    /// Construct an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the composite cache key for a provider + base-URL fingerprint.
    #[must_use]
    pub fn cache_key(provider: &str, base_url_fingerprint: &str) -> String {
        // Unit separator avoids ambiguity between provider and fingerprint.
        format!("{}\u{1f}{}", provider.trim(), base_url_fingerprint.trim())
    }

    /// Look up a cached entry by provider + base-URL fingerprint.
    #[must_use]
    pub fn get(
        &self,
        provider: &str,
        base_url_fingerprint: &str,
    ) -> Option<&CachedProviderCatalog> {
        self.entries
            .get(&Self::cache_key(provider, base_url_fingerprint))
    }

    /// Record a successful refresh, replacing any prior entry for this scope.
    ///
    /// Offering sources are normalized to [`CatalogSource::Live`] with the
    /// delta's fingerprint and `fetched_at`, so cached rows always carry honest
    /// provenance regardless of how the delta was assembled.
    pub fn record_success(&mut self, delta: ProviderCatalogDelta, ttl_secs: u64) {
        let ProviderCatalogDelta {
            provider,
            base_url_fingerprint,
            fetched_at,
            offerings,
        } = delta;
        let offerings = offerings
            .into_iter()
            .map(|mut row| {
                row.source = CatalogSource::Live {
                    base_url_fingerprint: base_url_fingerprint.clone(),
                    fetched_at,
                };
                row
            })
            .collect();
        let key = Self::cache_key(&provider, &base_url_fingerprint);
        self.entries.insert(
            key,
            CachedProviderCatalog {
                provider,
                base_url_fingerprint,
                fetched_at,
                ttl_secs,
                offerings,
                status: CatalogStatus::Fresh,
            },
        );
    }

    /// Record a refresh failure.
    ///
    /// Previously cached rows for this scope are preserved (so the UI can still
    /// offer them with a visible "stale/failed" status); only the status is
    /// updated. When no prior entry exists, an empty `Failed` entry is created so
    /// the failure is observable.
    pub fn record_failure(
        &mut self,
        provider: &str,
        base_url_fingerprint: &str,
        reason: CatalogRefreshError,
    ) {
        let key = Self::cache_key(provider, base_url_fingerprint);
        match self.entries.get_mut(&key) {
            Some(entry) => entry.status = CatalogStatus::Failed { reason },
            None => {
                self.entries.insert(
                    key,
                    CachedProviderCatalog {
                        provider: provider.trim().to_string(),
                        base_url_fingerprint: base_url_fingerprint.trim().to_string(),
                        fetched_at: 0,
                        ttl_secs: 0,
                        offerings: Vec::new(),
                        status: CatalogStatus::Failed { reason },
                    },
                );
            }
        }
    }

    /// The resolved status of an entry at `now_unix`.
    ///
    /// A `Fresh`-recorded entry that has since aged past its TTL reports
    /// `Stale`; `Failed`/`Unknown` are returned as stored.
    #[must_use]
    pub fn status(
        &self,
        provider: &str,
        base_url_fingerprint: &str,
        now_unix: u64,
    ) -> CatalogStatus {
        match self.get(provider, base_url_fingerprint) {
            None => CatalogStatus::Unknown,
            Some(entry) => match &entry.status {
                CatalogStatus::Failed { reason } => CatalogStatus::Failed { reason: *reason },
                CatalogStatus::Unknown => CatalogStatus::Unknown,
                CatalogStatus::Fresh | CatalogStatus::Stale { .. } => {
                    if entry.is_stale(now_unix) {
                        CatalogStatus::Stale {
                            age_secs: entry.age_secs(now_unix),
                        }
                    } else {
                        CatalogStatus::Fresh
                    }
                }
            },
        }
    }

    /// Fresh (within-TTL) live offerings for one provider + base URL at
    /// `now_unix`. Stale or failed entries contribute nothing here; callers fall
    /// back to bundled/configured rows and surface the status separately.
    #[must_use]
    pub fn fresh_offerings(
        &self,
        provider: &str,
        base_url_fingerprint: &str,
        now_unix: u64,
    ) -> Vec<CatalogOffering> {
        match self.get(provider, base_url_fingerprint) {
            Some(entry) if entry.is_fresh(now_unix) => entry.offerings.clone(),
            _ => Vec::new(),
        }
    }

    /// All fresh live offerings across every cached provider + base URL.
    #[must_use]
    pub fn all_fresh_offerings(&self, now_unix: u64) -> Vec<CatalogOffering> {
        self.entries
            .values()
            .filter(|entry| entry.is_fresh(now_unix))
            .flat_map(|entry| entry.offerings.clone())
            .collect()
    }

    /// Live offerings that pickers may still show: fresh rows plus stale / prior
    /// rows that survived a failed refresh (#4139).
    ///
    /// Unlike [`Self::all_fresh_offerings`], this keeps past-TTL and
    /// `Failed`-status entries as long as they still hold offering rows. Empty
    /// entries contribute nothing; callers fall back to the bundled snapshot.
    /// `now_unix` is accepted for API symmetry with the fresh helper (age chips
    /// live above this layer).
    #[must_use]
    pub fn all_visible_offerings(&self, _now_unix: u64) -> Vec<CatalogOffering> {
        self.entries
            .values()
            .filter(|entry| !entry.offerings.is_empty())
            .flat_map(|entry| entry.offerings.clone())
            .collect()
    }
}

/// A compiled, layer-merged catalog snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    /// Merged offerings, de-duplicated by (provider, wire id), in stable order.
    pub offerings: Vec<CatalogOffering>,
}

impl CatalogSnapshot {
    /// Project routing offerings for `RouteResolver::from_offerings`.
    #[must_use]
    pub fn to_offerings(&self) -> Vec<ProviderModelOffering> {
        self.offerings
            .iter()
            .map(CatalogOffering::to_offering)
            .collect()
    }

    /// All offerings for one provider id.
    #[must_use]
    pub fn offerings_for_provider(&self, provider: &str) -> Vec<&CatalogOffering> {
        self.offerings
            .iter()
            .filter(|row| row.provider == provider)
            .collect()
    }
}

/// Builds a [`CatalogSnapshot`] by merging layers in precedence order.
///
/// Last writer wins per `(provider, wire id)` field. Policy DENY is applied
/// after every layer and is never overridden:
///
/// ```text
///  0 bundled              committed models.dev-shaped snapshot
///  5 codewhale bundled    Codewhale-owned offline snapshot
/// 10 live models.dev      models.dev refresh
/// 20 provider             per-provider /v1/models refresh
/// 25 codewhale live       signed CWC catalog (authority)
/// 30 config               config.toml [providers.*] overrides
/// 40 user                 user approved set
///    policy DENY          last, never overridden
/// ```
///
/// [`Self::with_live`] remains the combined live bucket so existing callers
/// keep working; prefer [`Self::with_models_dev_live`] / [`Self::with_provider_live`]
/// for the split.
#[derive(Debug, Clone, Default)]
pub struct CatalogCompiler {
    bundled: Vec<CatalogOffering>,
    codewhale_bundled: Vec<CatalogOffering>,
    models_dev_live: Vec<CatalogOffering>,
    live: Vec<CatalogOffering>,
    provider_live: Vec<CatalogOffering>,
    codewhale_live: Vec<CatalogOffering>,
    config: Vec<CatalogOffering>,
    overrides: Vec<CatalogOffering>,
    policy: crate::route::CatalogPolicy,
}

impl CatalogCompiler {
    /// Start an empty compiler.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add bundled (lowest-precedence) rows.
    #[must_use]
    pub fn with_bundled(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.bundled.extend(rows);
        self
    }

    /// Seed bundled rows from a parsed Models.dev catalog.
    #[must_use]
    pub fn with_models_dev(mut self, catalog: &ModelsDevCatalog) -> Self {
        self.bundled
            .extend(bundled_offerings_from_models_dev(catalog));
        self
    }

    /// Add Codewhale-owned bundled catalog rows (layer 5).
    #[must_use]
    pub fn with_codewhale_bundled(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.codewhale_bundled.extend(rows);
        self
    }

    /// Add live signed Codewhale catalog rows (layer 25; authority when present).
    #[must_use]
    pub fn with_codewhale_live(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.codewhale_live.extend(rows);
        self
    }

    /// Add live models.dev refresh rows (layer 10).
    #[must_use]
    pub fn with_models_dev_live(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.models_dev_live.extend(rows);
        self
    }

    /// Add live (combined models.dev + provider) rows.
    ///
    /// Prefer [`Self::with_models_dev_live`] / [`Self::with_provider_live`].
    /// Kept so existing callers still compile; these rows sit between
    /// models.dev live and provider live.
    #[must_use]
    pub fn with_live(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.live.extend(rows);
        self
    }

    /// Add per-provider `/v1/models` refresh rows (layer 20).
    #[must_use]
    pub fn with_provider_live(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.provider_live.extend(rows);
        self
    }

    /// Add `config.toml` `[providers.*]` override rows (layer 30).
    #[must_use]
    pub fn with_config(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.config.extend(rows);
        self
    }

    /// Add user/custom override (highest catalog-layer precedence) rows.
    #[must_use]
    pub fn with_overrides(mut self, rows: Vec<CatalogOffering>) -> Self {
        self.overrides.extend(rows);
        self
    }

    /// Attach policy evaluated after every layer. DENY is never overridden.
    #[must_use]
    pub fn with_policy(mut self, policy: crate::route::CatalogPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Merge all layers into a deterministic snapshot, then apply policy DENY.
    #[must_use]
    pub fn compile(self) -> CatalogSnapshot {
        let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
        for row in self
            .bundled
            .into_iter()
            .chain(self.codewhale_bundled)
            .chain(self.models_dev_live)
            .chain(self.live)
            .chain(self.provider_live)
            .chain(self.codewhale_live)
            .chain(self.config)
            .chain(self.overrides)
        {
            merged.insert(row.merge_key(), row);
        }
        let offerings = merged
            .into_values()
            .filter(|row| self.policy.allows(&row.provider, &row.wire_model_id))
            .collect();
        CatalogSnapshot { offerings }
    }
}

/// Normalize a base URL and fingerprint it for cache scoping.
///
/// Normalization folds case in the scheme/host, trims trailing slashes, and
/// drops a default-port suffix, so cosmetically different spellings of the same
/// endpoint share a cache scope while genuinely different endpoints do not. The
/// fingerprint is a SHA-256 digest. Secret-bearing URLs are mapped to one
/// constant redacted input before hashing, so userinfo, query credentials, and
/// fragments never enter the digest function at all.
#[must_use]
pub fn base_url_fingerprint(base_url: &str) -> String {
    use sha2::Digest as _;

    let normalized = secret_free_fingerprint_input(base_url);
    let digest = sha2::Sha256::digest(normalized.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn secret_free_fingerprint_input(base_url: &str) -> String {
    const REDACTED: &str = "invalid-or-secret-bearing-url";
    let trimmed = base_url.trim();
    if let Some((scheme, rest)) = trimmed.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") {
            return REDACTED.to_string();
        }
        let authority_end = rest.find('/').unwrap_or(rest.len());
        let authority_with_userinfo = &rest[..authority_end];
        if authority_with_userinfo.contains(['?', '#']) {
            return REDACTED.to_string();
        }
        let authority = authority_with_userinfo
            .rsplit_once('@')
            .map_or(authority_with_userinfo, |(_, host)| host);
        if authority.is_empty() {
            return REDACTED.to_string();
        }
        let path = rest[authority_end..]
            .split(['?', '#'])
            .next()
            .unwrap_or_default();
        return normalize_base_url(&format!("{scheme}://{authority}{path}"));
    }
    // Scheme-less input still has an authority, and it can still carry
    // `user:pass@` userinfo. Strip it exactly as the scheme branch does, so the
    // digest input never contains a credential.
    let without_query = trimmed.split(['?', '#']).next().unwrap_or_default();
    let authority_end = without_query.find('/').unwrap_or(without_query.len());
    let authority = &without_query[..authority_end];
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if authority.is_empty() {
        return REDACTED.to_string();
    }
    normalize_base_url(&format!("{authority}{}", &without_query[authority_end..]))
}

fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    // Lowercase only the scheme://host authority; leave the path case-sensitive.
    if let Some(idx) = trimmed.find("://") {
        let (scheme, rest) = trimmed.split_at(idx);
        let scheme = scheme.to_ascii_lowercase();
        let rest = &rest[3..];
        let (authority, path) = match rest.find('/') {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, ""),
        };
        let authority = authority.to_ascii_lowercase();
        // Strip only the scheme's own default port, so a non-default pairing
        // such as `http://host:443` stays distinct from `http://host`.
        let default_port = match scheme.as_str() {
            "https" => Some(":443"),
            "http" => Some(":80"),
            _ => None,
        };
        let authority = default_port
            .and_then(|port| authority.strip_suffix(port))
            .unwrap_or(&authority);
        format!("{scheme}://{authority}{path}")
    } else {
        trimmed.to_ascii_lowercase()
    }
}

/// Current unix time in seconds, for callers assembling deltas / cache entries.
///
/// Pure cache logic takes `now_unix` explicitly so it stays deterministic in
/// tests; this helper is the one place that reads the wall clock.
#[must_use]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
