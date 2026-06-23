//! Behavior tests for the route foundation (#2608 / #3084 / #3384).

use super::RequestProtocol;
use super::descriptor::ProviderDescriptor;
use super::errors::RouteError;
use super::ids::{LogicalModelRef, ModelId, NamespaceHint, ProviderId, WireModelId};
use super::resolver::{RouteRequest, RouteResolver};
use crate::ProviderKind;

/// Build a request with only an explicit provider + a model selector string.
fn req(provider: Option<ProviderKind>, model: Option<&str>) -> RouteRequest {
    RouteRequest {
        explicit_provider: provider,
        model_selector: model.map(LogicalModelRef::from),
        saved_provider_model: None,
        base_url_override: None,
    }
}

#[test]
fn provider_id_from_kind_uses_canonical_id() {
    assert_eq!(
        ProviderId::from_kind(ProviderKind::Deepseek).as_str(),
        "deepseek"
    );
    assert_eq!(
        ProviderId::from_kind(ProviderKind::Openrouter).as_str(),
        "openrouter"
    );
}

#[test]
fn model_id_and_wire_model_id_are_distinct_types() {
    // This test asserts the values; the *type* distinction is enforced by the
    // compiler: a function taking `WireModelId` rejects a `ModelId` argument.
    let canonical = ModelId::from("deepseek-v4-pro");
    let wire = WireModelId::from("deepseek-ai/DeepSeek-V4-Pro");
    assert_eq!(canonical.as_str(), "deepseek-v4-pro");
    assert_eq!(wire.as_str(), "deepseek-ai/DeepSeek-V4-Pro");
}

#[test]
fn logical_model_ref_auto_is_sentinel() {
    assert!(LogicalModelRef::from("auto").is_auto());
    assert!(!LogicalModelRef::from("deepseek-v4-pro").is_auto());
}

#[test]
fn logical_model_ref_namespace_hint_parses_curated_prefixes() {
    let cases = [
        ("deepseek-ai/DeepSeek-V4-Pro", NamespaceHint::DeepseekAi),
        ("deepseek/deepseek-v4-pro", NamespaceHint::Deepseek),
        ("anthropic/claude-foo", NamespaceHint::Anthropic),
        ("openai/gpt-foo", NamespaceHint::Openai),
        ("qwen/qwen-foo", NamespaceHint::Qwen),
    ];
    for (raw, expected) in cases {
        assert_eq!(
            LogicalModelRef::from(raw).namespace_hint(),
            Some(expected),
            "{raw} should parse to {expected:?}"
        );
    }
    assert_eq!(LogicalModelRef::from("plain-model").namespace_hint(), None);
    assert_eq!(LogicalModelRef::from("auto").namespace_hint(), None);
}

/// By construction there is NO path from a namespace prefix to a provider.
///
/// This is enforced by the *absence* of any `From<NamespaceHint>` /
/// `From<LogicalModelRef>` for `ProviderId`. The following lines are the
/// canonical way to mint a `ProviderId` and demonstrate the only supported
/// source is an explicit `ProviderKind`, never a parsed prefix.
#[test]
fn no_namespace_hint_or_logical_ref_to_provider_id_conversion() {
    let hint = LogicalModelRef::from("deepseek-ai/DeepSeek-V4-Pro").namespace_hint();
    assert_eq!(hint, Some(NamespaceHint::DeepseekAi));
    // A ProviderId may ONLY be built from an explicit ProviderKind or string,
    // never derived from the hint above. (If a `From<NamespaceHint>` for
    // `ProviderId` were ever added, this seam would silently break #2608.)
    let provider = ProviderId::from_kind(ProviderKind::Together);
    assert_eq!(provider.as_str(), "together");
}

#[test]
fn newtypes_serialize_transparently() {
    let id = ProviderId::from("deepseek");
    assert_eq!(serde_json::to_string(&id).unwrap(), "\"deepseek\"");
    let wire = WireModelId::from("deepseek-ai/DeepSeek-V4-Pro");
    assert_eq!(
        serde_json::to_string(&wire).unwrap(),
        "\"deepseek-ai/DeepSeek-V4-Pro\""
    );
}

#[test]
fn descriptor_for_every_kind_has_nonempty_transport_facts() {
    for kind in ProviderKind::ALL {
        let d = ProviderDescriptor::for_kind(kind);
        assert!(!d.id().as_str().is_empty(), "{kind:?} id empty");
        assert!(
            !d.default_base_url().is_empty(),
            "{kind:?} default_base_url empty"
        );
        assert!(
            !d.default_wire_model().as_str().is_empty(),
            "{kind:?} default_wire_model empty"
        );
        // protocol() always yields a RequestProtocol; calling it must not panic.
        let _: RequestProtocol = d.protocol();
    }
}

