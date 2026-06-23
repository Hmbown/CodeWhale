//! The runtime-resolved executable route (#3384).
//!
//! A [`ReadyRouteCandidate`] is the concrete form of the #2608 contract:
//!
//! > Execution requires a `ReadyRouteCandidate`.
//! > A `ReadyRouteCandidate` can only be produced by `RouteResolver`.
//!
//! Fields are pub-*read*, but the type cannot be *constructed* outside this
//! crate: the struct is `#[non_exhaustive]` (no other crate can build it via a
//! struct literal) and deliberately does not derive `Deserialize` (so it cannot
//! be fabricated from JSON either). The only constructor is
//! [`ReadyRouteCandidate::new`]
//! (`pub(super)`), and [`super::resolver::RouteResolver::resolve`] is its sole
//! caller. A candidate's existence is therefore proof it passed the resolver.
//!
//! DEFERRED: #3384's full sketch also carried `capabilities: CapabilityProfile`
//! and `config_snapshot: Config`. Both are intentionally omitted here: pulling
//! `CapabilityProfile` into `crates/config` would force a `tui -> config` type
//! move, and embedding `Config` would couple the candidate to the full config
//! model. They will be added when those types have a home in this crate.

use serde::{Deserialize, Serialize};

use super::RequestProtocol;
use super::ids::{LogicalModelRef, ModelId, ProviderId, WireModelId};
use crate::ProviderKind;

/// A concrete, resolved endpoint the route will talk to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEndpoint {
    /// Resolved base URL (after any override).
    pub base_url: String,
    /// Endpoint key (e.g. `"chat"`, `"responses"`).
    pub endpoint_key: String,
    /// Wire protocol spoken at this endpoint.
    pub protocol: RequestProtocol,
}

/// The CLASS of auth source resolved for the route.
///
/// This records only *where* a credential comes from, never the credential
/// value itself. There is intentionally no field that could hold a secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedAuthSource {
    /// Supplied via CLI flag/argument.
    Cli,
    /// Read from a config file.
    ConfigFile,
    /// Read from the OS keyring.
    Keyring,
    /// Read from an environment variable.
    Env,
    /// Produced by running a command.
    Command,
    /// Resolved from a named secret.
    Secret,
    /// No credential resolved.
    Missing,
}

/// Pricing/quota class for the resolved route.
///
/// Carries only coarse, non-sensitive shape; never secrets or account ids.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PricingSku {
    /// Per-token pricing.
    Token {
        /// Input price per million tokens, if known.
        input_per_mtok: Option<f64>,
        /// Output price per million tokens, if known.
        output_per_mtok: Option<f64>,
    },
    /// Subscription quota usage.
    SubscriptionQuota {
        /// Percent of quota used, if known.
        used_pct: Option<f32>,
        /// When the quota resets, if known.
        resets_at: Option<String>,
    },
    /// Prepaid account credits.
    AccountCredits {
        /// Remaining balance, if known.
        balance: Option<f64>,
    },
    /// Local or otherwise not billed.
    LocalOrNotApplicable,
    /// Pricing unknown or stale.
    UnknownOrStale,
}

/// Outcome of route validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationReport {
    /// Whether the route passed validation.
    pub ok: bool,
    /// Human-readable diagnostics (advisory; secret-free).
    pub messages: Vec<String>,
}

/// A runtime-resolved, executable route.
///
/// Fields are read-only to callers; the type cannot be constructed outside this
/// crate (`#[non_exhaustive]` + no `Deserialize`). The only constructor is
/// [`Self::new`], which is `pub(super)`; see module docs.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct ReadyRouteCandidate {
    /// Resolved provider id.
    pub provider_id: ProviderId,
    /// Resolved provider kind.
    pub provider_kind: ProviderKind,
    /// The selector the user/route requested.
    pub logical_model: LogicalModelRef,
    /// Canonical model identity, if one was resolved.
    pub canonical_model: Option<ModelId>,
    /// Provider-owned wire id put on the request.
    pub wire_model_id: WireModelId,
    /// Resolved endpoint transport facts.
    pub endpoint: ResolvedEndpoint,
    /// Resolved auth source CLASS (never a secret value).
    pub auth: ResolvedAuthSource,
    /// Selected wire protocol.
    pub protocol: RequestProtocol,
    /// Pricing/quota class, if known.
    pub pricing: Option<PricingSku>,
    /// Validation outcome.
    pub validation: ValidationReport,
}

impl ReadyRouteCandidate {
    /// Mint a candidate. Restricted to [`super::resolver`] so the resolver is
    /// the sole producer of executable routes (the #2608 mutation gate).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        provider_id: ProviderId,
        provider_kind: ProviderKind,
        logical_model: LogicalModelRef,
        canonical_model: Option<ModelId>,
        wire_model_id: WireModelId,
        endpoint: ResolvedEndpoint,
        auth: ResolvedAuthSource,
        protocol: RequestProtocol,
        pricing: Option<PricingSku>,
        validation: ValidationReport,
    ) -> Self {
        Self {
            provider_id,
            provider_kind,
            logical_model,
            canonical_model,
            wire_model_id,
            endpoint,
            auth,
            protocol,
            pricing,
            validation,
        }
    }
}
