//! Declarative auth types for the Route Contract (Phase 1).
//!
//! These types describe how a route asks for credentials. They do **not**
//! implement OAuth adapters. `AuthKind::OAuth` is a declared method only;
//! `UNIFIED_PROVIDER_LOGIN.md` remains gated.

use serde::{Deserialize, Serialize};

/// How a route obtains credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    /// A provider-issued API key. Semantics are unchanged from today's first-class key path.
    ApiKey,
    /// An OAuth adapter *type*. Do not implement the Anthropic OAuth flow here.
    #[serde(rename = "oauth")]
    OAuth,
    /// No credential required (local / keyless by default).
    Keyless,
    /// Read-only consent to an external CLI's credential file.
    ExternalConsent,
}

/// A single declared auth method on a route.
///
/// Phase 2 walks [`Self::prompts`] as one loop. Phase 1 only ships the types
/// and the export projection so the descriptor is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthMethod {
    /// Acquisition kind.
    pub kind: AuthKind,
    /// Human label for the picker / export.
    pub label: &'static str,
    /// Ordered prompts. Usually empty for a plain API-key route.
    pub prompts: &'static [Prompt],
}

/// One question in a declarative auth prompt loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// Free-text (or secret) answer stored under `key`.
    Text {
        /// Answer map key.
        key: &'static str,
        /// Prompt shown to the user.
        message: &'static str,
        /// Optional placeholder.
        placeholder: Option<&'static str>,
        /// When true, the answer is a secret and must not be echoed.
        secret: bool,
        /// Optional visibility predicate.
        when: Option<When>,
    },
    /// Forced choice; `value` of the selected option is stored under `key`.
    Select {
        /// Answer map key.
        key: &'static str,
        /// Prompt shown to the user.
        message: &'static str,
        /// Options. A family-disambiguation `value` is a route id.
        options: &'static [Choice],
        /// Optional visibility predicate.
        when: Option<When>,
    },
}

/// Predicate over previously collected answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    /// Answer key to compare.
    pub key: &'static str,
    /// Comparison operator.
    pub op: Op,
    /// Comparison value.
    pub value: &'static str,
}

/// Comparison used by [`When`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Op {
    /// Answer equals `value`.
    Eq,
    /// Answer does not equal `value`.
    Neq,
}

/// One option in a [`Prompt::Select`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    /// Display label.
    pub label: &'static str,
    /// Stored value. For family disambiguation this **is** a route id.
    pub value: &'static str,
    /// Optional hint.
    pub hint: Option<&'static str>,
}

impl AuthMethod {
    /// Plain API-key method with no extra prompts.
    pub const API_KEY: Self = Self {
        kind: AuthKind::ApiKey,
        label: "API key",
        prompts: &[],
    };

    /// Declared OAuth method. Adapter implementation is out of Phase 1 scope.
    pub const OAUTH: Self = Self {
        kind: AuthKind::OAuth,
        label: "OAuth",
        prompts: &[],
    };

    /// Keyless / local-optional method.
    pub const KEYLESS: Self = Self {
        kind: AuthKind::Keyless,
        label: "Keyless",
        prompts: &[],
    };

    /// External-consent method (read-only grant to another CLI's file).
    pub const EXTERNAL_CONSENT: Self = Self {
        kind: AuthKind::ExternalConsent,
        label: "External consent",
        prompts: &[],
    };

    /// Whether `answers` satisfy this prompt's `when` clause.
    #[must_use]
    pub fn prompt_visible(
        prompt: &Prompt,
        answers: &std::collections::BTreeMap<String, String>,
    ) -> bool {
        let when = match prompt {
            Prompt::Text { when, .. } | Prompt::Select { when, .. } => *when,
        };
        match when {
            None => true,
            Some(When { key, op, value }) => {
                let actual = answers.get(key).map(String::as_str).unwrap_or("");
                match op {
                    Op::Eq => actual == value,
                    Op::Neq => actual != value,
                }
            }
        }
    }
}

/// Owned, serializable projection of [`AuthMethod`] for `providers export`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethodExport {
    /// Acquisition kind.
    pub kind: AuthKind,
    /// Human label.
    pub label: String,
    /// Owned prompts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<PromptExport>,
}

/// Owned, serializable projection of [`Prompt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PromptExport {
    /// Free-text prompt.
    Text {
        /// Answer map key.
        key: String,
        /// Prompt shown to the user.
        message: String,
        /// Optional placeholder.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
        /// Whether the answer is a secret.
        secret: bool,
        /// Optional visibility predicate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<WhenExport>,
    },
    /// Forced-choice prompt.
    Select {
        /// Answer map key.
        key: String,
        /// Prompt shown to the user.
        message: String,
        /// Options.
        options: Vec<ChoiceExport>,
        /// Optional visibility predicate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<WhenExport>,
    },
}

/// Owned [`When`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhenExport {
    /// Answer key to compare.
    pub key: String,
    /// Comparison operator.
    pub op: Op,
    /// Comparison value.
    pub value: String,
}

/// Owned [`Choice`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChoiceExport {
    /// Display label.
    pub label: String,
    /// Stored value.
    pub value: String,
    /// Optional hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl From<AuthMethod> for AuthMethodExport {
    fn from(value: AuthMethod) -> Self {
        Self {
            kind: value.kind,
            label: value.label.to_string(),
            prompts: value
                .prompts
                .iter()
                .copied()
                .map(PromptExport::from)
                .collect(),
        }
    }
}

impl From<Prompt> for PromptExport {
    fn from(value: Prompt) -> Self {
        match value {
            Prompt::Text {
                key,
                message,
                placeholder,
                secret,
                when,
            } => Self::Text {
                key: key.to_string(),
                message: message.to_string(),
                placeholder: placeholder.map(str::to_string),
                secret,
                when: when.map(WhenExport::from),
            },
            Prompt::Select {
                key,
                message,
                options,
                when,
            } => Self::Select {
                key: key.to_string(),
                message: message.to_string(),
                options: options.iter().copied().map(ChoiceExport::from).collect(),
                when: when.map(WhenExport::from),
            },
        }
    }
}

impl From<When> for WhenExport {
    fn from(value: When) -> Self {
        Self {
            key: value.key.to_string(),
            op: value.op,
            value: value.value.to_string(),
        }
    }
}

impl From<Choice> for ChoiceExport {
    fn from(value: Choice) -> Self {
        Self {
            label: value.label.to_string(),
            value: value.value.to_string(),
            hint: value.hint.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_eq_hides_unmatched_prompt() {
        let prompt = Prompt::Text {
            key: "key",
            message: "Key",
            placeholder: None,
            secret: true,
            when: Some(When {
                key: "plan",
                op: Op::Eq,
                value: "coding",
            }),
        };
        let mut answers = std::collections::BTreeMap::new();
        answers.insert("plan".into(), "token".into());
        assert!(!AuthMethod::prompt_visible(&prompt, &answers));
        answers.insert("plan".into(), "coding".into());
        assert!(AuthMethod::prompt_visible(&prompt, &answers));
    }
}