#[test]
fn descriptor_protocol_matches_provider_wire() {
    for kind in ProviderKind::ALL {
        let d = ProviderDescriptor::for_kind(kind);
        assert_eq!(
            d.protocol(),
            kind.provider().wire(),
            "{kind:?} protocol must equal provider().wire()"
        );
        let expected = match kind {
            ProviderKind::OpenaiCodex => RequestProtocol::Responses,
            ProviderKind::Anthropic => RequestProtocol::AnthropicMessages,
            _ => RequestProtocol::ChatCompletions,
        };
        assert_eq!(d.protocol(), expected, "{kind:?} protocol mismatch");
    }
}

#[test]
fn resolver_explicit_provider_scoped_model_maps_to_wire_id() {
    let r = RouteResolver::new();
    let out = r
        .resolve(&req(Some(ProviderKind::Deepseek), Some("deepseek-v4-pro")))
        .expect("should resolve");
    assert_eq!(out.provider_kind, ProviderKind::Deepseek);
    assert_eq!(out.wire_model_id.as_str(), "deepseek-v4-pro");
    assert_eq!(
        out.canonical_model.as_ref().map(ModelId::as_str),
        Some("deepseek-v4-pro")
    );
}

#[test]
fn resolver_aggregator_preserves_prefixed_wire_id_without_inferring_deepseek() {
    let r = RouteResolver::new();
    let out = r
        .resolve(&req(
            Some(ProviderKind::Together),
            Some("deepseek-ai/DeepSeek-V4-Pro"),
        ))
        .expect("aggregator should resolve");
    // Provider stays Together, NOT Deepseek, despite the deepseek-ai/ prefix.
    assert_eq!(out.provider_kind, ProviderKind::Together);
    assert_ne!(out.provider_kind, ProviderKind::Deepseek);
    // Wire id preserved verbatim.
    assert_eq!(out.wire_model_id.as_str(), "deepseek-ai/DeepSeek-V4-Pro");
}

#[test]
fn resolver_openrouter_keeps_provider_for_every_namespace_prefix() {
    let r = RouteResolver::new();
    let prefixes = [
        "deepseek-ai/DeepSeek-V4-Pro",
        "deepseek/deepseek-v4-pro",
        "anthropic/claude-foo",
        "openai/gpt-foo",
        "qwen/qwen-foo",
    ];
    for raw in prefixes {
        let selector = LogicalModelRef::from(raw);
        // The selector DOES carry a namespace hint...
        assert!(
            selector.namespace_hint().is_some(),
            "{raw} should have a namespace hint"
        );
        let out = r
            .resolve(&req(Some(ProviderKind::Openrouter), Some(raw)))
            .unwrap_or_else(|e| panic!("{raw} should resolve on openrouter: {e}"));
        // ...but the provider stays Openrouter regardless.
        assert_eq!(
            out.provider_kind,
            ProviderKind::Openrouter,
            "{raw} must not change provider"
        );
        assert_eq!(out.wire_model_id.as_str(), raw, "{raw} wire id verbatim");
    }
}

#[test]
fn resolver_no_explicit_provider_does_not_infer_deepseek_from_prefix() {
    let r = RouteResolver::new();
    // explicit_provider=None => default scope (Deepseek). A prefixed selector
    // is foreign for the strict-direct default, so it ERRORS rather than being
    // silently accepted as a deepseek model: the prefix never *selects* it.
    let out = r.resolve(&req(None, Some("deepseek/deepseek-v4-pro")));
    match out {
        Err(RouteError::ForeignModelForDirectProvider { provider, model }) => {
            assert_eq!(provider.as_str(), "deepseek");
            assert_eq!(model, "deepseek/deepseek-v4-pro");
        }
        other => panic!("expected ForeignModelForDirectProvider, got {other:?}"),
    }
}

#[test]
fn resolver_auto_is_sentinel_not_literal_model() {
    let r = RouteResolver::new();
    let out = r
        .resolve(&req(Some(ProviderKind::Deepseek), Some("auto")))
        .expect("auto should resolve");
    // The logical selector is the auto sentinel...
    assert!(out.logical_model.is_auto());
    // ...and "auto" is NOT put on the wire as a literal model.
    assert_ne!(out.wire_model_id.as_str(), "auto");
    assert_eq!(out.wire_model_id.as_str(), "deepseek-v4-pro");
}

