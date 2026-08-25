//! Antigravity (`agy`) `state.vscdb` token-envelope parsing.
//!
//! The official store persists the OAuth token under
//! `antigravityUnifiedStateSync.oauthToken` as a base64 protobuf envelope
//! (discovered against agy 1.1.13, 2026-08-24; shapes only — no secret
//! material is ever logged):
//!
//! ```text
//! base64(                                        // outer, urlsafe, unpadded
//!   field1: b"oauthTokenInfoSentinelKey"        // marker message
//!   field2: base64(                             // inner, urlsafe, unpadded
//!     field1: access token  ("ya29....", 260B)
//!     field2: b"Bearer"
//!     field3: refresh token ("1//....", 103B)
//!     field4: varint message { field1: expiry epoch seconds }
//!   )
//!   field1: b"authStateWithContextSentinel"     // marker message
//!   field2: {"state":"signedIn", ...} JSON
//! )
//! ```
//!
//! The same DB also carries `antigravityAuthStatus` (plain JSON with
//! `apiKey`/`email`/`name`), which is the credential the current app
//! actively maintains. This module exposes both readers; it never writes,
//! never refreshes, and never logs token values.

use base64::{
    Engine as _, engine::general_purpose::URL_SAFE, engine::general_purpose::URL_SAFE_NO_PAD,
};

/// ItemTable key holding the `agy` OAuth token envelope.
pub(crate) const AGY_OAUTH_TOKEN_KEY: &str = "antigravityUnifiedStateSync.oauthToken";

/// ItemTable key holding the current auth status JSON.
pub const AGY_AUTH_STATUS_KEY: &str = "antigravityAuthStatus";

/// The credential extracted from the external `agy` store.
///
/// The envelope also carries a refresh token in field 3. Codewhale parses
/// past it and never retains it: this import is read-only and never
/// refreshes the external store, so keeping the value would be secret
/// material held for no purpose.
pub(crate) struct AgyExternalCredential {
    /// Bearer access token (`ya29...` shape) — the sendable credential.
    pub access_token: String,
    /// Token expiry, Unix epoch seconds, when the envelope carries it.
    pub expires_at: Option<i64>,
}

