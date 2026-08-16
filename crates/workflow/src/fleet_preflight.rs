//! Worker route **preflight**: everything about a route that must be true and
//! frozen *before* a Workflow starts, and certainly before any Router is
//! asked anything.
//!
//! An exact Fleet's promise is that the saved provider/model is the one that
//! runs. That promise is only worth something if it is *checked* at the point
//! the run is admitted, not discovered at the first API call:
//!
//! - **Provider identity** — the exact configured provider key and its kind.
//! - **Canonical wire model** — the model string that will actually be placed
//!   on the request. Receipt and child spawn must use *this* value, not the
//!   file's spelling of it, or the receipt describes a request nobody made.
//! - **Credential / readiness** — decided **locally**, from configuration. No
//!   live probe: a preflight that hits the network would spend money and leak
//!   the fact of the run before the operator's gates have even been evaluated.
//!   Keyless local providers (`vllm`, `ollama`, `sglang`, …) are
//!   [`CredentialReadiness::KeylessLocal`] and are perfectly valid.
//! - **Endpoint identity** — a non-secret label for *where* the request goes,
//!   so two members pointed at different deployments of the same model id are
//!   distinguishable on a receipt. Never a full URL with credentials in it.
//! - **Reasoning capability** — what the route can truthfully express, derived
//!   once here so a later launch cannot invent one.
//!
//! Everything in this module is a plain value with no clock, no filesystem, and
//! no network. The host supplies the facts; this crate defines their shape and
//! the invariants over them.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::fleet_exact::FrozenRoute;
use crate::fleet_reasoning::ReasoningCapability;

/// Whether a route can be called at all, decided from local configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CredentialReadiness {
    /// A credential for this provider is configured on this machine.
    Configured,
    /// The provider is a local, keyless endpoint. Valid, and not a downgrade.
    KeylessLocal,
    /// No credential is configured. The route cannot run.
    Missing { detail: String },
}

impl CredentialReadiness {
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Configured | Self::KeylessLocal)
    }

    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::KeylessLocal => "keyless_local",
            Self::Missing { .. } => "missing",
        }
    }
}

/// A non-secret identity for the endpoint a route talks to.
///
/// Deliberately **not** a base URL: a configured base URL can carry a token in
/// its path or query, and receipts are durable. Host plus a coarse path label is
/// enough to tell two deployments apart, which is the only thing this is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointIdentity {
    /// Host (and port, when non-default), lowercased. Never credentials.
    pub host: String,
    /// Whether the endpoint resolves to loopback / a private address.
    pub local: bool,
}

impl EndpointIdentity {
    /// Build an endpoint identity from a base URL, keeping only the host.
    ///
    /// Parsing is deliberately minimal and dependency-free: strip the scheme,
    /// drop anything before an `@` (which is exactly where a credential would
    /// live), then keep the authority up to the first `/`.
    #[must_use]
    pub fn from_base_url(base_url: &str) -> Self {
        let without_scheme = base_url
            .trim()
            .split_once("://")
            .map_or(base_url.trim(), |(_, rest)| rest);
        let authority = without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        // `user:password@host` — everything before the `@` is a credential.
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host)
            .to_ascii_lowercase();
        let bare = host.split(':').next().unwrap_or(&host);
        let local = bare == "localhost"
            || bare == "127.0.0.1"
            || bare == "::1"
            || bare.starts_with("192.168.")
            || bare.starts_with("10.")
            || bare.ends_with(".local");
        Self { host, local }
    }

    /// The compact receipt form.
    #[must_use]
    pub fn label(&self) -> String {
        if self.local {
            format!("{} (local)", self.host)
        } else {
            self.host.clone()
        }
    }
}

/// One worker's route, fully preflighted and frozen.
///
/// Constructed once, at Workflow start. A launch reads it; nothing rewrites
/// it. The `wire_model` here is the single source of truth for both the
/// receipt and the child spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightedRoute {
    /// The member this route belongs to.
    pub member_id: String,
    /// Canonical runtime provider identity recorded on receipts.
    pub provider_id: String,
    /// Runtime configuration key used to rebuild this route when compatibility
    /// migration canonicalized [`Self::provider_id`] for receipts.
    ///
    /// Absent for ordinary routes. The released Ollama Cloud shape is the
    /// current use: a saved `ollama` route is reported canonically as
    /// `ollama-cloud`, while client construction must still read the exact
    /// legacy table and credential slot. This value is non-secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_config_id: Option<String>,
    /// Provider kind (`zai`, `openai`, `deepseek`, `vllm`, …).
    pub provider_kind: String,
    /// The model id exactly as saved in the Fleet file.
    pub declared_model: String,
    /// The canonical model string that will be placed on the wire. Receipt and
    /// child spawn both use this.
    pub wire_model: String,
    /// Where the request goes.
    pub endpoint: EndpointIdentity,
    /// Locally decided readiness. Never a live probe.
    pub credential: CredentialReadiness,
    /// What the route can truthfully express about reasoning.
    pub capability: ReasoningCapability,
}

