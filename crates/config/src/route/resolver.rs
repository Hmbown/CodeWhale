//! The sole producer of [`ReadyRouteCandidate`] (#3384).
//!
//! [`RouteResolver::resolve`] is the ONLY caller of
//! [`ReadyRouteCandidate::new`]. It resolves a [`RouteRequest`] into an
//! executable route using:
//!
//! 1. provider from `explicit_provider` ONLY (no base-URL / prefix sniffing);
//!    when absent, the workspace default provider scope is used. The provider
//!    is NEVER inferred from a model prefix.
//! 2. the model selector, interpreted STRICTLY within that provider's scope
//!    against resolver-provided offerings plus the provider default. The default
//!    resolver uses [`bundled_offerings`], while tests or snapshot loaders can
//!    inject Models.dev-derived rows. Prefixed selectors are preserved verbatim
//!    as the [`WireModelId`].
//! 3. `auto` => the [`LogicalModelRef::is_auto`] sentinel, never a literal
//!    model.
//!
//! It encodes its OWN minimal direct/aggregator/local classification because
//! the tui helpers (`provider_passes_model_through` /
//! `accepts_custom_model_ids`) are not reachable from `crates/config`. The
//! classification here is deliberately NARROWER than tui's `validate_route`:
//! it only rejects [`RouteError::ForeignModelForDirectProvider`] for a small
//! set of strict direct providers given a clearly-foreign selector;
//! aggregators, local, and custom endpoints pass through `Ok` with
//! `validation.ok == true`.
//!
//! There is deliberately no prompt-text / freeform field on [`RouteRequest`],
//! which structurally bars prompt-content routing.

use super::candidate::{
    PricingSku, ReadyRouteCandidate, ResolvedAuthSource, ResolvedEndpoint, ValidationReport,
};
use super::descriptor::ProviderDescriptor;
use super::errors::RouteError;
use super::ids::{LogicalModelRef, ModelId, ProviderId, WireModelId};
use super::offering::{ProviderModelOffering, RouteLimits, bundled_offerings};
use crate::catalog::{CatalogOffering, bundled_catalog_offerings};
use crate::{ProviderKind, provider_preserves_custom_base_url_model};

/// A request to resolve into an executable route.
///
/// Note the absence of any prompt-text/freeform field: the resolver cannot see
/// prompt content, so it cannot silently route on it.
#[derive(Debug, Clone, Default)]
pub struct RouteRequest {
    /// Explicit provider choice. The ONLY source of provider identity.
    pub explicit_provider: Option<ProviderKind>,
    /// The model the caller selected (may be `auto` or prefixed).
    pub model_selector: Option<LogicalModelRef>,
    /// A previously-saved provider wire model id, used as scope fallback.
    pub saved_provider_model: Option<WireModelId>,
    /// An explicit base URL override for the endpoint.
    pub base_url_override: Option<String>,
}

/// Resolves [`RouteRequest`]s into [`ReadyRouteCandidate`]s.
#[derive(Debug, Clone)]
pub struct RouteResolver {
    offerings: Vec<ProviderModelOffering>,
}

impl Default for RouteResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteResolver {
    /// Construct a resolver with CodeWhale's bundled offline offerings.
    ///
    /// The default offerings are the committed Models.dev-shaped catalog asset
    /// (`crate::catalog::bundled_catalog_offerings`, real context windows and
    /// honest per-row `cost`) merged with the tiny hand seam
    /// ([`bundled_offerings`]). The hand seam is kept and given precedence on a
    /// `(provider, wire id)` collision: it encodes the curated canonical-model
    /// joins the route invariants depend on (e.g. a DeepSeek-native row and the
    /// aggregator rows that map a prefixed wire id back to `deepseek-v4-pro`),
    /// which generated Models.dev JSON does not prove. Asset-only rows (GLM,
    /// Kimi, MiniMax, Qwen, …) add the real provider/model facts the picker and
    /// candidates were previously missing.
    #[must_use]
    pub fn new() -> Self {
        Self::from_offerings(default_offerings())
    }

    /// Construct a resolver from a provider-scoped offering catalog.
    ///
    /// This is the bridge for Models.dev snapshots: callers parse a catalog,
    /// emit provider offerings, then hand those rows to the resolver without
    /// changing route-resolution semantics.
    #[must_use]
    pub fn from_offerings(offerings: Vec<ProviderModelOffering>) -> Self {
        Self { offerings }
    }