/// Read the access token out of a parsed `antigravityAuthStatus` JSON value.
///
/// Shape (from the official store): `{"name", "email", "apiKey", ...}` where
/// `apiKey` holds the current bearer access token.
pub(crate) fn access_token_from_auth_status_json(raw: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let token = parsed.get("apiKey")?.as_str()?;
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Parse the `antigravityUnifiedStateSync.oauthToken` protobuf envelope.
pub(crate) fn parse_token_envelope(raw: &str) -> Option<AgyExternalCredential> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let outer = base64_urlsafe_decode(trimmed.as_bytes())?;
    // The outer value is one or more concatenated protobuf messages. Find
    // the `oauthTokenInfoSentinelKey` marker message and take the field that
    // follows it (field 2 of the same message).
    let fields = protobuf_fields(&outer)?;
    let mut access_token = None;
    let mut expires_at = None;
    let mut in_token_info = false;
    for (field_number, payload) in fields {
        if payload == b"oauthTokenInfoSentinelKey" && field_number == 1 {
            in_token_info = true;
            continue;
        }
        if in_token_info && field_number == 2 {
            // Inner base64 blob of the token-info message.
            let inner_blob = base64_urlsafe_decode(&payload)?;
            let inner_fields = protobuf_fields(&inner_blob)?;
            let nested = protobuf_fields(&inner_fields.first()?.1)?;
            for (num, value) in nested {
                match num {
                    1 if access_token.is_none() => {
                        access_token = Some(String::from_utf8_lossy(&value).to_string());
                    }
                    // Field 3 is the refresh token: parsed past, never kept.
                    4 => {
                        // field4 = nested message { field1: varint expiry }
                        if let Some(expiry_fields) = protobuf_fields(&value) {
                            for (n, v) in expiry_fields {
                                if n == 1 {
                                    expires_at = Some(varint_u64(&v)? as i64);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            in_token_info = false;
        }
    }
    let access_token = access_token?;
    if access_token.trim().is_empty() {
        return None;
    }
    Some(AgyExternalCredential {
        access_token,
        expires_at,
    })
}

// ── Minimal protobuf wire reader (length-delimited subset) ─────────────────

/// Decode unpadded urlsafe base64, tolerating standard-alphabet padding.
fn base64_urlsafe_decode(input: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(input).ok()?;
    let cleaned: String = text.chars().filter(|c| c != &'\n' && c != &'\r').collect();
    URL_SAFE_NO_PAD
        .decode(cleaned.trim_end_matches('='))
        .or_else(|_| URL_SAFE.decode(cleaned.as_str()))
        .ok()
}

/// Walk top-level protobuf fields; returns (field_number, payload) pairs for
/// wire-type-2 (length-delimited) entries and raw bytes for varints.
fn protobuf_fields(data: &[u8]) -> Option<Vec<(u64, Vec<u8>)>> {
    let mut fields = Vec::new();
    let mut cursor = 0usize;
    while cursor < data.len() {
        let key = varint_u64(&data[cursor..])?;
        cursor += varint_len(&data[cursor..])?;
        let field_number = key >> 3;
        match key & 7 {
            0 => {
                let len = varint_len(&data[cursor..])?;
                fields.push((field_number, data[cursor..cursor + len].to_vec()));
                cursor += len;
            }
            2 => {
                let len = varint_u64(&data[cursor..])? as usize;
                cursor += varint_len(&data[cursor..])?;
                let end = cursor.checked_add(len)?;
                if end > data.len() {
                    return None;
                }
                fields.push((field_number, data[cursor..end].to_vec()));
                cursor = end;
            }
            _ => return None, // the envelope only uses varint + length-delimited
        }
    }
    Some(fields)
}

fn varint_u64(bytes: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    for byte in bytes.iter().take(10) {
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

fn varint_len(bytes: &[u8]) -> Option<usize> {
    for (index, byte) in bytes.iter().enumerate().take(10) {
        if byte & 0x80 == 0 {
            return Some(index + 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fixture builders: synthetic shapes only, never real token material ──

    fn varint_bytes(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    /// Length-delimited (wire type 2) field.
    fn delimited(field_number: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = varint_bytes((field_number << 3) | 2);
        out.extend(varint_bytes(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    /// Varint (wire type 0) field.
    fn varint_field(field_number: u64, value: u64) -> Vec<u8> {
        let mut out = varint_bytes(field_number << 3);
        out.extend(varint_bytes(value));
        out
    }

    /// Build an envelope with the shape observed in the official store.
    fn fixture_envelope(access: &str, refresh: &str, expires_at: Option<u64>) -> String {
        let mut token_info = Vec::new();
        token_info.extend(delimited(1, access.as_bytes()));
        token_info.extend(delimited(2, b"Bearer"));
        token_info.extend(delimited(3, refresh.as_bytes()));
        if let Some(expiry) = expires_at {
            token_info.extend(delimited(4, &varint_field(1, expiry)));
        }
        // The inner blob wraps the token-info message in one outer field.
        let inner_blob = delimited(1, &token_info);
        let inner_b64 = URL_SAFE_NO_PAD.encode(&inner_blob);

        let mut outer = Vec::new();
        outer.extend(delimited(1, b"oauthTokenInfoSentinelKey"));
        outer.extend(delimited(2, inner_b64.as_bytes()));
        outer.extend(delimited(1, b"authStateWithContextSentinel"));
        outer.extend(delimited(2, br#"{"state":"signedIn"}"#));
        URL_SAFE_NO_PAD.encode(&outer)
    }

    #[test]
    fn parses_access_token_and_expiry_from_envelope() {
        let raw = fixture_envelope(
            "ya29.fixture-access",
            "1//fixture-refresh",
            Some(1_800_000_000),
        );
        let parsed = parse_token_envelope(&raw).expect("envelope parses");
        assert_eq!(parsed.access_token, "ya29.fixture-access");
        assert_eq!(parsed.expires_at, Some(1_800_000_000));
    }

    #[test]
    fn envelope_without_expiry_parses_with_no_deadline() {
        let raw = fixture_envelope("ya29.no-expiry", "1//fixture-refresh", None);
        let parsed = parse_token_envelope(&raw).expect("envelope parses");
        assert_eq!(parsed.access_token, "ya29.no-expiry");
        assert_eq!(parsed.expires_at, None);
    }

    #[test]
    fn envelope_never_surfaces_the_refresh_token() {
        // The credential type has no refresh-token field at all: the value is
        // parsed past and dropped, so it cannot leak into a request or a log.
        let raw = fixture_envelope("ya29.fixture-access", "1//fixture-refresh", None);
        let parsed = parse_token_envelope(&raw).expect("envelope parses");
        assert!(!parsed.access_token.contains("1//"));
    }

    #[test]
    fn non_envelope_values_are_not_envelopes() {
        assert!(parse_token_envelope("").is_none());
        assert!(parse_token_envelope("   ").is_none());
        // A bare token is a legacy shape, not an envelope.
        assert!(parse_token_envelope("ya29.bare-token").is_none());
        // Base64 that decodes but carries no sentinel marker.
        assert!(
            parse_token_envelope(&URL_SAFE_NO_PAD.encode(b"not a protobuf envelope")).is_none()
        );
    }

    #[test]
    fn auth_status_json_yields_the_api_key() {
        let raw =
            r#"{"name":"Fixture User","email":"user@example.com","apiKey":"ya29.status-key"}"#;
        assert_eq!(
            access_token_from_auth_status_json(raw),
            Some("ya29.status-key".to_string())
        );
    }

    #[test]
    fn auth_status_without_a_usable_key_is_absent() {
        // Not JSON at all (older builds stored a bare state string).
        assert_eq!(access_token_from_auth_status_json("signedIn"), None);
        // JSON with no apiKey member.
        assert_eq!(
            access_token_from_auth_status_json(r#"{"state":"signedIn"}"#),
            None
        );
        // Present but empty.
        assert_eq!(
            access_token_from_auth_status_json(r#"{"apiKey":"   "}"#),
            None
        );
    }
}
