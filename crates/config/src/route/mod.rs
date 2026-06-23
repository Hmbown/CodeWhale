//! Route foundation: additive, runtime-unwired types for EPIC #2608.
//!
//! This module tree introduces the canonical identity newtypes (#3084) and the
//! `ReadyRouteCandidate` / `RouteResolver` contract (#3384) without touching
//! any runtime routing path. Nothing here is consumed by `config.rs`, the TUI,
//! the client, or the engine yet; it is a self-contained seam that later
//! tracks will wire in.
//!
//! Layering:
//! - [`ids`] — provider/model/wire string newtypes + namespace hints.
//! - [`descriptor`] — route-facing view over the static provider registry.
//! - [`offering`] — provider/model offering seam (wire-id binding).
//! - [`candidate`] — the runtime-resolved executable route + its parts.
//! - [`errors`] — route resolution errors.
//! - [`resolver`] — the sole producer of [`candidate::ReadyRouteCandidate`].
//!
//! Naming: the request/response wire shape is spelled [`RequestProtocol`],
//! which is a re-export alias of [`crate::provider::WireFormat`] rather than a
//! fourth protocol synonym.

#![allow(dead_code)]

/// The selected endpoint's request/response wire shape.
///
/// Alias of [`crate::provider::WireFormat`]; intentionally NOT a new enum, to
/// avoid introducing yet another protocol synonym.
pub use crate::provider::WireFormat as RequestProtocol;

pub mod candidate;
pub mod descriptor;
pub mod errors;
pub mod ids;
pub mod offering;
pub mod resolver;

pub use candidate::{
    PricingSku, ReadyRouteCandidate, ResolvedAuthSource, ResolvedEndpoint, ValidationReport,
};
pub use descriptor::{EndpointDescriptor, ProviderDescriptor};
pub use errors::RouteError;
pub use ids::{LogicalModelRef, ModelId, NamespaceHint, ProviderId, WireModelId};
pub use offering::{ProviderModelOffering, bundled_offerings};
pub use resolver::{RouteRequest, RouteResolver};

#[cfg(test)]
mod conformance_tests;
#[cfg(test)]
mod tests;
