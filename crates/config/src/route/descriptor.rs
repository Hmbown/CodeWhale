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
use crate::provider::{self, CredentialAcquisition, Provider, WirePolicy};
use serde::{Deserialize, Serialize};

use super::RequestProtocol;
use super::auth::AuthMethod;
use super::ids::{ProviderId, RouteId, WireModelId};

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

    /// Flat kebab route id for this descriptor.
    #[must_use]
    pub fn route_id(&self) -> RouteId {
        RouteId::from_kind(self.kind)
    }

    /// Display-grouping family. Not a second identity: stored identity is [`Self::route_id`].
    #[must_use]
    pub fn family(&self) -> &'static str {
        family_for(self.kind)
    }

    /// Bespoke-transport classification. OpenAI-compatible catalog rows share one kind.
    #[must_use]
    pub fn transport(&self) -> TransportKind {
        TransportKind::for_kind(self.kind)
    }

    /// Declared auth methods. OAuth is a type only; no adapter is implemented here.
    #[must_use]
    pub fn auth_methods(&self) -> &'static [AuthMethod] {
        auth_methods_for(self.kind)
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

/// Bespoke-transport classification. Catalog rows that speak OpenAI Chat
/// Completions share [`TransportKind::ChatCompletions`]; only genuinely
/// different wires get their own kind. This is the direction `ProviderKind`
/// shrinks toward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportKind {
    /// OpenAI-compatible `/v1/chat/completions`.
    ChatCompletions,
    /// Native Anthropic Messages (`/v1/messages`).
    AnthropicMessages,
    /// OpenAI Responses (`/responses`).
    OpenAiResponses,
    /// Closed, model-aware protocol roster (OpenCode Zen, DeepSeek dual-wire).
    ModelAware,
    /// ChatGPT Codex OAuth route.
    Codex,
    /// Google Antigravity consent-gated OAuth.
    Antigravity,
    /// Local runtime (Ollama / vLLM / SGLang).
    LocalRuntime,
    /// User-defined OpenAI-compatible endpoint.
    Custom,
}

impl TransportKind {
    /// Map a known kind onto the ~8 bespoke transports. Everything else is a
    /// catalog row on [`Self::ChatCompletions`].
    #[must_use]
    pub fn for_kind(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::Anthropic
            | ProviderKind::DeepseekAnthropic
            | ProviderKind::MinimaxAnthropic
            | ProviderKind::ModelstudioTokenPlanAnthropic
            | ProviderKind::ModelstudioCodingPlanAnthropic => Self::AnthropicMessages,
            ProviderKind::OpenaiCodex => Self::Codex,
            ProviderKind::Antigravity => Self::Antigravity,
            ProviderKind::Ollama | ProviderKind::Vllm | ProviderKind::Sglang => Self::LocalRuntime,
            ProviderKind::Custom => Self::Custom,
            ProviderKind::Deepseek | ProviderKind::OpencodeZen => Self::ModelAware,
            _ => match kind.provider().wire_policy() {
                WirePolicy::ModelAware => Self::ModelAware,
                WirePolicy::Fixed(crate::provider::WireFormat::Responses) => Self::OpenAiResponses,
                WirePolicy::Fixed(crate::provider::WireFormat::AnthropicMessages) => {
                    Self::AnthropicMessages
                }
                WirePolicy::Fixed(crate::provider::WireFormat::ChatCompletions) => {
                    Self::ChatCompletions
                }
            },
        }
    }
}

/// Display-grouping family. Selecting a family with multiple routes asks a
/// `select` whose option value **is a route id**.
#[must_use]
pub fn family_for(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Deepseek | ProviderKind::DeepseekAnthropic => "deepseek",
        ProviderKind::Minimax | ProviderKind::MinimaxAnthropic => "minimax",
        ProviderKind::ModelstudioTokenPlan
        | ProviderKind::ModelstudioTokenPlanAnthropic
        | ProviderKind::ModelstudioCodingPlan
        | ProviderKind::ModelstudioCodingPlanAnthropic => "alibaba-modelstudio",
        ProviderKind::Siliconflow | ProviderKind::SiliconflowCN => "siliconflow",
        ProviderKind::Ollama | ProviderKind::OllamaCloud => "ollama",
        other => other.as_str(),
    }
}

/// Auth methods declared for a provider kind. OAuth is a type, not an adapter.
#[must_use]
pub fn auth_methods_for(kind: ProviderKind) -> &'static [AuthMethod] {
    match kind.provider().credential_help().acquisition {
        CredentialAcquisition::ApiKey => &[AuthMethod::API_KEY],
        CredentialAcquisition::ApiKeyOrOAuth => &[AuthMethod::API_KEY, AuthMethod::OAUTH],
        CredentialAcquisition::LocalOptional => &[AuthMethod::KEYLESS],
        CredentialAcquisition::OAuth => &[AuthMethod::OAUTH],
        CredentialAcquisition::Configuration => &[AuthMethod::EXTERNAL_CONSENT],
    }
}
