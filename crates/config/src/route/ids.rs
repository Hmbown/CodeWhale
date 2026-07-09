//! Transparent string newtypes for provider/model/route identities.
//!
//! These types make the distinct *meanings* of route strings unmistakable at
//! the type level so callers can no longer mix:
//!
//! - [`ProviderId`] — a provider's canonical id (e.g. `"deepseek"`).
//! - [`ModelId`] — a canonical, provider-agnostic logical model id.
//! - [`WireModelId`] — a provider-owned wire id sent on the request
//!   (e.g. `"deepseek-ai/DeepSeek-V4-Pro"` on Together).
//! - [`LogicalModelRef`] — a user/selector reference to a model, which may be
//!   `"auto"`, a bare model, or an aggregator-prefixed string.
//!
//! [`ModelId`] and [`WireModelId`] are deliberately DISTINCT types and are
//! never interchangeable: a canonical model identity is not the same thing as
//! the provider-specific string put on the wire.
//!
//! INVARIANT (load-bearing for #2608): a namespace prefix can NEVER become a
//! provider. There is intentionally NO `From`/`Into` conversion from
//! [`LogicalModelRef`] or [`NamespaceHint`] to [`ProviderId`]. A prefix like
//! `deepseek-ai/` is a catalog/namespace hint only; it is not proof of
//! provider ownership. Do not add such a conversion.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The `"auto"` router sentinel for [`LogicalModelRef`].
///
/// `auto` is an opt-in router sentinel — it never refers to a literal model
/// named "auto". Centralized here so every comparison site uses the same
/// spelling (#4158).
pub const AUTO_SENTINEL: &str = "auto";

use crate::ProviderKind;

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Borrow the inner string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_newtype!(
    /// A provider's canonical identifier (e.g. `"deepseek"`, `"openrouter"`).
    ProviderId
);

string_newtype!(
    /// A canonical, provider-agnostic logical model identity.
    ///
    /// Distinct from [`WireModelId`]: this is "what the model is", not "what
    /// string a provider expects on the wire".
    ModelId
);

string_newtype!(
    /// A provider-owned wire model id sent verbatim on the request.
    ///
    /// Distinct from [`ModelId`]: aggregator-prefixed strings such as
    /// `"deepseek-ai/DeepSeek-V4-Pro"` are wire ids, not canonical identities.
    WireModelId
);

string_newtype!(
    /// A user/selector reference to a model.
    ///
    /// May be the `"auto"` sentinel, a bare model name, or an
    /// aggregator-prefixed string. A [`LogicalModelRef`] carries no provider
    /// authority by itself; see [`Self::namespace_hint`].
    LogicalModelRef
);

impl ProviderId {
    /// Build a [`ProviderId`] from a [`ProviderKind`] using its canonical id.
    #[must_use]
    pub fn from_kind(kind: ProviderKind) -> Self {
        Self(kind.as_str().to_string())
    }
}

/// A leading namespace/organization prefix carried by a [`LogicalModelRef`].
///
/// A namespace hint is a *catalog* hint only. It is NEVER convertible to a
/// [`ProviderId`]; an aggregator may serve `deepseek-ai/...` without being
/// DeepSeek, and a custom endpoint may legitimately use a look-alike string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NamespaceHint {
    /// `deepseek-ai/` prefix.
    DeepseekAi,
    /// `deepseek/` prefix.
    Deepseek,
    /// `anthropic/` prefix.
    Anthropic,
    /// `openai/` prefix.
    Openai,
    /// `qwen/` prefix.
    Qwen,
}

impl LogicalModelRef {
    /// Borrow the raw selector string.
    #[must_use]
    pub fn raw(&self) -> &str {
        self.as_str()
    }

    /// Whether this selector is the explicit `auto` router sentinel.
    ///
    /// `auto` is an opt-in router sentinel, never a literal model id.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        self.raw() == AUTO_SENTINEL
    }

    /// Parse the leading namespace prefix, if any.
    ///
    /// Returns `Some` only for the curated aggregator/organization prefixes.
    /// This is a hint about catalog namespace and does NOT identify a provider.
    #[must_use]
    pub fn namespace_hint(&self) -> Option<NamespaceHint> {
        let raw = self.raw();
        // Order matters: `deepseek-ai/` must be matched before `deepseek/`.
        if raw.starts_with("deepseek-ai/") {
            Some(NamespaceHint::DeepseekAi)
        } else if raw.starts_with("deepseek/") {
            Some(NamespaceHint::Deepseek)
        } else if raw.starts_with("anthropic/") {
            Some(NamespaceHint::Anthropic)
        } else if raw.starts_with("openai/") {
            Some(NamespaceHint::Openai)
        } else if raw.starts_with("qwen/") {
            Some(NamespaceHint::Qwen)
        } else {
            None
        }
    }
}
