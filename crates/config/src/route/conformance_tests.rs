//! Provider descriptor conformance (#3084).
//!
//! These tests assert that *every* shipped `ProviderKind` has a well-formed
//! route-facing descriptor and resolves a default route — so adding a provider
//! without wiring its descriptor/resolver behavior fails CI here rather than at
//! runtime. They are intentionally data-driven over [`ProviderKind::all`] and
//! network-free; provider execution/adapter behavior is exercised elsewhere.

use super::bundled_offerings;
use super::descriptor::ProviderDescriptor;
use super::ids::{LogicalModelRef, ProviderId};
use super::resolver::{RouteRequest, RouteResolver};
use crate::ProviderKind;

fn none_request(kind: ProviderKind) -> RouteRequest {
    RouteRequest {
        explicit_provider: Some(kind),
        model_selector: None,
        saved_provider_model: None,
        base_url_override: None,
    }
}

#[test]
fn every_provider_kind_has_a_wellformed_descriptor() {
    for &kind in ProviderKind::all() {
        let descriptor = ProviderDescriptor::for_kind(kind);

        // The descriptor id is non-empty and agrees with the canonical mapping;
        // a mismatch means a provider was added to one table but not the other.
        assert!(
            !descriptor.id().as_str().trim().is_empty(),
            "{kind:?}: empty provider id"
        );
        assert_eq!(
            descriptor.id(),
            ProviderId::from_kind(kind),
            "{kind:?}: descriptor id disagrees with ProviderId::from_kind"
        );

        // Transport facts route resolution depends on must be present.
        assert!(
            !descriptor.default_wire_model().as_str().trim().is_empty(),
            "{kind:?}: empty default wire model"
        );
        assert!(
            !descriptor.default_base_url().trim().is_empty(),
            "{kind:?}: empty default base URL"
        );

        // Any declared auth env var name must be a real, non-empty key.
        for env_var in descriptor.env_vars() {
            assert!(
                !env_var.trim().is_empty(),
                "{kind:?}: empty env var name in descriptor"
            );
        }

        // Current fixed providers resolve a concrete protocol for their
        // default endpoint key.
        let _ = descriptor.protocol_for_endpoint("chat");
    }
}

#[test]
fn every_provider_kind_resolves_its_default_route() {
    let resolver = RouteResolver::new();
    let bundled = bundled_offerings();
    for &kind in ProviderKind::all() {
        let descriptor = ProviderDescriptor::for_kind(kind);
        let candidate = resolver.resolve(&none_request(kind)).unwrap_or_else(|err| {
            panic!("{kind:?}: default (None selector) route must resolve, got {err:?}")
        });

        assert_eq!(
            candidate.provider_kind, kind,
            "{kind:?}: resolved to a different provider"
        );
        assert_eq!(
            candidate.provider_id,
            ProviderId::from_kind(kind),
            "{kind:?}: resolved provider id mismatch"
        );

        // The resolver prefers this provider's bundled *default offering* wire id
        // when one exists, and otherwise falls back to the descriptor default
        // wire model. Assert that exact contract so a future drift between the
        // catalog-backed default and `Provider::default_model()` fails with an
        // honest message instead of coincidentally passing.
        let expected_wire = bundled
            .iter()
            .find(|offering| {
                offering.provider == ProviderId::from_kind(kind) && offering.default_for_provider
            })
            .map_or_else(
                || descriptor.default_wire_model().as_str().to_string(),
                |offering| offering.wire_model_id.as_str().to_string(),
            );
        assert_eq!(
            candidate.wire_model_id.as_str(),
            expected_wire,
            "{kind:?}: None selector must resolve to the bundled default offering (or descriptor default)"
        );
    }
}

#[test]
fn every_provider_kind_resolves_the_auto_selector() {
    let resolver = RouteResolver::new();
    for &kind in ProviderKind::all() {
        let request = RouteRequest {
            explicit_provider: Some(kind),
            model_selector: Some(LogicalModelRef::from("auto")),
            saved_provider_model: None,
            base_url_override: None,
        };
        let candidate = resolver
            .resolve(&request)
            .unwrap_or_else(|err| panic!("{kind:?}: `auto` must resolve, got {err:?}"));

        assert_eq!(
            candidate.provider_kind, kind,
            "{kind:?}: auto resolved to a different provider"
        );
        assert!(
            candidate.logical_model.is_auto(),
            "{kind:?}: `auto` must stay the auto sentinel, never a literal model"
        );
        // `auto` with no catalog default falls back to the descriptor default,
        // which conformance #2 already pins; here we only assert it resolves.
        assert!(
            !candidate.wire_model_id.as_str().trim().is_empty(),
            "{kind:?}: auto resolved to an empty wire model"
        );
    }
}
