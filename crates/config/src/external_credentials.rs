use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::ProviderKind;

/// Schema version for informed consent to another CLI's credential file.
pub const EXTERNAL_CREDENTIAL_CONSENT_VERSION: u32 = 1;

/// The complete side-effect contract for read-only external credentials.
pub const EXTERNAL_CREDENTIAL_READ_ONLY_SEMANTICS: &str = "read this exact file; no refresh, identity-provider or discovery requests, external-file writes, or rewrites; normal requests to the explicitly selected provider may use the token";

/// Quote an OS path for terminals, logs, JSON display fields, and errors.
///
/// The result is always one line. Terminal controls, line separators, bidi
/// formatting controls, quotes, and backslashes are escaped. Unix paths keep
/// non-UTF-8 bytes exact as `\xNN`; Windows preserves unpaired UTF-16 units as
/// `\u{NNNN}`.
#[must_use]
pub fn quote_os_path(path: &Path) -> String {
    quote_os_path_inner(path)
}

#[cfg(unix)]
fn quote_os_path_inner(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let bytes = path.as_os_str().as_bytes();
    if let Ok(text) = std::str::from_utf8(bytes) {
        return quote_path_text(text);
    }
    let mut out = String::from("\"");
    for byte in bytes {
        match byte {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x20..=0x7e => out.push(char::from(*byte)),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out.push('"');
    out
}

#[cfg(windows)]
fn quote_os_path_inner(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt as _;
    let mut out = String::from("\"");
    for decoded in char::decode_utf16(path.as_os_str().encode_wide()) {
        match decoded {
            Ok(character) => push_escaped_path_character(&mut out, character),
            Err(error) => out.push_str(&format!("\\u{{{:04x}}}", error.unpaired_surrogate())),
        }
    }
    out.push('"');
    out
}

#[cfg(not(any(unix, windows)))]
fn quote_os_path_inner(path: &Path) -> String {
    quote_path_text(&path.to_string_lossy())
}

#[cfg(not(windows))]
fn quote_path_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        push_escaped_path_character(&mut out, character);
    }
    out.push('"');
    out
}

fn push_escaped_path_character(out: &mut String, character: char) {
    match character {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{1b}' => out.push_str("\\x1b"),
        character if character.is_control() || is_bidi_format_control(character) => {
            out.extend(character.escape_unicode());
        }
        character => out.push(character),
    }
}

fn is_bidi_format_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// Resolve a user-selected path without touching the filesystem.
///
/// Consent is bound to the exact logical path, so this intentionally avoids
/// canonicalization (which would stat the candidate before consent exists).
pub fn resolve_external_credential_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| anyhow::anyhow!("resolving external credential path: {err}"))?
            .join(path)
    };

    // Normalize only lexical `.` / `..` components. Canonicalization would
    // inspect a credential path before consent exists and would also silently
    // bless a symlink target. The secure reader rejects symlink/reparse-point
    // components when the granted capability is actually consumed.
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    bail!(
                        "external credential path escapes its absolute root: {}",
                        quote_os_path(&absolute)
                    );
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if !normalized.is_absolute() {
        bail!(
            "external credential path must resolve to an absolute path: {}",
            quote_os_path(&normalized)
        );
    }
    Ok(normalized)
}

/// The side-effect envelope Codewhale may use for an external credential.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCredentialAccess {
    /// Do not inspect or access the external credential store.
    #[default]
    Disabled,
    /// Read the exact selected file without refreshing or rewriting it.
    ReadOnly,
    /// Permit a documented preservation adapter to refresh and rewrite it.
    Managed,
}

impl ExternalCredentialAccess {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ReadOnly => "read_only",
            Self::Managed => "managed",
        }
    }
}

/// External credential owners supported by the consent schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalCredentialSource {
    CodexCli,
    KimiCodeCli,
    GrokCli,
    /// Official DeepSeek Harness (`dsh`) `$DSH_HOME/.credentials.yaml`.
    DshCli,
    /// Legacy tombstone retained only so old Codewhale consent records can
    /// deserialize and be cleared. No runtime may resolve or read this source.
    AgyCli,
}

