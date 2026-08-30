//! Trust anchors for the cloud facts channel.
//!
//! Keys are pinned in the binary. The Supabase `facts_key` table and the
//! website mirror (`web/lib/cloud-facts/keys.ts`) are informational; a facts
//! envelope is accepted only when its signature verifies under an `Active` key
//! listed here. `web/scripts/check-cloud-facts.mjs` fails CI if this table and
//! the TypeScript mirror diverge.
//!
//! Rotation (two-release rule): pin the new key here → ship → sign with both
//! keys (`sigs`) → mark the old key `Retired` → ship → drop it. Compromise:
//! revoke every release signed by the key server-side, ship a binary without
//! the key. There is deliberately no in-band "distrust this key" message.

/// Domain separator prefixed to every signed message.
///
/// Message = `DOMAIN || key_id || 0x00 || payload_bytes`.
pub const DOMAIN: &[u8] = b"codewhale-facts/v1\0";

/// Transport envelope version this client understands.
pub const ENVELOPE_VERSION: u64 = 1;

/// Highest signed-payload `schema_version` this client understands. Newer
/// payloads are rejected as `SchemaTooNew` and the bundled facts stay in use.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Hard cap on the decoded payload; enforced before any crypto runs.
pub const MAX_PAYLOAD_BYTES: usize = 512 * 1024;

/// Hard cap on the raw envelope document (payload base64 + metadata).
pub const MAX_ENVELOPE_BYTES: usize = 768 * 1024;

/// Whether a pinned key may still authenticate new releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    /// Accepts signatures.
    Active,
    /// Still listed so `/status` can name it, but no longer accepted.
    Retired,
}

/// One pinned Ed25519 verifying key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedKey {
    /// `cwf-<label>`; also part of the signed message.
    pub key_id: &'static str,
    /// Raw 32-byte Ed25519 public key.
    pub public_key: [u8; 32],
    pub status: KeyStatus,
}

/// Keys the shipped binary trusts.
///
/// `cwf-dogfood-2026-08` is a throwaway dogfood key generated on 2026-08-30 to
/// prove the channel end to end. Its private half lives outside every
/// repository (founder custody). Rotate before customer-facing use.
pub const TRUSTED_KEYS: &[TrustedKey] = &[TrustedKey {
    key_id: "cwf-dogfood-2026-08",
    // std base64: MfA1//Uvi7rjlUEh8fuem8SHpqMoGnWEJxsfhcbEPX8=
    public_key: [
        49, 240, 53, 255, 245, 47, 139, 186, 227, 149, 65, 33, 241, 251, 158, 155, 196, 135, 166,
        163, 40, 26, 117, 132, 39, 27, 31, 133, 198, 196, 61, 127,
    ],
    status: KeyStatus::Active,
}];

/// Look up a pinned key by id.
#[must_use]
pub fn trusted_key<'a>(keys: &'a [TrustedKey], key_id: &str) -> Option<&'a TrustedKey> {
    keys.iter().find(|key| key.key_id == key_id)
}