impl PreflightedRoute {
    /// Provider key the host must scope when rebuilding this frozen route.
    #[must_use]
    pub fn provider_config_id(&self) -> &str {
        self.provider_config_id
            .as_deref()
            .unwrap_or(&self.provider_id)
    }

    /// The frozen provider/model pair, in canonical wire form.
    ///
    /// This is what a receipt records and the provider identity the child
    /// ultimately runs. [`Self::provider_config_id`] may retain an older
    /// configuration selector solely so rebuilding that identity reads the
    /// correct table and credential slot.
    #[must_use]
    pub fn frozen(&self) -> FrozenRoute {
        FrozenRoute {
            provider: self.provider_id.clone(),
            model: self.wire_model.clone(),
        }
    }

    /// Whether the declared model string differed from the canonical wire form.
    /// Recorded rather than hidden: `glm-5` resolving to `glm-5-20260101` is a
    /// fact the operator should be able to see on a receipt.
    #[must_use]
    pub fn model_canonicalized(&self) -> bool {
        self.declared_model != self.wire_model
    }

    /// Fail if this route is not runnable. Called at Workflow start.
    pub fn require_ready(&self) -> Result<(), PreflightError> {
        match &self.credential {
            CredentialReadiness::Configured | CredentialReadiness::KeylessLocal => Ok(()),
            CredentialReadiness::Missing { detail } => Err(PreflightError::CredentialMissing {
                member: self.member_id.clone(),
                provider: self.provider_id.clone(),
                detail: detail.clone(),
            }),
        }
    }
}

/// A frozen preflight for every worker in a Workflow, plus the Router.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutePreflight {
    workers: Vec<PreflightedRoute>,
    router: Option<PreflightedRoute>,
}

impl RoutePreflight {
    #[must_use]
    pub fn new(workers: Vec<PreflightedRoute>, router: Option<PreflightedRoute>) -> Self {
        Self { workers, router }
    }

    #[must_use]
    pub fn workers(&self) -> &[PreflightedRoute] {
        &self.workers
    }

    #[must_use]
    pub fn router(&self) -> Option<&PreflightedRoute> {
        self.router.as_ref()
    }

    /// The frozen route for one member id.
    #[must_use]
    pub fn worker(&self, member_id: &str) -> Option<&PreflightedRoute> {
        let key = member_id.trim().to_ascii_lowercase();
        self.workers.iter().find(|route| route.member_id == key)
    }

    /// Fail unless every worker route is runnable.
    ///
    /// Called before a Workflow is allowed to start, so a member with no
    /// credential is a startup error rather than a first-task surprise.
    pub fn require_all_ready(&self) -> Result<(), PreflightError> {
        for route in &self.workers {
            route.require_ready()?;
        }
        Ok(())
    }

