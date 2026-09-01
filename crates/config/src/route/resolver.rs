//! The sole producer of [`ReadyRouteCandidate`] (#3384).
//!
//! [`RouteResolver::resolve`] is the ONLY caller of
//! `ReadyRouteCandidate::new`. It resolves a [`RouteRequest`] into an
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
    LimitField, PricingSku, ReadyRouteCandidate, ResolvedAuthSource, ResolvedEndpoint,
    SourcedLimitOverride, ValidationReport,
};
use super::capabilities::{
    RouteCapabilities, documented_moonshot_web_search_for_route,
    documented_zai_web_search_for_route,
};
use super::descriptor::ProviderDescriptor;
use super::errors::RouteError;
use super::ids::{LogicalModelRef, ModelId, ProviderId, WireModelId};
use super::offering::{ProviderModelOffering, RouteLimits, bundled_offerings};
use crate::catalog::{CatalogOffering, bundled_catalog_offerings};
use crate::provider::WirePolicy;
use crate::{ProviderKind, opencode_go_chat_model_id, provider_preserves_custom_base_url_model};

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
    /// Sourced limit overrides, applied in order BEFORE the candidate is
    /// constructed and recorded on it as provenance. This is the ONLY channel
    /// for adjusting a route's effective limits: the candidate itself is
    /// immutable once minted.
    pub limit_overrides: Vec<SourcedLimitOverride>,
}

/// Resolves [`RouteRequest`]s into [`ReadyRouteCandidate`]s.
#[derive(Debug, Clone)]
pub struct RouteResolver {
    offerings: Vec<ProviderModelOffering>,
}

/// Offering-owned facts selected within one provider scope before the final
/// executable route candidate is minted.
struct ResolvedOffering {
    wire_model_id: WireModelId,
    canonical_model: Option<ModelId>,
    endpoint_key: String,
    limits: RouteLimits,
    capabilities: RouteCapabilities,
    pricing: PricingSku,
}

impl ResolvedOffering {
    fn unknown(wire_model_id: WireModelId) -> Self {
        Self {
            wire_model_id,
            canonical_model: None,
            endpoint_key: "chat".to_string(),
            limits: RouteLimits::default(),
            capabilities: RouteCapabilities::default(),
            pricing: PricingSku::UnknownOrStale,
        }
    }

    fn from_offering(offering: &ProviderModelOffering) -> Self {
        Self {
            wire_model_id: offering.wire_model_id.clone(),
            canonical_model: offering.canonical_model.clone(),
            endpoint_key: offering.endpoint_key.clone(),
            limits: offering.limits,
            capabilities: offering.capabilities,
            pricing: offering.pricing.clone(),
        }
    }
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
        let custom_endpoint =
            request_uses_custom_endpoint(&descriptor, req.base_url_override.as_deref());
        let class = if custom_endpoint {
            ProviderClass::LocalOrCustom
        } else {
            classify(provider_kind)
        };
        let model_aware = descriptor.wire_policy() == WirePolicy::ModelAware;
        // A model-aware protocol row is an exact provider-endpoint fact.
        // OpenCode Zen's published roster is closed; DeepSeek's direct route
        // deliberately preserves its existing future-model pass-through and
        // sends unknown bare ids over Chat until an exact Responses row exists.
        // A custom DeepSeek-compatible endpoint retains that pass-through, but
        // a custom URL must not weaken another model-aware provider's closed
        // protocol roster.
        let require_catalog_match = model_aware && provider_kind != ProviderKind::Deepseek;
        let mut selected = if is_auto {
            match default_offering {
                None if require_catalog_match => {
                    return Err(RouteError::UnsupportedModelProtocol {
                        provider: provider_id.clone(),
                        model: descriptor.default_wire_model().as_str().to_string(),
                        endpoint_key: "unproven".to_string(),
                    });
                }
                None => ResolvedOffering::unknown(descriptor.default_wire_model()),
                Some(offering) => ResolvedOffering::from_offering(offering),
            }
        } else {
            self.scope_selector(
                provider_kind,
                &provider_id,
                &logical_model,
                class,
                require_catalog_match,
            )?
        };
        if provider_kind == ProviderKind::Deepseek {
            if custom_endpoint {
                selected.endpoint_key = "chat".to_string();
            } else if selected.canonical_model.is_none()
                && deepseek_versioned_model_prefers_responses(selected.wire_model_id.as_str())
            {
                // DeepSeek introduced its native agent wire on V4 Flash. Keep
                // exact catalog rows authoritative (notably V4 Pro => Chat),
                // while allowing future versioned direct models to adopt the
                // new Responses surface without a Codewhale release.
                selected.endpoint_key = "responses".to_string();
            }
        }
        if custom_endpoint {
            // Capabilities and pricing belong to the exact provider endpoint
            // offering that reported them. Reusing a provider enum and a
            // first-party model id against a custom compatible endpoint does
            // not prove that proxy serves the same modality, tool, reasoning,
            // or billing contract. Keep the caller's model id and Chat
            // pass-through above, but clear every unowned offering fact at the
            // authority boundary instead of presenting it as verified.
            selected.capabilities = RouteCapabilities::default();
            selected.pricing = PricingSku::UnknownOrStale;
        }
        if provider_kind == ProviderKind::Zai {
            let effective_base_url = req
                .base_url_override
                .as_deref()
                .unwrap_or_else(|| descriptor.default_base_url());
            selected.capabilities.server_side_web_search = documented_zai_web_search_for_route(
                provider_kind,
                selected.wire_model_id.as_str(),
                effective_base_url,
            );
        }
        if provider_kind == ProviderKind::Moonshot {
            let effective_base_url = req
                .base_url_override
                .as_deref()
                .unwrap_or_else(|| descriptor.default_base_url());
            selected.capabilities.server_side_web_search = documented_moonshot_web_search_for_route(
                provider_kind,
                selected.wire_model_id.as_str(),
                effective_base_url,
            );
        }