    /// Resolve a request into an executable route candidate.
    ///
    /// # Errors
    /// Returns [`RouteError`] when the model is empty, the provider is invalid,
    /// or a clearly-foreign model is requested for a strict direct provider.
    pub fn resolve(&self, req: &RouteRequest) -> Result<ReadyRouteCandidate, RouteError> {
        // 1. Provider scope from explicit choice only; default otherwise.
        //    The provider is NEVER inferred from a model prefix.
        let provider_kind = req.explicit_provider.unwrap_or_default();
        let descriptor = ProviderDescriptor::for_kind(provider_kind);
        let provider_id = descriptor.id();
        let default_offering = self.default_offering(&provider_id);

        // 2. Determine the logical selector from explicit choice, then the
        //    saved-model fallback, then the provider default.
        let logical_model = match &req.model_selector {
            Some(selector) => selector.clone(),
            None => {
                // No selector: fall back to saved wire model, then provider
                // default. Both stay in the resolved provider's scope.
                let raw = req
                    .saved_provider_model
                    .as_ref()
                    .map(|w| w.as_str().to_string())
                    .unwrap_or_else(|| {
                        default_offering.map_or_else(
                            || descriptor.default_wire_model().as_str().to_string(),
                            |offering| offering.wire_model_id.as_str().to_string(),
                        )
                    });
                LogicalModelRef::from(raw)
            }
        };

        // Reject an empty selector from ANY source (explicit, saved, or a
        // degenerate default), not just an empty explicit selector.
        if logical_model.raw().is_empty() {
            return Err(RouteError::EmptyModel);
        }

        // 3. `auto` is an opt-in sentinel: resolve to the provider default wire
        //    id without treating "auto" as a literal model name.
        let is_auto = logical_model.is_auto();

        // 4. Map the selector to a wire id within provider scope.
        //    Prefixed selectors are preserved VERBATIM as the wire id.
        let class = if request_uses_custom_endpoint(&descriptor, req.base_url_override.as_deref()) {
            ProviderClass::LocalOrCustom
        } else {
            classify(provider_kind)
        };
        let (wire_model_id, canonical_model, endpoint_key, limits, pricing) = if is_auto {
            default_offering.map_or_else(
                || {
                    (
                        descriptor.default_wire_model(),
                        None,
                        "chat".to_string(),
                        RouteLimits::default(),
                        // No offering in hand on the default branch: pricing is
                        // honestly unknown (#3085), never a fabricated zero.
                        PricingSku::UnknownOrStale,
                    )
                },
                |offering| {
                    (
                        offering.wire_model_id.clone(),
                        offering.canonical_model.clone(),
                        offering.endpoint_key.clone(),
                        offering.limits,
                        // Matched offering: carry its sourced pricing meter.
                        offering.pricing.clone(),
                    )
                },
            )
        } else {
            self.scope_selector(provider_kind, &provider_id, &logical_model, class)?
        };

        let endpoint = ResolvedEndpoint {
            base_url: req
                .base_url_override
                .clone()
                .unwrap_or_else(|| descriptor.default_base_url().to_string()),
            endpoint_key,
            protocol: descriptor.protocol(),
        };

        // Advisory validation (#1519): a non-loopback `http://` endpoint sends
        // credentials in plaintext. This is advisory, not a hard fail, so
        // `ok` stays true and local `http://localhost` runtimes (Ollama / vLLM /
        // SGLang defaults) stay clean.
        let mut messages = Vec::new();
        if endpoint_uses_insecure_http(&endpoint.base_url) {
            messages
                .push("endpoint uses insecure http:// (credentials sent in plaintext)".to_string());
        }
        let validation = ValidationReport { ok: true, messages };

        Ok(ReadyRouteCandidate::new(
            provider_id,
            provider_kind,
            logical_model,
            canonical_model,
            wire_model_id,
            endpoint,
            ResolvedAuthSource::Missing,
            descriptor.protocol(),
            limits,
            // #3085: honest pricing projected from the matched offering (the
            // catalog layer maps sourced cost → SKU); `UnknownOrStale` whenever
            // no offering was matched or the offering carried no price.
            Some(pricing),
            validation,
        ))
    }