/// Default DeepSeek Harness credentials document, resolved without probing.
///
/// Matches dsh-credentials-local: `$DSH_HOME/.credentials.yaml`, or
/// `~/.dsh/.credentials.yaml` when `DSH_HOME` is unset. Consent is pinned to
/// this exact path; a later `DSH_HOME` change is reported, never followed.
#[must_use]
pub fn default_dsh_credentials_path() -> PathBuf {
    let home = match std::env::var_os("DSH_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => codewhale_paths::user_home()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".dsh"),
    };
    home.join(".credentials.yaml")
}

impl ExternalCredentialSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodexCli => "codex_cli",
            Self::KimiCodeCli => "kimi_code_cli",
            Self::GrokCli => "grok_cli",
            Self::DshCli => "dsh_cli",
            Self::AgyCli => "agy_cli",
        }
    }

    /// Human-facing owner name used in informed-consent disclosures.
    #[must_use]
    pub const fn owner_label(self) -> &'static str {
        match self {
            Self::CodexCli => "Codex CLI",
            Self::KimiCodeCli => "Kimi Code CLI",
            Self::GrokCli => "Grok CLI",
            Self::DshCli => "DeepSeek Harness",
            Self::AgyCli => "retired Antigravity consent",
        }
    }
}

/// Side-effect-free projection used by picker, config, and doctor surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCredentialConsentStatus {
    pub access: ExternalCredentialAccess,
    pub provider: String,
    pub source: ExternalCredentialSource,
    pub owner: &'static str,
    pub path: PathBuf,
    pub consent_version: u32,
    pub configured: bool,
    pub scope_valid: bool,
    /// True when the ambient CLI path now differs from the persisted pinned
    /// path. This is informational; it never redirects or deactivates consent.
    pub ambient_path_changed: bool,
    pub route_state: &'static str,
    pub semantics: &'static str,
    pub revoke_command: String,
}

impl ExternalCredentialConsentStatus {
    /// Warn without displaying the untrusted ambient replacement. The
    /// persisted path remains authoritative and is escaped for one line.
    #[must_use]
    pub fn ambient_path_warning(&self) -> Option<String> {
        self.ambient_path_changed.then(|| {
            format!(
                "warning: ambient {} credential path changed; consent remains pinned to {} and was not redirected",
                self.owner,
                quote_os_path(&self.path)
            )
        })
    }
}

/// Describe persisted external-credential policy without filesystem or network
/// access. `expected_path` is resolved lexically by the caller.
#[must_use]
pub fn external_credential_consent_status(
    consent: Option<&ExternalCredentialConsentToml>,
    provider: ProviderKind,
    source: ExternalCredentialSource,
    expected_path: &Path,
    active_provider: ProviderKind,
) -> ExternalCredentialConsentStatus {
    let configured = consent.is_some();
    let access = consent.map_or(ExternalCredentialAccess::Disabled, |value| value.access);
    // User-facing status identifies the route being inspected. Persisted
    // provider/source fields are untrusted config input and are represented by
    // `scope_valid` rather than echoed into a terminal surface.
    let reported_provider = provider.as_str().to_string();
    let reported_source = source;
    let reported_path = consent
        .map(|value| value.path.clone())
        .unwrap_or_else(|| expected_path.to_path_buf());
    let consent_version = consent.map_or(EXTERNAL_CREDENTIAL_CONSENT_VERSION, |value| {
        value.consent_version
    });
    let scope_valid = consent.is_some_and(|value| {
        value
            .validate_read_scope(provider, source, &value.path)
            .is_ok()
    });
    let ambient_path_changed = consent.is_some_and(|value| value.path != expected_path);
    let active =
        provider == active_provider && access == ExternalCredentialAccess::ReadOnly && scope_valid;
    let route_state = if active { "active" } else { "dormant" };
    let semantics = match access {
        ExternalCredentialAccess::Disabled => {
            "disabled; no external-credential probing, reading, refresh, discovery, identity-provider or network acquisition, writes, or rewrites; normal requests to the explicitly selected provider may use Codewhale-owned credentials"
        }
        ExternalCredentialAccess::ReadOnly => EXTERNAL_CREDENTIAL_READ_ONLY_SEMANTICS,
        ExternalCredentialAccess::Managed => {
            "managed access unavailable; no schema-safe preservation adapter"
        }
    };

    ExternalCredentialConsentStatus {
        access,
        provider: reported_provider,
        source: reported_source,
        owner: reported_source.owner_label(),
        path: reported_path,
        consent_version,
        configured,
        scope_valid,
        ambient_path_changed,
        route_state,
        semantics,
        revoke_command: format!(
            "codewhale auth external-revoke --provider {}",
            provider.as_str()
        ),
    }
}