        let protocol = descriptor
            .protocol_for_endpoint(&selected.endpoint_key)
            .ok_or_else(|| RouteError::UnsupportedModelProtocol {
                provider: provider_id.clone(),
                model: selected.wire_model_id.as_str().to_string(),
                endpoint_key: selected.endpoint_key.clone(),
            })?;
        let endpoint = ResolvedEndpoint {
            base_url: req
                .base_url_override
                .clone()
                .unwrap_or_else(|| descriptor.default_base_url().to_string()),
            endpoint_key: selected.endpoint_key,
            protocol,
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

        // Apply caller-requested limit overrides in order, BEFORE the candidate
        // is minted. The candidate is immutable afterwards; the applied
        // overrides are recorded on it as provenance.
        let mut limits = selected.limits;
        for limit_override in &req.limit_overrides {
            match limit_override.field {
                LimitField::ContextTokens => limits.context_tokens = limit_override.value,
                LimitField::InputTokens => limits.input_tokens = limit_override.value,
                LimitField::OutputTokens => limits.output_tokens = limit_override.value,
            }
        }

        Ok(ReadyRouteCandidate::new(
            provider_id,
            provider_kind,
            logical_model,
            selected.canonical_model,
            selected.wire_model_id,
            endpoint,
            // The resolver never inspects credentials: auth is honestly
            // `Unresolved` at resolution time, not a claimed `Missing`.
            ResolvedAuthSource::Unresolved,
            protocol,
            limits,
            selected.capabilities,
            // #3085: honest pricing projected from the matched offering (the
            // catalog layer maps sourced cost → SKU); `UnknownOrStale` whenever
            // no offering was matched or the offering carried no price.
            Some(selected.pricing),
            validation,
            req.limit_overrides.clone(),
        ))
    }

