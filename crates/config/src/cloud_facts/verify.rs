//! Envelope verification: size cap → shape → pinned key → Ed25519 → payload
//! parse → cross-checks → channel → schema → version scope → rollback →
//! expiry. Nothing in the payload is trusted before the signature verifies.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};

use super::keys::{
    DOMAIN, ENVELOPE_VERSION, KeyStatus, MAX_ENVELOPE_BYTES, MAX_PAYLOAD_BYTES,
    SUPPORTED_SCHEMA_VERSION, TrustedKey, trusted_key,
};
use super::types::CloudFacts;

/// Grace after `not_after` before facts downgrade to `stale` (48 h).
pub const NOT_AFTER_GRACE_SECS: u64 = 48 * 60 * 60;

/// The transport envelope as served by `/api/facts/v1/<channel>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub envelope: u64,
    pub channel: String,
    pub facts_version: u64,
    #[serde(default)]
    pub schema_version: Option<u32>,
    pub key_id: String,
    pub alg: String,
    #[serde(default)]
    pub applies_to: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
    pub payload_b64: String,
    pub sig_b64: String,
    #[serde(default)]
    pub sigs: Vec<ExtraSignature>,
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Additional `(key_id, sig)` pairs carried during key rotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraSignature {
    pub key_id: String,
    pub sig_b64: String,
}

/// Why an envelope was not accepted. Every variant leaves bundled facts in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactsRejection {
    TooLarge { bytes: usize },
    BadEnvelope(String),
    UnknownKey { key_id: String },
    RetiredKey { key_id: String },
    BadSignature,
    BadPayload(String),
    Mismatch(String),
    WrongChannel { expected: String, got: String },
    SchemaTooNew { schema_version: u32 },
    BadVersionReq(String),
    NotApplicable { applies_to: String },
    Rollback { got: u64, highest_seen: u64 },
}

impl std::fmt::Display for FactsRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(f, "envelope too large ({bytes} bytes)"),
            Self::BadEnvelope(msg) => write!(f, "bad envelope: {msg}"),
            Self::UnknownKey { key_id } => write!(f, "unknown key {key_id}"),
            Self::RetiredKey { key_id } => write!(f, "retired key {key_id}"),
            Self::BadSignature => write!(f, "bad signature"),
            Self::BadPayload(msg) => write!(f, "bad payload: {msg}"),
            Self::Mismatch(msg) => write!(f, "envelope/payload mismatch: {msg}"),
            Self::WrongChannel { expected, got } => {
                write!(f, "wrong channel (expected {expected}, got {got})")
            }
            Self::SchemaTooNew { schema_version } => {
                write!(f, "schema_version {schema_version} newer than supported")
            }
            Self::BadVersionReq(req) => write!(f, "unparseable applies_to {req:?}"),
            Self::NotApplicable { applies_to } => {
                write!(f, "not applicable to this binary ({applies_to})")
            }
            Self::Rollback { got, highest_seen } => {
                write!(f, "rollback (v{got} < seen v{highest_seen})")
            }
        }
    }
}

/// A payload that passed every check.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedFacts {
    pub facts: CloudFacts,
    /// The pinned key that authenticated the payload.
    pub key_id: String,
    /// Hex SHA-256 of the exact signed bytes.
    pub sha256: String,
    pub raw_len: usize,
    /// `not_after` has passed by more than the grace window.
    pub stale: bool,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// The exact bytes an Ed25519 signature covers.
#[must_use]
pub fn signing_message(key_id: &str, payload: &[u8]) -> Vec<u8> {
    let mut msg = Vec::with_capacity(DOMAIN.len() + key_id.len() + 1 + payload.len());
    msg.extend_from_slice(DOMAIN);
    msg.extend_from_slice(key_id.as_bytes());
    msg.push(0);
    msg.extend_from_slice(payload);
    msg
}

fn ed25519_ok(public_key: &[u8; 32], message: &[u8], signature: &[u8]) -> bool {
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(message, signature)
        .is_ok()
}

