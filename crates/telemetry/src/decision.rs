//! The one place that decides whether anything may be collected, and the token
//! that makes that decision unforgeable.
//!
//! Every emitting surface calls [`decide`] and then, if and only if it gets
//! [`TelemetryDecision::Enabled`], hands the contained [`TelemetryConsent`] to
//! [`crate::init`]. `TelemetryConsent` has no `Default`, no public constructor,
//! and cannot be built from a `bool`; `init` takes it **by value**. That is what
//! makes consent enforceable by the type system rather than by six init sites
//! each remembering to re-check the same five-part predicate.

use std::path::{Path, PathBuf};

use codewhale_config::{ResolvedRuntimeOptions, SetupState, TELEMETRY_NOTICE_VERSION};

use crate::buffer;
use crate::event::Surface;

/// Directory name under `$CODEWHALE_HOME` that holds every telemetry file.
pub const TELEMETRY_DIR: &str = "telemetry";

/// The outcome of the emit predicate.
///
/// The split between [`Self::OptedOut`] and [`Self::ForcedOff`] is load-bearing,
/// not cosmetic. "Telemetry resolved to false" is the *default* state of every
/// installation, so a wipe keyed on it would delete a consenting user's identity
/// and unflushed buffer every time they ran one `codewhale exec` with a
/// transient `CODEWHALE_TELEMETRY=0` — the recipe the runtime docs themselves
/// prescribe.
#[derive(Debug)]
pub enum TelemetryDecision {
    /// The user answered the notice and said yes, and nothing forces off.
    Enabled(TelemetryConsent),
    /// A human said no — `--telemetry false`, `CODEWHALE_TELEMETRY=0`,
    /// `telemetry = false`, or declining the notice. **The only variant that
    /// touches disk**: it wipes and leaves a tombstone.
    OptedOut,
    /// Off for a reason that is not the user's answer: no notice decision
    /// recorded, an unparseable env value, an unresolvable home, a rejected
    /// endpoint, or a bumped notice version. Touches nothing, ever. Leaves
    /// identity and buffer exactly as they were.
    ForcedOff,
}

impl TelemetryDecision {
    /// Whether this decision permits emission.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    /// A stable label for logs and tests.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Enabled(_) => "enabled",
            Self::OptedOut => "opted_out",
            Self::ForcedOff => "forced_off",
        }
    }
}

/// Proof that a specific machine, at a specific moment, was permitted to
/// collect.
///
/// Constructed only by [`decide`]. Not `Default`, not constructible from a
/// `bool`, and consumed by value.
#[derive(Debug)]
pub struct TelemetryConsent {
    root: PathBuf,
    endpoint: Option<String>,
    surface: Surface,
    config_path: Option<PathBuf>,
}

impl TelemetryConsent {
    /// Remember which config file this process was launched with, so the flush
    /// path can re-resolve from it.
    ///
    /// Without this the documented mid-session opt-out —
    /// `codewhale config set telemetry false`, an external write by another
    /// process — would never be observed by a session that is already running.
    #[must_use]
    pub fn with_config_path(mut self, config_path: Option<PathBuf>) -> Self {
        self.config_path = config_path;
        self
    }

    /// The config file this process was launched with, if any.
    #[must_use]
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// `$CODEWHALE_HOME/telemetry`.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The validated endpoint, or `None` for the dry-run sink.
    #[must_use]
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    /// The surface this consent was resolved for.
    #[must_use]
    pub fn surface(&self) -> Surface {
        self.surface
    }
}

/// Why an endpoint was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointError {
    /// Not a URL we could parse at all.
    Unparseable,
    /// `http://` to something that is not loopback.
    InsecureScheme,
    /// A scheme that is neither `http` nor `https`.
    UnsupportedScheme,
}

impl EndpointError {
    /// A stable label for the single `warn` line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Unparseable => "unparseable",
            Self::InsecureScheme => "plaintext http to a non-loopback host",
            Self::UnsupportedScheme => "scheme is neither https nor http",
        }
    }
}

/// Validate a configured endpoint.
///
/// `https://` is required. Plaintext is permitted **only** for loopback hosts,
/// where a batch never reaches a wire — that is the staging and dogfood case.
///
/// There is deliberately **no environment variable that overrides this**.
/// `CODEWHALE_ALLOW_INSECURE_HTTP` is not consulted: it authorizes an insecure
/// *provider* base URL, for harnesses that legitimately intercept model traffic,
/// and reusing it would let that interception decision also authorize plaintext
/// telemetry POSTs to an arbitrary host. Two unrelated trust decisions must not
/// share one switch, least of all in the subsystem whose whole promise is that
/// the user knows what leaves the machine.
pub fn validate_endpoint(raw: &str) -> Result<String, EndpointError> {
    let trimmed = raw.trim();
    let url = reqwest::Url::parse(trimmed).map_err(|_| EndpointError::Unparseable)?;
    match url.scheme() {
        "https" => Ok(trimmed.to_string()),
        "http" => {
            if is_loopback_host(url.host_str()) {
                Ok(trimmed.to_string())
            } else {
                Err(EndpointError::InsecureScheme)
            }
        }
        _ => Err(EndpointError::UnsupportedScheme),
    }
}