    /// Interpret a concrete (non-auto) selector strictly within provider scope.
    fn scope_selector(
        &self,
        provider_kind: ProviderKind,
        provider_id: &ProviderId,
        logical_model: &LogicalModelRef,
        class: ProviderClass,
        require_catalog_match: bool,
    ) -> Result<ResolvedOffering, RouteError> {
        // OpenCode Go publishes one combined model roster across two wire
        // protocols. Codewhale's provider is deliberately Chat Completions
        // only, so this allowlist must sit at the sole route-candidate seam.
        // In particular, a custom base URL must not reopen generic
        // LocalOrCustom pass-through for Messages-only model ids.
        let raw = if provider_kind == ProviderKind::OpencodeGo {
            opencode_go_chat_model_id(logical_model.raw()).ok_or_else(|| {
                RouteError::ForeignModelForDirectProvider {
                    provider: provider_id.clone(),
                    model: logical_model.raw().to_string(),
                }
            })?
        } else if provider_kind == ProviderKind::OpencodeZen {
            logical_model
                .raw()
                .strip_prefix("opencode/")
                .or_else(|| logical_model.raw().strip_prefix("opencode-zen/"))
                .unwrap_or_else(|| logical_model.raw())
        } else if provider_kind == ProviderKind::Concentrate {
            // Concentrate's own namespace is not part of its wire ids.
            // `concentrate/auto` is the explicit spelling for the gateway's
            // `auto` router — Codewhale's bare `auto` is the resolver sentinel
            // above, never a literal model id — and `concentrate/<id>` is
            // `<id>`. Upstream prefixes (`openai/gpt-5.6-sol`) stay verbatim:
            // they pin the upstream provider inside the gateway.
            logical_model
                .raw()
                .strip_prefix("concentrate/")
                .unwrap_or_else(|| logical_model.raw())
        } else {
            provider_scoped_wire_alias(provider_kind, logical_model.raw(), class)
        };

        // Try to match a catalog offering owned by THIS provider, either by
        // canonical model id or by exact wire id. This keeps interpretation
        // inside provider scope; offerings from other providers are ignored.
        // DeepSeek and Z.ai also publish marketing-cased wire ids while saved
        // selectors can be lowercase. Defer that fallback until exact matching
        // is exhausted, and only accept a unique provider-owned match so
        // catalog order can never choose between case-distinct model ids.
        let allow_casefold_wire_match = class == ProviderClass::StrictDirect
            && matches!(provider_kind, ProviderKind::Deepseek | ProviderKind::Zai);
        let mut casefold_match = None;
        let mut casefold_ambiguous = false;
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
                return Ok(ResolvedOffering::from_offering(offering));
            }
            if allow_casefold_wire_match
                && offering.wire_model_id.as_str().eq_ignore_ascii_case(raw)
            {
                if casefold_match.is_some() {
                    casefold_ambiguous = true;
                } else {
                    casefold_match = Some(offering);
                }
            }
        }
        if !casefold_ambiguous && let Some(offering) = casefold_match {
            return Ok(ResolvedOffering::from_offering(offering));
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
                if require_catalog_match {
                    return Err(RouteError::UnsupportedModelProtocol {
                        provider: provider_id.clone(),
                        model: raw.to_string(),
                        endpoint_key: "unproven".to_string(),
                    });
                }
                // A bare, unknown model on a strict direct provider is passed
                // through verbatim (the provider validates it server-side). No
                // offering matched, so pricing is honestly unknown (#3085).
                Ok(ResolvedOffering::unknown(WireModelId::from(raw)))
            }
            // Aggregators, local runtimes, and custom OpenAI-compatible
            // endpoints legitimately accept arbitrary / prefixed ids verbatim.
            ProviderClass::Aggregator | ProviderClass::LocalOrCustom => {
                if require_catalog_match {
                    // Opencode Zen serves Muse Spark exclusively over Responses.
                    // Handle any future muse-spark variant (e.g. -free suffix)
                    // even when no exact bundled offering exists — fail open to
                    // responses rather than failing closed to "unproven".
                    if provider_kind == ProviderKind::OpencodeZen
                        && raw.to_ascii_lowercase().contains("muse-spark")
                    {
                        return Ok(ResolvedOffering {
                            wire_model_id: WireModelId::from(raw),
                            canonical_model: None,
                            endpoint_key: "responses".to_string(),
                            limits: RouteLimits::default(),
                            capabilities: RouteCapabilities::default(),
                            pricing: PricingSku::UnknownOrStale,
                        });
                    }
                    return Err(RouteError::UnsupportedModelProtocol {
                        provider: provider_id.clone(),
                        model: raw.to_string(),
                        endpoint_key: "unproven".to_string(),
                    });
                }
                // No offering matched: pricing is honestly unknown (#3085).
                Ok(ResolvedOffering::unknown(WireModelId::from(raw)))
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
/// Curated transport rows win a `(provider, wire id)` collision over the asset;
/// all other offerings continue to come from Models.dev.
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

fn deepseek_versioned_model_prefers_responses(model: &str) -> bool {
    model
        .trim()
        .to_ascii_lowercase()
        .strip_prefix("deepseek-v")
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|first| first.is_ascii_digit())
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
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // Parse real addresses rather than pattern-matching a `127.` prefix: the
    // old `strip_prefix("127.") && 4 dot-parts` check classified
    // `127.evil.example.com` as loopback (2026-08-04 review), which would let
    // a hostile hostname inherit local-trust routing. `Ipv4Addr::is_loopback`
    // is exactly the 127.0.0.0/8 block; `Ipv6Addr::is_loopback` is `::1`.
    if let Ok(v4) = host.parse::<std::net::Ipv4Addr>() {
        return v4.is_loopback();
    }
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        return v6.is_loopback();
    }
    false
}

#[cfg(test)]
mod loopback_tests {
    use super::{endpoint_uses_insecure_http, is_loopback_host};

    #[test]
    fn loopback_matches_only_real_loopback_addresses() {
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LocalHost"));
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.1.2.3")); // all of 127.0.0.0/8
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));

        // The 2026-08-04 regression: a hostile hostname that merely starts
        // with `127.` and has four dot-parts must NOT be trusted as local.
        assert!(!is_loopback_host("127.evil.example.com"));
        assert!(!is_loopback_host("127.0.0.1.evil.com"));
        assert!(!is_loopback_host("notlocalhost"));
        assert!(!is_loopback_host("10.0.0.1"));
        assert!(!is_loopback_host("localhost.evil.com"));
    }

    #[test]
    fn insecure_http_flags_a_hostile_127_lookalike() {
        // loopback stays exempt (local runtimes use plain http)
        assert!(!endpoint_uses_insecure_http("http://127.0.0.1:11434/v1"));
        assert!(!endpoint_uses_insecure_http("http://localhost:8000/v1"));
        // a real remote host dressed up as 127.* is insecure http
        assert!(endpoint_uses_insecure_http(
            "http://127.evil.example.com/v1"
        ));
    }
}