/// Parse an RFC 3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SS[.fff]Z`) to unix seconds.
///
/// Offsets other than `Z` are not accepted (returns `None`); the publisher
/// always writes UTC.
#[must_use]
pub fn parse_rfc3339_utc(value: &str) -> Option<u64> {
    let value = value.trim();
    let (date, time) = value.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let time = time.split('.').next()?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    if d.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut t = time.split(':');
    let hour: u64 = t.next()?.parse().ok()?;
    let minute: u64 = t.next()?.parse().ok()?;
    let second: u64 = t.next()?.parse().ok()?;
    if t.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    if days < 0 {
        return None;
    }
    Some(days as u64 * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Verify a raw envelope document.
///
/// * `expected_channel` — the channel the client asked for.
/// * `current` — the running binary's version (`CARGO_PKG_VERSION`).
/// * `highest_seen` — highest `facts_version` this client previously accepted
///   for the channel (rollback protection); equal is accepted.
/// * `keys` — the pinned trust anchors (normally [`super::keys::TRUSTED_KEYS`]).
/// * `now_unix` — wall clock, injected for deterministic tests.
pub fn verify_envelope(
    bytes: &[u8],
    expected_channel: &str,
    current: &semver::Version,
    highest_seen: Option<u64>,
    keys: &[TrustedKey],
    now_unix: u64,
) -> Result<VerifiedFacts, FactsRejection> {
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(FactsRejection::TooLarge { bytes: bytes.len() });
    }
    let envelope: Envelope = serde_json::from_slice(bytes)
        .map_err(|err| FactsRejection::BadEnvelope(err.to_string()))?;
    if envelope.envelope != ENVELOPE_VERSION {
        return Err(FactsRejection::BadEnvelope(format!(
            "envelope version {} (supported {ENVELOPE_VERSION})",
            envelope.envelope
        )));
    }
    if envelope.alg != "ed25519" {
        return Err(FactsRejection::BadEnvelope(format!(
            "unsupported alg {:?}",
            envelope.alg
        )));
    }
    if envelope.payload_b64.len() > MAX_PAYLOAD_BYTES * 4 / 3 + 4 {
        return Err(FactsRejection::TooLarge {
            bytes: envelope.payload_b64.len(),
        });
    }
    let payload = BASE64
        .decode(envelope.payload_b64.as_bytes())
        .map_err(|err| FactsRejection::BadEnvelope(format!("payload_b64: {err}")))?;
    if payload.is_empty() {
        return Err(FactsRejection::BadEnvelope("empty payload".into()));
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(FactsRejection::TooLarge {
            bytes: payload.len(),
        });
    }

    // Candidate signatures: the primary first, then rotation extras. Accept
    // the first that verifies under a pinned, active key.
    let mut candidates: Vec<(&str, &str)> = Vec::with_capacity(1 + envelope.sigs.len());
    candidates.push((envelope.key_id.as_str(), envelope.sig_b64.as_str()));
    for extra in &envelope.sigs {
        candidates.push((extra.key_id.as_str(), extra.sig_b64.as_str()));
    }
    let mut saw_known = false;
    let mut saw_retired: Option<String> = None;
    let mut verified_key: Option<String> = None;
    for (key_id, sig_b64) in candidates {
        let Some(key) = trusted_key(keys, key_id) else {
            continue;
        };
        if key.status == KeyStatus::Retired {
            saw_retired.get_or_insert_with(|| key_id.to_string());
            continue;
        }
        saw_known = true;
        let Ok(signature) = BASE64.decode(sig_b64.as_bytes()) else {
            continue;
        };
        if signature.len() != 64 {
            continue;
        }
        let message = signing_message(key_id, &payload);
        if ed25519_ok(&key.public_key, &message, &signature) {
            verified_key = Some(key_id.to_string());
            break;
        }
    }
    let Some(key_id) = verified_key else {
        if saw_known {
            return Err(FactsRejection::BadSignature);
        }
        if let Some(key_id) = saw_retired {
            return Err(FactsRejection::RetiredKey { key_id });
        }
        return Err(FactsRejection::UnknownKey {
            key_id: envelope.key_id,
        });
    };

    // Only now is the payload trusted enough to parse.
    let facts: CloudFacts = serde_json::from_slice(&payload)
        .map_err(|err| FactsRejection::BadPayload(err.to_string()))?;
    if facts.channel != envelope.channel {
        return Err(FactsRejection::Mismatch(format!(
            "channel {:?} vs {:?}",
            envelope.channel, facts.channel
        )));
    }
    if facts.facts_version != envelope.facts_version {
        return Err(FactsRejection::Mismatch(format!(
            "facts_version {} vs {}",
            envelope.facts_version, facts.facts_version
        )));
    }
    if let Some(outer) = envelope.applies_to.as_deref()
        && outer != facts.applies_to
    {
        return Err(FactsRejection::Mismatch(format!(
            "applies_to {outer:?} vs {:?}",
            facts.applies_to
        )));
    }
    if let Some(outer) = envelope.schema_version
        && outer != facts.schema_version
    {
        return Err(FactsRejection::Mismatch(format!(
            "schema_version {outer} vs {}",
            facts.schema_version
        )));
    }
    if facts.channel != expected_channel {
        return Err(FactsRejection::WrongChannel {
            expected: expected_channel.to_string(),
            got: facts.channel,
        });
    }
    if facts.schema_version > SUPPORTED_SCHEMA_VERSION {
        return Err(FactsRejection::SchemaTooNew {
            schema_version: facts.schema_version,
        });
    }
    if !version_req_matches(&facts.applies_to, current)? {
        return Err(FactsRejection::NotApplicable {
            applies_to: facts.applies_to,
        });
    }
    if let Some(highest) = highest_seen
        && facts.facts_version < highest
    {
        return Err(FactsRejection::Rollback {
            got: facts.facts_version,
            highest_seen: highest,
        });
    }
    let stale = facts
        .not_after
        .as_deref()
        .and_then(parse_rfc3339_utc)
        .is_some_and(|not_after| now_unix > not_after.saturating_add(NOT_AFTER_GRACE_SECS));

    use sha2::Digest as _;
    let sha256 = hex(&sha2::Sha256::digest(&payload));
    Ok(VerifiedFacts {
        facts,
        key_id,
        sha256,
        raw_len: payload.len(),
        stale,
    })
}

/// Evaluate a Cargo-style semver requirement against the running version.
///
/// `*`/empty matches everything. Prerelease binaries (`0.9.12-beta.1`) only
/// match requirements that name a prerelease on the same `major.minor.patch`,
/// which is semver's rule; the beta channel must set `applies_to` explicitly.
pub fn version_req_matches(req: &str, current: &semver::Version) -> Result<bool, FactsRejection> {
    let trimmed = req.trim();
    if trimmed.is_empty() || trimmed == "*" {
        return Ok(true);
    }
    let parsed = semver::VersionReq::parse(trimmed)
        .map_err(|_| FactsRejection::BadVersionReq(trimmed.to_string()))?;
    Ok(parsed.matches(current))
}

/// Lenient per-item variant: unparseable requirements simply do not match.
#[must_use]
pub fn item_applies(req: Option<&str>, current: &semver::Version) -> bool {
    match req {
        None => true,
        Some(req) => version_req_matches(req, current).unwrap_or(false),
    }
}