/// Whether a host is one a packet can never leave the machine to reach.
///
/// `Url::host_str` returns an IPv6 literal in its bracketed form (`[::1]`), so
/// the brackets come off before the address is parsed. Anything that parses as
/// an IP is judged by `is_loopback` — 127.0.0.0/8 and `::1` — and the only
/// accepted name is `localhost`.
fn is_loopback_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => bare.eq_ignore_ascii_case("localhost"),
    }
}

/// Resolve the emit predicate, reading the Codewhale home from the environment.
///
/// See [`decide_in_home`] for the injectable form used by tests.
pub fn decide(
    resolved: &ResolvedRuntimeOptions,
    setup: &SetupState,
    surface: Surface,
) -> TelemetryDecision {
    // `codewhale_home()` returns `Ok(None)` when no home can be resolved, and
    // an error when an explicit override was unusable. Both are "we have
    // nowhere to keep state", which is `ForcedOff`, never a wipe.
    let home = codewhale_paths::codewhale_home().ok().flatten();
    decide_in_home(home.as_deref(), resolved, setup, surface)
}

/// Resolve the emit predicate against an explicit Codewhale home.
///
/// The predicate, in order:
///
/// 1. Telemetry resolved to `false` **and** a human said so → `OptedOut`;
///    resolved `false` from the unset default → `ForcedOff`.
/// 2. Notice decision recorded and declined → `OptedOut`.
/// 3. No notice decision for the current notice version → `ForcedOff`. **A
///    pre-existing `telemetry = true` is not consent**: the key has been
///    settable and inert for a long time, so anyone who set it set a no-op. The
///    notice record is an independent AND condition, never inferred from the
///    bool.
/// 4. No resolvable home → `ForcedOff`.
/// 5. Endpoint configured but refused by [`validate_endpoint`] → `ForcedOff`.
/// 6. Otherwise `Enabled`.
///
/// Consent is **machine-scoped**. The notice is only ever *rendered* on a TTY,
/// but a decision recorded on a TTY authorizes later non-TTY runs on the same
/// home. A fresh CI home has no decision, so step 3 fires and nothing is
/// collected — and nothing is written to disk to find out.
pub fn decide_in_home(
    home: Option<&Path>,
    resolved: &ResolvedRuntimeOptions,
    setup: &SetupState,
    surface: Surface,
) -> TelemetryDecision {
    let root = home.map(|home| home.join(TELEMETRY_DIR));

    // 1. An explicit "off" from a human is an answer and wipes; the unset
    //    default is not an answer and must leave every byte alone.
    if !resolved.telemetry {
        if resolved.telemetry_explicit_off {
            return opted_out(root.as_deref());
        }
        return TelemetryDecision::ForcedOff;
    }

    // 2/3. The notice record is an independent condition. Declining is an
    //      answer; never having been asked is not.
    if setup.needs_telemetry_notice(TELEMETRY_NOTICE_VERSION) {
        return TelemetryDecision::ForcedOff;
    }
    if !setup.telemetry_opt_in {
        return opted_out(root.as_deref());
    }

    // 4. Nowhere to keep an install id or a buffer.
    let Some(root) = root else {
        return TelemetryDecision::ForcedOff;
    };

    // 5. A refused endpoint is a configuration error, not a user answer.
    let endpoint = match resolved.telemetry_endpoint.as_deref() {
        Some(raw) if !raw.trim().is_empty() => match validate_endpoint(raw) {
            Ok(endpoint) => Some(endpoint),
            Err(error) => {
                tracing::warn!(
                    "telemetry endpoint refused ({}); telemetry is off for this run",
                    error.label()
                );
                return TelemetryDecision::ForcedOff;
            }
        },
        _ => None,
    };

    TelemetryDecision::Enabled(TelemetryConsent {
        root,
        endpoint,
        surface,
        config_path: None,
    })
}

/// Re-run the predicate from the filesystem, for the flush path.
///
/// Loads the same config file the process was launched with and the current
/// setup state, so a `codewhale config set telemetry false` written by another
/// process between init and flush is honoured. Returns `ForcedOff` if either
/// load fails: a flush is never the right place to guess.
#[must_use]
pub fn re_decide(config_path: Option<&Path>, surface: Surface) -> TelemetryDecision {
    let Ok(store) = codewhale_config::ConfigStore::load(config_path.map(Path::to_path_buf)) else {
        return TelemetryDecision::ForcedOff;
    };
    let resolved = store
        .config
        .resolve_runtime_options(&codewhale_config::CliRuntimeOverrides::default());
    let setup = SetupState::load().ok().flatten().unwrap_or_default();
    decide(&resolved, &setup, surface)
}

/// Perform the opt-out wipe, then report `OptedOut`.
///
/// Nothing is created for a user who never opted in: if the telemetry directory
/// does not exist there is nothing to wipe and nothing to announce, so this
/// returns without touching the filesystem.
fn opted_out(root: Option<&Path>) -> TelemetryDecision {
    if let Some(root) = root
        && root.is_dir()
        && let Err(error) = buffer::wipe(root)
    {
        // A failed wipe fails **closed**: the tombstone is written first and is
        // never removed by the wipe, so even a partial failure leaves the
        // buffer permanently undrainable.
        tracing::warn!("telemetry opt-out wipe was incomplete: {error}");
    }
    TelemetryDecision::OptedOut
}
