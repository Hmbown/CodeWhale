//! Provider descriptors over the existing built-in provider registry (#3084).
//!
//! A [`ProviderDescriptor`] is a thin, route-facing view over the static
//! [`provider::Provider`] trait objects already in [`crate::provider`]. It
//! surfaces only the transport facts route resolution needs (id, base URL,
//! default wire model, env vars, protocol) without duplicating the registry.
//!
//! Because a descriptor holds a `&'static dyn Provider`, it is intentionally
//! NOT `Serialize`/`PartialEq`-derivable. Never embed a [`ProviderDescriptor`]
//! inside a `Serialize` struct; serialize the resolved facts instead.

use crate::ProviderKind;
use crate::provider::{self, Provider, WirePolicy};

use super::RequestProtocol;
use super::ids::{ProviderId, WireModelId};

/// Route-facing view of a built-in provider's transport facts.
///
/// Holds a trait object, so it is deliberately not serializable/comparable.
#[derive(Clone, Copy)]
pub struct ProviderDescriptor {
    /// The provider kind this descriptor describes.
    pub kind: ProviderKind,
    /// Backing static provider metadata entry.
    pub inner: &'static dyn Provider,
}

impl ProviderDescriptor {
    /// Build a descriptor for a known provider kind.
    #[must_use]
    pub fn for_kind(kind: ProviderKind) -> Self {
        Self {
            kind,
            inner: provider::provider_for_kind(kind),
        }
    }

    /// Canonical provider id.
    #[must_use]
    pub fn id(&self) -> ProviderId {
        ProviderId::from(self.inner.id())
    }

    /// Default base URL when no override is present.
    #[must_use]
    pub fn default_base_url(&self) -> &'static str {
        self.inner.default_base_url()
    }

    /// Default wire model id when no model is selected.
    #[must_use]
    pub fn default_wire_model(&self) -> WireModelId {
        WireModelId::from(self.inner.default_model())
    }

    /// Environment variable candidates for this provider's API key.
    #[must_use]
    pub fn env_vars(&self) -> &'static [&'static str] {
        self.inner.env_vars()
    }

    /// Policy used to select this provider's wire protocol.
    #[must_use]
    pub fn wire_policy(&self) -> WirePolicy {
        self.inner.wire_policy()
    }

    /// Resolve the concrete protocol for an offering endpoint key.
    #[must_use]
    pub fn protocol_for_endpoint(&self, endpoint_key: &str) -> Option<RequestProtocol> {
        self.wire_policy().resolve(endpoint_key)
    }
}

impl std::fmt::Debug for ProviderDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderDescriptor")
            .field("kind", &self.kind)
            .field("id", &self.inner.id())
            .field("wire_policy", &self.inner.wire_policy())
            .finish()
    }
}

/// A concrete endpoint's transport facts.
///
/// Unlike [`ProviderDescriptor`], this owns plain data and is safe to embed in
/// serializable route output (see [`super::candidate::ResolvedEndpoint`]).
#[derive(Debug, Clone)]
pub struct EndpointDescriptor {
    /// Stable endpoint key (e.g. `"chat"`, `"responses"`).
    pub endpoint_key: String,
    /// Wire protocol spoken at this endpoint.
    pub protocol: RequestProtocol,
    /// Default base URL for this endpoint.
    pub default_base_url: String,
    /// Whether streaming is supported.
    pub streaming: bool,
}