    /// Interpret a concrete (non-auto) selector strictly within provider scope.
    fn scope_selector(
        &self,
        provider_kind: ProviderKind,
        provider_id: &ProviderId,
        logical_model: &LogicalModelRef,
        class: ProviderClass,
    ) -> Result<
        (
            WireModelId,
            Option<ModelId>,
            String,
            RouteLimits,
            PricingSku,
        ),
        RouteError,
    > {
        let raw = provider_scoped_wire_alias(provider_kind, logical_model.raw(), class);

        // Try to match a catalog offering owned by THIS provider, either by
        // canonical model id or by exact wire id. This keeps interpretation
        // inside provider scope; offerings from other providers are ignored.
        for offering in &self.offerings {
            if offering.provider != *provider_id {
                continue;
            }
            let matches_canonical = offering
                .canonical_model
                .as_ref()
                .is_some_and(|m| m.as_str() == raw);
            let matches_wire = offering.wire_model_id.as_str() == raw;
            if matches_canonical || matches_wire {
                return Ok((
                    offering.wire_model_id.clone(),
                    offering.canonical_model.clone(),
                    offering.endpoint_key.clone(),
                    offering.limits,
                    // Matched offering: carry its sourced pricing meter (#3085).
                    offering.pricing.clone(),
                ));
            }
        }

        // No catalog match. Apply class-specific pass-through rules.
        match class {
            ProviderClass::StrictDirect => {
                if self.selector_matches_other_provider_offering(provider_id, raw) {
                    return Err(RouteError::ForeignModelForDirectProvider {
                        provider: provider_id.clone(),
                        model: raw.to_string(),
                    });
                }
                // A clearly-foreign selector for a strict direct provider is
                // rejected. "Clearly foreign" = it carries an aggregator/org
                // namespace prefix, which a direct provider never expects.
                if logical_model.namespace_hint().is_some() {
                    return Err(RouteError::ForeignModelForDirectProvider {
                        provider: provider_id.clone(),
                        model: raw.to_string(),
                    });
                }
                // A bare, unknown model on a strict direct provider is passed
                // through verbatim (the provider validates it server-side). No
                // offering matched, so pricing is honestly unknown (#3085).
                Ok((
                    WireModelId::from(raw),
                    None,
                    "chat".to_string(),
                    RouteLimits::default(),
                    PricingSku::UnknownOrStale,
                ))
            }
            // Aggregators, local runtimes, and custom OpenAI-compatible
            // endpoints legitimately accept arbitrary / prefixed ids verbatim.
            ProviderClass::Aggregator | ProviderClass::LocalOrCustom => {
                let _ = provider_kind;
                // No offering matched: pricing is honestly unknown (#3085).
                Ok((
                    WireModelId::from(raw),
                    None,
                    "chat".to_string(),
                    RouteLimits::default(),
                    PricingSku::UnknownOrStale,
                ))
            }
        }
    }

    fn default_offering(&self, provider_id: &ProviderId) -> Option<&ProviderModelOffering> {
        self.offerings
            .iter()
            .find(|offering| offering.provider == *provider_id && offering.default_for_provider)
    }

    /// True when `raw` names an offering that lives on a *different* provider.
    ///
    /// The `wire_model_id` arm catches the common case (a bare id another
    /// provider serves). The `canonical_model` arm covers catalog rows whose
    /// canonical id is slash-free: Models.dev canonical ids normally contain a
    /// namespace (`zhipuai/glm-5.2`) and are already caught by the
    /// `namespace_hint()` guard at the call site, but a bare canonical id (or a
    /// hand-authored offering) would slip through wire-id matching alone. It is
    /// kept deliberately so a bare canonical selector cannot masquerade as a
    /// pass-through model on the wrong provider.
    fn selector_matches_other_provider_offering(
        &self,
        provider_id: &ProviderId,
        raw: &str,
    ) -> bool {
        self.offerings.iter().any(|offering| {
            offering.provider != *provider_id
                && (offering.wire_model_id.as_str() == raw
                    || offering
                        .canonical_model
                        .as_ref()
                        .is_some_and(|model| model.as_str() == raw))
        })
    }
}

/// Normalize aliases whose provider wire identity is publicly documented but
/// intentionally absent from the offline offering catalog. Keeping this seam
/// provider-scoped avoids claiming unverified limits or pricing while ensuring
/// receipts and HTTP requests carry the exact upstream model id.
fn provider_scoped_wire_alias(
    provider_kind: ProviderKind,
    raw: &str,
    class: ProviderClass,
) -> &str {
    if class != ProviderClass::LocalOrCustom {
        if provider_kind == ProviderKind::Together
            && (raw.eq_ignore_ascii_case("inkling") || raw.eq_ignore_ascii_case("together-inkling"))
        {
            return "thinkingmachines/inkling";
        }
        if provider_kind == ProviderKind::Openrouter
            && (raw.eq_ignore_ascii_case("qwen3.7-plus")
                || raw.eq_ignore_ascii_case("qwen-3.7-plus"))
        {
            return "qwen/qwen3.7-plus";
        }
    }
    raw
}

