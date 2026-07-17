//! Route resolution errors (#3384).
//!
//! `thiserror` is not a dependency of this crate, so [`Display`] and
//! [`std::error::Error`] are hand-implemented. No new dependency is added.

use std::fmt;

use super::ids::ProviderId;

/// Why a [`super::resolver::RouteResolver`] could not produce a candidate.
#[derive(Debug, Clone)]
pub enum RouteError {
    /// The requested model selector was empty.
    EmptyModel,
    /// The named provider could not be resolved.
    InvalidProvider(String),
    /// A model matched multiple providers; the caller must disambiguate.
    AmbiguousModel(Vec<ProviderId>),
    /// A clearly-foreign model was requested for a strict direct provider.
    ForeignModelForDirectProvider {
        /// The strict direct provider that rejected the model.
        provider: ProviderId,
        /// The foreign model selector that was rejected.
        model: String,
    },
    /// A model-aware provider did not prove a supported request protocol for
    /// the selected model/endpoint.
    UnsupportedModelProtocol {
        /// Provider whose catalog row was incomplete or unsupported.
        provider: ProviderId,
        /// Selected provider-owned model id.
        model: String,
        /// Catalog endpoint key, when one was present.
        endpoint_key: String,
    },
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyModel => write!(f, "model selector was empty"),
            Self::InvalidProvider(name) => write!(f, "invalid provider: {name}"),
            Self::AmbiguousModel(providers) => {
                let names: Vec<&str> = providers.iter().map(ProviderId::as_str).collect();
                write!(
                    f,
                    "model matches multiple providers ({}); specify a provider",
                    names.join(", ")
                )
            }
            Self::ForeignModelForDirectProvider { provider, model } => write!(
                f,
                "model {model:?} is not served by direct provider {}",
                provider.as_str()
            ),
            Self::UnsupportedModelProtocol {
                provider,
                model,
                endpoint_key,
            } => write!(
                f,
                "model {model:?} on provider {} has unsupported or unproven endpoint {endpoint_key:?}",
                provider.as_str()
            ),
        }
    }
}

impl std::error::Error for RouteError {}