/// Persisted, provider-scoped consent for one exact external credential file.
///
/// Provider and source are repeated intentionally. A copied provider table or
/// a future source-path remap must fail closed instead of inheriting authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalCredentialConsentToml {
    pub access: ExternalCredentialAccess,
    pub provider: String,
    pub source: ExternalCredentialSource,
    pub path: PathBuf,
    pub consent_version: u32,
}

impl ExternalCredentialConsentToml {
    #[must_use]
    pub fn read_only(
        provider: ProviderKind,
        source: ExternalCredentialSource,
        path: PathBuf,
    ) -> Self {
        Self {
            access: ExternalCredentialAccess::ReadOnly,
            provider: provider.as_str().to_string(),
            source,
            path,
            consent_version: EXTERNAL_CREDENTIAL_CONSENT_VERSION,
        }
    }

    /// Validate that this record is a current read-only consent for one exact
    /// provider/source/path tuple without minting an I/O capability.
    ///
    /// This is intentionally side-effect free so inventory and picker surfaces
    /// can acknowledge dormant consent without inspecting the external file.
    pub fn validate_read_scope(
        &self,
        provider: ProviderKind,
        source: ExternalCredentialSource,
        resolved_path: &Path,
    ) -> Result<()> {
        if self.access == ExternalCredentialAccess::Disabled {
            bail!(
                "external credential access is disabled for {}",
                provider.as_str()
            );
        }
        if self.access == ExternalCredentialAccess::Managed {
            bail!(
                "managed external credential access is unsupported for {}; no schema-safe preservation adapter is available",
                provider.as_str()
            );
        }
        if self.consent_version != EXTERNAL_CREDENTIAL_CONSENT_VERSION {
            bail!(
                "external credential consent for {} uses unsupported version {}; revoke and consent again",
                provider.as_str(),
                self.consent_version
            );
        }
        if self.provider != provider.as_str() {
            bail!(
                "external credential consent is scoped to provider {:?}, not {}",
                self.provider,
                provider.as_str()
            );
        }
        if self.source != source {
            bail!(
                "external credential consent source mismatch for {} (expected {})",
                provider.as_str(),
                source.as_str()
            );
        }
        if !self.path.is_absolute() {
            bail!(
                "external credential consent path for {} must be absolute",
                provider.as_str()
            );
        }
        let normalized = resolve_external_credential_path(&self.path)?;
        if normalized != self.path {
            bail!(
                "external credential consent path for {} must be lexically normalized: {}",
                provider.as_str(),
                quote_os_path(&self.path)
            );
        }
        if self.path != resolved_path {
            bail!(
                "external credential path changed for {}; consent covers {}, current path is {}",
                provider.as_str(),
                quote_os_path(&self.path),
                quote_os_path(resolved_path)
            );
        }
        Ok(())
    }

    /// Validate and mint the read capability consumed by credential adapters.
    /// No filesystem operation occurs while validating the policy.
    pub fn read_grant(
        &self,
        provider: ProviderKind,
        source: ExternalCredentialSource,
        resolved_path: &Path,
    ) -> Result<ExternalCredentialReadGrant> {
        self.validate_read_scope(provider, source, resolved_path)?;
        Ok(ExternalCredentialReadGrant {
            provider,
            source,
            path: resolved_path.to_path_buf(),
            consent_version: self.consent_version,
        })
    }
}

/// Opaque proof that one exact provider/source/path tuple may be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCredentialReadGrant {
    provider: ProviderKind,
    source: ExternalCredentialSource,
    path: PathBuf,
    consent_version: u32,
}

impl ExternalCredentialReadGrant {
    #[must_use]
    pub fn provider(&self) -> ProviderKind {
        self.provider
    }