/// Build the default resolver offerings from the bundled Models.dev asset.
///
/// [`bundled_offerings`] is an empty override seam (#4139): when it later gains
/// curated rows again, those win a `(provider, wire id)` collision over the
/// asset. Today the asset is the sole bundled source of truth.
fn default_offerings() -> Vec<ProviderModelOffering> {
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut out = Vec::new();
    let asset_rows = bundled_catalog_offerings()
        .iter()
        .map(CatalogOffering::to_offering)
        .collect::<Vec<_>>();
    // Seam first so it wins identity collisions, then asset-only rows follow.
    for offering in bundled_offerings().into_iter().chain(asset_rows) {
        let key = (
            offering.provider.as_str().to_string(),
            offering.wire_model_id.as_str().to_string(),
        );
        if seen.insert(key) {
            out.push(offering);
        }
    }
    out
}

/// The resolver's minimal route classification.
///
/// Intentionally narrower than tui's `validate_route`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderClass {
    /// Strict direct provider: rejects clearly-foreign (prefixed) selectors.
    StrictDirect,
    /// Aggregator: serves many catalogs under prefixed wire ids.
    Aggregator,
    /// Local runtime or custom OpenAI-compatible endpoint: pass-through.
    LocalOrCustom,
}

/// Classify a provider kind for resolver pass-through rules.
///
/// Only a SMALL set of providers are strict-direct. Everything else passes
/// through, so the resolver stays permissive by default.
fn classify(kind: ProviderKind) -> ProviderClass {
    match kind {
        // Strict first-party direct providers.
        ProviderKind::Deepseek | ProviderKind::Zai => ProviderClass::StrictDirect,
        // Local runtimes / custom OpenAI-compatible endpoints.
        ProviderKind::Ollama | ProviderKind::Vllm | ProviderKind::Sglang | ProviderKind::Openai => {
            ProviderClass::LocalOrCustom
        }
        // Everything else is treated as an aggregator-style pass-through.
        _ => ProviderClass::Aggregator,
    }
}

fn request_uses_custom_endpoint(
    descriptor: &ProviderDescriptor,
    base_url_override: Option<&str>,
) -> bool {
    base_url_override
        .is_some_and(|base_url| provider_preserves_custom_base_url_model(descriptor.kind, base_url))
}

/// True when `base_url` is an `http://` endpoint whose host is NOT loopback
/// (#1519). Such an endpoint sends credentials in plaintext over the network;
/// loopback (`localhost` / `127.0.0.1` / `::1`) is exempt because local
/// runtimes (Ollama / vLLM / SGLang) default to plain `http://localhost`.
fn endpoint_uses_insecure_http(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    // Scheme match is case-insensitive but must be `http`, not `https`.
    let Some(rest) = strip_http_scheme(trimmed) else {
        return false;
    };
    !is_loopback_host(host_of_authority(rest))
}

/// Strip a leading case-insensitive `http://` scheme, returning the remainder.
/// Returns `None` for any other scheme (including `https://`) or no scheme.
fn strip_http_scheme(base_url: &str) -> Option<&str> {
    let idx = base_url.find("://")?;
    let (scheme, rest) = base_url.split_at(idx);
    if scheme.eq_ignore_ascii_case("http") {
        Some(&rest[3..])
    } else {
        None
    }
}

/// Extract the bare host from an authority+path string: take the authority up
/// to the first `/`, drop any `user@` userinfo and `:port` suffix, and unwrap
/// `[..]` IPv6 brackets.
fn host_of_authority(rest: &str) -> &str {
    let authority = rest.split('/').next().unwrap_or(rest);
    // Drop userinfo (`user:pass@host`) if present.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(inner) = authority.strip_prefix('[') {
        // Bracketed IPv6 literal: host is everything up to the closing bracket.
        return inner.split(']').next().unwrap_or(inner);
    }
    // Otherwise strip a trailing `:port`.
    authority.split(':').next().unwrap_or(authority)
}

/// Whether `host` is an IPv4/IPv6/name loopback address.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_matches(|c| c == '[' || c == ']');
    host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        // Any 127.0.0.0/8 address is loopback.
        || host
            .strip_prefix("127.")
            .is_some_and(|_| host.split('.').count() == 4)
}
