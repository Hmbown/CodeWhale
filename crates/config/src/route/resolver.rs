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
//!    against [`bundled_offerings`] plus the provider default. Prefixed
//!    selectors are preserved verbatim as the [`WireModelId`].
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
use super::offering::bundled_offerings;
use crate::ProviderKind;

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
#[derive(Debug, Clone, Default)]
pub struct RouteResolver;

impl RouteResolver {
    /// Construct a resolver.
    #[must_use]
    pub fn new() -> Self {
        Self
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

        // 2. Determine the logical selector + reject empties up front.
        let logical_model = match &req.model_selector {
            Some(selector) => {
                if selector.raw().is_empty() {
                    return Err(RouteError::EmptyModel);
                }
                selector.clone()
            }
            None => {
                // No selector: fall back to saved wire model, then provider
                // default. Both stay in the resolved provider's scope.
                let raw = req
                    .saved_provider_model
                    .as_ref()
                    .map(|w| w.as_str().to_string())
                    .unwrap_or_else(|| descriptor.default_wire_model().as_str().to_string());
                LogicalModelRef::from(raw)
            }
        };

        // 3. `auto` is an opt-in sentinel: resolve to the provider default wire
        //    id without treating "auto" as a literal model name.
        let is_auto = logical_model.is_auto();

        // 4. Map the selector to a wire id within provider scope.
        //    Prefixed selectors are preserved VERBATIM as the wire id.
        let class = classify(provider_kind);
        let (wire_model_id, canonical_model) = if is_auto {
            (descriptor.default_wire_model(), None)
        } else {
            self.scope_selector(provider_kind, &provider_id, &logical_model, class)?
        };

        let endpoint = ResolvedEndpoint {
            base_url: req
                .base_url_override
                .clone()
                .unwrap_or_else(|| descriptor.default_base_url().to_string()),
            endpoint_key: "chat".to_string(),
            protocol: descriptor.protocol(),
        };

        let validation = ValidationReport {
            ok: true,
            messages: Vec::new(),
        };

        Ok(ReadyRouteCandidate::new(
            provider_id,
            provider_kind,
            logical_model,
            canonical_model,
            wire_model_id,
            endpoint,
            ResolvedAuthSource::Missing,
            descriptor.protocol(),
            Some(PricingSku::UnknownOrStale),
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
    ) -> Result<(WireModelId, Option<ModelId>), RouteError> {
        let raw = logical_model.raw();

        // Try to match a bundled offering owned by THIS provider, either by
        // canonical model id or by exact wire id. This keeps interpretation
        // inside provider scope; offerings from other providers are ignored.
        for offering in bundled_offerings() {
            if offering.provider != *provider_id {
                continue;
            }
            let matches_canonical = offering
                .canonical_model
                .as_ref()
                .is_some_and(|m| m.as_str() == raw);
            let matches_wire = offering.wire_model_id.as_str() == raw;
            if matches_canonical || matches_wire {
                return Ok((offering.wire_model_id, offering.canonical_model));
            }
        }

        // No catalog match. Apply class-specific pass-through rules.
        match class {
            ProviderClass::StrictDirect => {
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
                // through verbatim (the provider validates it server-side).
                Ok((WireModelId::from(raw), None))
            }
            // Aggregators, local runtimes, and custom OpenAI-compatible
            // endpoints legitimately accept arbitrary / prefixed ids verbatim.
            ProviderClass::Aggregator | ProviderClass::LocalOrCustom => {
                let _ = provider_kind;
                Ok((WireModelId::from(raw), None))
            }
        }
    }
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