    #[must_use]
    pub fn source(&self) -> ExternalCredentialSource {
        self.source
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn consent_version(&self) -> u32 {
        self.consent_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute_test_path(file: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\Users\test\{file}"))
        } else {
            PathBuf::from(format!("/tmp/{file}"))
        }
    }

    #[test]
    fn default_dsh_credentials_path_uses_dsh_home_or_dot_dsh() {
        let previous = std::env::var_os("DSH_HOME");
        unsafe {
            std::env::set_var("DSH_HOME", "/opt/dsh-home");
        }
        let with_home = default_dsh_credentials_path();
        match previous {
            Some(value) => unsafe { std::env::set_var("DSH_HOME", value) },
            None => unsafe { std::env::remove_var("DSH_HOME") },
        }
        assert_eq!(with_home, PathBuf::from("/opt/dsh-home/.credentials.yaml"));
        assert_eq!(ExternalCredentialSource::DshCli.as_str(), "dsh_cli");
        assert_eq!(
            ExternalCredentialSource::DshCli.owner_label(),
            "DeepSeek Harness"
        );
    }

    #[test]
    fn disclosed_paths_are_absolute_and_lexically_normalized_without_io() {
        let resolved =
            resolve_external_credential_path("one/./two/../auth.json").expect("lexical resolution");
        assert!(resolved.is_absolute());
        assert!(
            resolved.ends_with(Path::new("one/auth.json")),
            "{}",
            resolved.display()
        );
        assert!(!resolved.to_string_lossy().contains("/./"));
        assert!(!resolved.to_string_lossy().contains("/../"));
    }

    #[test]
    fn structural_status_reports_full_scope_without_io() {
        let path = absolute_test_path("codex-auth.json");
        let consent = ExternalCredentialConsentToml::read_only(
            ProviderKind::OpenaiCodex,
            ExternalCredentialSource::CodexCli,
            path.clone(),
        );
        let active = external_credential_consent_status(
            Some(&consent),
            ProviderKind::OpenaiCodex,
            ExternalCredentialSource::CodexCli,
            &path,
            ProviderKind::OpenaiCodex,
        );
        assert_eq!(active.access, ExternalCredentialAccess::ReadOnly);
        assert_eq!(active.owner, "Codex CLI");
        assert_eq!(active.path, path);
        assert_eq!(active.route_state, "active");
        assert!(active.scope_valid);
        assert!(active.semantics.contains("no refresh"));
        assert_eq!(
            active.revoke_command,
            "codewhale auth external-revoke --provider openai-codex"
        );

        let changed_path = absolute_test_path("moved-auth.json");
        let pinned = external_credential_consent_status(
            Some(&consent),
            ProviderKind::OpenaiCodex,
            ExternalCredentialSource::CodexCli,
            &changed_path,
            ProviderKind::OpenaiCodex,
        );
        assert!(pinned.scope_valid);
        assert_eq!(pinned.route_state, "active");
        assert!(pinned.ambient_path_changed);
        assert_eq!(pinned.path, path, "report the pinned persisted grant path");
        let warning = pinned
            .ambient_path_warning()
            .expect("ambient mismatch warning");
        assert!(warning.contains("remains pinned"), "{warning}");
        assert!(warning.contains(&quote_os_path(&path)), "{warning}");
    }

    #[test]
    fn displayed_paths_escape_terminal_and_bidi_controls_on_one_line() {
        let path = PathBuf::from(
            "/safe/line\nmanaged\u{1b}[2J\u{2028}first\u{2029}second\u{202e}name.json",
        );
        let quoted = quote_os_path(&path);
        assert!(quoted.starts_with('"') && quoted.ends_with('"'));
        assert!(quoted.contains("\\n"), "{quoted}");
        assert!(quoted.contains("\\x1b"), "{quoted}");
        assert!(quoted.contains("\\u{2028}"), "{quoted}");
        assert!(quoted.contains("\\u{2029}"), "{quoted}");
        assert!(quoted.contains("\\u{202e}"), "{quoted}");
        assert!(!quoted.contains('\n'));
        assert!(!quoted.contains('\u{1b}'));
        assert!(!quoted.contains('\u{2028}'));
        assert!(!quoted.contains('\u{2029}'));
        assert!(!quoted.contains('\u{202e}'));
    }

    #[test]
    fn disabled_disclosure_does_not_imply_normal_provider_network_is_disabled() {
        let path = absolute_test_path("codex-auth.json");
        let status = external_credential_consent_status(
            None,
            ProviderKind::OpenaiCodex,
            ExternalCredentialSource::CodexCli,
            &path,
            ProviderKind::OpenaiCodex,
        );
        assert!(status.semantics.contains("no external-credential"));
        assert!(status.semantics.contains("normal requests"));
        assert!(!status.semantics.contains("no network requests"));
    }

    #[test]
    fn read_grant_requires_exact_provider_source_path_and_version() {
        let path = absolute_test_path("codex-auth.json");
        let consent = ExternalCredentialConsentToml::read_only(
            ProviderKind::OpenaiCodex,
            ExternalCredentialSource::CodexCli,
            path.clone(),
        );

        let grant = consent
            .read_grant(
                ProviderKind::OpenaiCodex,
                ExternalCredentialSource::CodexCli,
                &path,
            )
            .expect("exact consent tuple");
        assert_eq!(grant.path(), path);

        assert!(
            consent
                .read_grant(ProviderKind::Xai, ExternalCredentialSource::CodexCli, &path)
                .is_err()
        );
        assert!(
            consent
                .read_grant(
                    ProviderKind::OpenaiCodex,
                    ExternalCredentialSource::GrokCli,
                    &path
                )
                .is_err()
        );
        assert!(
            consent
                .read_grant(
                    ProviderKind::OpenaiCodex,
                    ExternalCredentialSource::CodexCli,
                    &path.with_file_name("other.json")
                )
                .is_err()
        );
    }

    #[test]
    fn persisted_consent_path_must_be_lexically_normalized() {
        let raw_path = if cfg!(windows) {
            PathBuf::from(r"C:\Users\test\credentials\..\auth.json")
        } else {
            PathBuf::from("/tmp/credentials/../auth.json")
        };
        let consent = ExternalCredentialConsentToml::read_only(
            ProviderKind::Xai,
            ExternalCredentialSource::GrokCli,
            raw_path.clone(),
        );
        assert!(
            consent
                .read_grant(
                    ProviderKind::Xai,
                    ExternalCredentialSource::GrokCli,
                    &raw_path
                )
                .is_err()
        );
    }

    #[test]
    fn managed_consent_is_explicitly_unsupported_without_an_adapter() {
        let path = absolute_test_path("grok-auth.json");
        let mut consent = ExternalCredentialConsentToml::read_only(
            ProviderKind::Xai,
            ExternalCredentialSource::GrokCli,
            path.clone(),
        );
        consent.access = ExternalCredentialAccess::Managed;

        let error = consent
            .read_grant(ProviderKind::Xai, ExternalCredentialSource::GrokCli, &path)
            .expect_err("managed access must fail closed");
        assert!(
            error
                .to_string()
                .contains("schema-safe preservation adapter")
        );
    }

    #[test]
    fn consent_round_trips_every_scope_field() {
        let path = absolute_test_path("codex-auth.json");
        let consent = ExternalCredentialConsentToml::read_only(
            ProviderKind::OpenaiCodex,
            ExternalCredentialSource::CodexCli,
            path,
        );

        let encoded = toml::to_string(&consent).expect("serialize consent");
        let decoded: ExternalCredentialConsentToml =
            toml::from_str(&encoded).expect("deserialize consent");
        assert_eq!(decoded, consent);
        assert!(encoded.contains("access = \"read_only\""));
        assert!(encoded.contains("provider = \"openai-codex\""));
        assert!(encoded.contains("source = \"codex_cli\""));
        assert!(encoded.contains("consent_version = 1"));
    }

    #[test]
    fn disabled_stale_and_relative_consent_fail_before_a_grant() {
        let path = absolute_test_path("grok-auth.json");
        let mut consent = ExternalCredentialConsentToml::read_only(
            ProviderKind::Xai,
            ExternalCredentialSource::GrokCli,
            path.clone(),
        );

        consent.access = ExternalCredentialAccess::Disabled;
        assert!(
            consent
                .read_grant(ProviderKind::Xai, ExternalCredentialSource::GrokCli, &path)
                .expect_err("disabled consent")
                .to_string()
                .contains("disabled")
        );

        consent.access = ExternalCredentialAccess::ReadOnly;
        consent.consent_version = EXTERNAL_CREDENTIAL_CONSENT_VERSION + 1;
        assert!(
            consent
                .read_grant(ProviderKind::Xai, ExternalCredentialSource::GrokCli, &path)
                .expect_err("stale consent")
                .to_string()
                .contains("unsupported version")
        );

        consent.consent_version = EXTERNAL_CREDENTIAL_CONSENT_VERSION;
        consent.path = PathBuf::from("relative/auth.json");
        assert!(
            consent
                .read_grant(
                    ProviderKind::Xai,
                    ExternalCredentialSource::GrokCli,
                    Path::new("relative/auth.json"),
                )
                .expect_err("relative path")
                .to_string()
                .contains("must be absolute")
        );
    }
}
