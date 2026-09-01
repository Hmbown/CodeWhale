//! Where a credential came from, and — when there isn't one — where we looked.
//!
//! Ported from pi-mono's `AuthResult { auth, env, source }`
//! (`packages/ai/src/auth/types.ts`, MIT, Copyright (c) 2025 Mario Zechner;
//! full notice in the parent module). pi returns a human-readable `source`
//! such as `"ANTHROPIC_API_KEY"`, `"OAuth"`, or `"~/.aws/credentials"` from
//! every resolution so a status surface can say which place won.
//!
//! CodeWhale adds the negative half, because that is where its picker was
//! useless: a failed resolution carries the ordered list of places that were
//! actually probed, so "missing key" can name them and say what would fix it.
//!
//! Nothing here holds secret material — only labels.

use std::borrow::Cow;

/// One place credential resolution looked, and what would put a credential
/// there. Labels only; never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialProbe {
    /// Where we looked, e.g. `"env DEEPSEEK_API_KEY"` or `"secret store slot \"deepseek\""`.
    pub(crate) place: Cow<'static, str>,
    /// What the user would do to make this place answer, if there is a
    /// one-line answer. `None` when the place is informational only.
    pub(crate) fix: Option<Cow<'static, str>>,
}

impl CredentialProbe {
    pub(crate) fn with_fix(
        place: impl Into<Cow<'static, str>>,
        fix: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            place: place.into(),
            fix: Some(fix.into()),
        }
    }
}

/// The place a credential was resolved from, or the fact that no place had
/// one. This is the single value every readiness surface should render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CredentialSource {
    /// `auth_mode = "none"`: the route intentionally sends no credential.
    AuthModeNone,
    /// A keyless self-hosted or loopback route.
    KeylessRoute { base_url: String },
    /// `--api-key` on the command line (or the dispatcher's source-marked
    /// forward of it).
    CliOverride,
    /// The root `api_key` compatibility slot in the config file.
    RootConfigApiKey,
    /// `[providers.<table>] api_key`.
    ProviderConfigApiKey { table: String },
    /// `[providers.<table>] api_key_env = "<var>"`, resolved from `<var>`.
    ProviderConfigEnv { var: String },
    /// An ambient provider environment variable.
    AmbientEnv { var: String },
    /// CodeWhale's own durable secret store.
    SecretStore { slot: String },
    /// A read-only, explicitly consented credential file owned by another CLI.
    ExternalGrant { cli: String, path: String },
    /// CodeWhale-owned OAuth device-login storage (xAI today).
    OAuth { flow: String },
    /// The user-global `~/.codewhale/config.toml`, consulted last so a key
    /// saved there survives loading a workspace config.
    UserGlobalConfig,
    /// Nothing had a credential. `probed` is in precedence order.
    Missing { probed: Vec<CredentialProbe> },
}

impl CredentialSource {
    pub(crate) fn is_present(&self) -> bool {
        !matches!(self, Self::Missing { .. })
    }

    /// Short human-readable label, in pi's spirit: the name of the place, not
    /// a sentence. Safe to render anywhere — never contains a secret.
    pub(crate) fn label(&self) -> Cow<'static, str> {
        match self {
            Self::AuthModeNone => Cow::Borrowed("auth_mode = \"none\""),
            Self::KeylessRoute { base_url } => Cow::Owned(format!("keyless route {base_url}")),
            Self::CliOverride => Cow::Borrowed("--api-key"),
            Self::RootConfigApiKey => Cow::Borrowed("config api_key"),
            Self::ProviderConfigApiKey { table } => Cow::Owned(format!("[{table}] api_key")),
            Self::ProviderConfigEnv { var } => Cow::Owned(format!("api_key_env {var}")),
            Self::AmbientEnv { var } => Cow::Owned(var.clone()),
            Self::SecretStore { slot } => Cow::Owned(format!("secret store \"{slot}\"")),
            Self::ExternalGrant { cli, path } => {
                Cow::Owned(format!("{cli} credentials (read-only) {path}"))
            }
            Self::OAuth { flow } => Cow::Owned(format!("{flow} OAuth")),
            Self::UserGlobalConfig => Cow::Borrowed("~/.codewhale/config.toml api_key"),
            Self::Missing { .. } => Cow::Borrowed("not found"),
        }
    }

    /// The ordered places that were probed, for a failed resolution.
    pub(crate) fn probed(&self) -> &[CredentialProbe] {
        match self {
            Self::Missing { probed } => probed,
            _ => &[],
        }
    }
}

/// A resolution plus its source. Deliberately does not carry the credential:
/// readiness surfaces need the source, and the request path already has its
/// own resolver that returns the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialResolution {
    pub(crate) source: CredentialSource,
}

impl CredentialResolution {
    pub(crate) fn found(source: CredentialSource) -> Self {
        debug_assert!(source.is_present());
        Self { source }
    }

    pub(crate) fn missing(probed: Vec<CredentialProbe>) -> Self {
        Self {
            source: CredentialSource::Missing { probed },
        }
    }

    pub(crate) fn is_present(&self) -> bool {
        self.source.is_present()
    }

    /// One line naming the places checked, for a status row. Empty when the
    /// resolution succeeded.
    pub(crate) fn checked_places(&self) -> String {
        self.source
            .probed()
            .iter()
            .map(|probe| probe.place.as_ref())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The first actionable fix among the probed places, if any.
    pub(crate) fn first_fix(&self) -> Option<&str> {
        self.source
            .probed()
            .iter()
            .find_map(|probe| probe.fix.as_deref())
    }
}