    /// Whether any worker route talks to a different provider than the Router.
    /// This is what a receipt discloses as cross-provider inference.
    #[must_use]
    pub fn crosses_providers(&self, member_id: &str) -> bool {
        match (self.worker(member_id), self.router()) {
            (Some(worker), Some(router)) => worker.provider_id != router.provider_id,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PreflightError {
    #[error(
        "fleet member `{member}` is pinned to provider `{provider}`, which does not resolve to a \
         configured provider: {detail}"
    )]
    ProviderUnresolved {
        member: String,
        provider: String,
        detail: String,
    },
    #[error(
        "fleet member `{member}` is pinned to model `{model}` on provider `{provider}`, which is \
         not a valid route: {detail}"
    )]
    ModelUnresolved {
        member: String,
        provider: String,
        model: String,
        detail: String,
    },
    #[error(
        "fleet member `{member}` cannot run: provider `{provider}` has no credential configured \
         on this machine ({detail}). This is decided locally — no provider was contacted. Keyless \
         local providers do not need one."
    )]
    CredentialMissing {
        member: String,
        provider: String,
        detail: String,
    },
    #[error(
        "cannot determine what reasoning control provider `{provider}` actually expresses for \
         model `{model}`: {detail}. An exact fleet fails closed here rather than claiming a \
         capability it did not verify."
    )]
    CapabilityUnknown {
        provider: String,
        model: String,
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(member: &str, provider: &str, wire: &str) -> PreflightedRoute {
        PreflightedRoute {
            member_id: member.to_string(),
            provider_id: provider.to_string(),
            provider_config_id: None,
            provider_kind: provider.to_string(),
            declared_model: wire.to_string(),
            wire_model: wire.to_string(),
            endpoint: EndpointIdentity::from_base_url("https://api.z.ai/api/paas/v4"),
            credential: CredentialReadiness::Configured,
            capability: ReasoningCapability::tiered(),
        }
    }

    #[test]
    fn an_endpoint_identity_keeps_the_host_and_drops_credentials() {
        let identity =
            EndpointIdentity::from_base_url("https://user:sk-secret@api.z.ai/api/paas/v4");
        assert_eq!(identity.host, "api.z.ai");
        assert!(!identity.local);
        assert!(!identity.label().contains("sk-secret"));
        assert!(!identity.label().contains('/'));
    }

    #[test]
    fn loopback_and_private_endpoints_are_marked_local() {
        for url in [
            "http://127.0.0.1:8000/v1",
            "http://localhost:11434",
            "http://192.168.1.20:8000/v1",
            "http://box.local/v1",
        ] {
            let identity = EndpointIdentity::from_base_url(url);
            assert!(identity.local, "{url} must be local");
            assert!(identity.label().ends_with("(local)"));
        }
        assert!(!EndpointIdentity::from_base_url("https://api.openai.com/v1").local);
    }

    /// The receipt and the child spawn must not be able to disagree, so there
    /// is exactly one canonical wire model and both read it.
    #[test]
    fn the_frozen_route_uses_the_canonical_wire_model() {
        let mut preflighted = route("implementer", "zai", "glm-5");
        preflighted.wire_model = "glm-5-20260101".to_string();

        assert_eq!(preflighted.frozen().model, "glm-5-20260101");
        assert_eq!(preflighted.frozen().provider, "zai");
        assert!(preflighted.model_canonicalized());
        assert_eq!(preflighted.declared_model, "glm-5");
    }

    /// Keyless local providers are first-class: readiness is about whether the
    /// route can run, not about whether a key exists.
    #[test]
    fn keyless_local_providers_are_ready() {
        let mut local = route("worker", "vllm", "qwen3");
        local.credential = CredentialReadiness::KeylessLocal;
        local.endpoint = EndpointIdentity::from_base_url("http://127.0.0.1:8000/v1");

        assert!(local.credential.is_ready());
        local.require_ready().expect("keyless local is valid");
        assert_eq!(local.credential.as_str(), "keyless_local");
    }

    #[test]
    fn a_missing_credential_fails_the_workflow_locally() {
        let mut route = route("implementer", "zai", "glm-5");
        route.credential = CredentialReadiness::Missing {
            detail: "no ZAI_API_KEY".to_string(),
        };

        let preflight = RoutePreflight::new(vec![route], None);
        let err = preflight
            .require_all_ready()
            .expect_err("a member with no credential must not start");
        assert!(matches!(err, PreflightError::CredentialMissing { .. }));
        let message = err.to_string();
        assert!(
            message.contains("decided locally"),
            "the error must say no provider was contacted: {message}"
        );
    }

    #[test]
    fn cross_provider_inference_is_detectable_from_the_preflight() {
        let preflight = RoutePreflight::new(
            vec![route("implementer", "zai", "glm-5")],
            Some(route("router", "openai", "gpt-5.6-luna")),
        );
        assert!(preflight.crosses_providers("implementer"));

        let same = RoutePreflight::new(
            vec![route("implementer", "zai", "glm-5")],
            Some(route("router", "zai", "glm-5-turbo")),
        );
        assert!(!same.crosses_providers("implementer"));

        // With no router, nothing crosses.
        let none = RoutePreflight::new(vec![route("implementer", "zai", "glm-5")], None);
        assert!(!none.crosses_providers("implementer"));
    }

    #[test]
    fn a_preflight_serializes_without_secrets_or_paths() {
        let preflight = RoutePreflight::new(
            vec![route("implementer", "zai", "glm-5")],
            Some(route("router", "openai", "gpt-5.6-luna")),
        );
        let json = serde_json::to_string(&preflight).expect("serialize");
        let lowered = json.to_ascii_lowercase();
        for forbidden in [
            "api_key", "secret", "bearer", "base_url", "/users/", "https://",
        ] {
            assert!(!lowered.contains(forbidden), "{forbidden} in {json}");
        }
        let back: RoutePreflight = serde_json::from_str(&json).expect("round-trip");
        assert_eq!(back, preflight);
    }
}