#[test]
fn resolver_strict_direct_rejects_clearly_foreign_selector() {
    let r = RouteResolver::new();
    let out = r.resolve(&req(Some(ProviderKind::Zai), Some("anthropic/claude-foo")));
    match out {
        Err(RouteError::ForeignModelForDirectProvider { provider, model }) => {
            assert_eq!(provider.as_str(), "zai");
            assert_eq!(model, "anthropic/claude-foo");
        }
        other => panic!("expected ForeignModelForDirectProvider, got {other:?}"),
    }
}

#[test]
fn resolver_deepseek_none_selector_uses_default_wire_id() {
    let r = RouteResolver::new();
    let out = r
        .resolve(&req(Some(ProviderKind::Deepseek), None))
        .expect("none selector should use provider default");
    assert_eq!(out.provider_kind, ProviderKind::Deepseek);
    assert_eq!(out.wire_model_id.as_str(), "deepseek-v4-pro");
}

#[test]
fn resolver_empty_string_selector_is_empty_model_error() {
    let r = RouteResolver::new();
    let out = r.resolve(&req(Some(ProviderKind::Deepseek), Some("")));
    assert!(matches!(out, Err(RouteError::EmptyModel)));
}

#[test]
fn resolver_empty_saved_provider_model_is_empty_model_error() {
    // An empty selector from the saved-model fallback must be rejected too, not
    // just an empty explicit selector (the guard covers every selector source).
    let r = RouteResolver::new();
    let request = RouteRequest {
        explicit_provider: Some(ProviderKind::Deepseek),
        model_selector: None,
        saved_provider_model: Some(WireModelId::from("")),
        base_url_override: None,
    };
    assert!(matches!(r.resolve(&request), Err(RouteError::EmptyModel)));
}

#[test]
fn resolver_passthrough_provider_preserves_custom_id_verbatim() {
    let r = RouteResolver::new();
    let out = r
        .resolve(&req(Some(ProviderKind::Ollama), Some("my-local:7b")))
        .expect("local passthrough should resolve");
    assert_eq!(out.provider_kind, ProviderKind::Ollama);
    assert_eq!(out.wire_model_id.as_str(), "my-local:7b");
    assert!(out.validation.ok);
}

#[test]
fn resolved_candidate_serializes_secret_free() {
    let r = RouteResolver::new();
    // Cover a direct, an aggregator, and a local/passthrough route.
    let candidates = [
        r.resolve(&req(Some(ProviderKind::Deepseek), Some("deepseek-v4-pro")))
            .expect("direct resolves"),
        r.resolve(&req(
            Some(ProviderKind::Together),
            Some("deepseek-ai/DeepSeek-V4-Pro"),
        ))
        .expect("aggregator resolves"),
        r.resolve(&req(Some(ProviderKind::Ollama), Some("my-local:7b")))
            .expect("local resolves"),
    ];
    for out in candidates {
        let json = serde_json::to_string(&out).expect("candidate serializes");
        // Carries provider/model/wire/protocol/auth-source class.
        assert!(json.contains("provider_id"), "{json}");
        assert!(json.contains("provider_kind"), "{json}");
        assert!(json.contains("wire_model_id"), "{json}");
        assert!(json.contains("protocol"), "{json}");
        assert!(json.contains("auth"), "{json}");
        // Never any secret/api-key material.
        let lower = json.to_lowercase();
        assert!(!lower.contains("api_key"), "leaked api_key: {json}");
        assert!(!lower.contains("apikey"), "leaked apikey: {json}");
        assert!(!lower.contains("secret_id"), "leaked secret_id: {json}");
        assert!(!lower.contains("password"), "leaked password: {json}");
        assert!(!lower.contains("bearer"), "leaked bearer: {json}");
        assert!(
            !lower.contains("authorization"),
            "leaked authorization: {json}"
        );
    }
}

#[test]
fn resolver_protocol_matches_descriptor_for_every_provider() {
    let r = RouteResolver::new();
    for kind in ProviderKind::ALL {
        // Use each provider's own default wire id as the selector so strict
        // direct providers do not reject; this exercises the resolver across
        // the whole provider set.
        let default_wire = ProviderDescriptor::for_kind(kind).default_wire_model();
        let request = req(Some(kind), Some(default_wire.as_str()));
        let out = r
            .resolve(&request)
            .unwrap_or_else(|e| panic!("{kind:?} should resolve its own default: {e}"));
        assert_eq!(
            out.protocol,
            ProviderDescriptor::for_kind(kind).protocol(),
            "{kind:?} candidate protocol must match descriptor"
        );
        assert_eq!(
            out.endpoint.protocol, out.protocol,
            "{kind:?} endpoint protocol"
        );
    }
}
