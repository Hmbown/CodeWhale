//! Antigravity (`agy`) `state.vscdb` token-envelope parsing.
//!
//! The official store persists the OAuth token under
//! `antigravityUnifiedStateSync.oauthToken` as a base64 protobuf envelope
//! (discovered against agy 1.1.13, 2026-08-24; shapes only — no secret
//! material is ever logged):
//!
//! ```text
//! base64(                                    // outer, urlsafe, unpadded
//!   field1: entry {                          // one entry per sentinel
//!     field1: b"oauthTokenInfoSentinelKey"   // which entry this is
//!     field2: holder {
//!       field1: base64(                      // inner, urlsafe, unpadded
//!         field1: access token  ("ya29....", 260B)
//!         field2: b"Bearer"
//!         field3: refresh token ("1//....", 103B)
//!         field4: message { field1: varint expiry, epoch seconds }
//!       )
//!     }
//!   }
//!   field1: entry {
//!     field1: b"authStateWithContextSentinelKey"
//!     field2: holder { field1: base64({"state":"signedIn", ...}) }
//!   }
//! )
//! ```
//!
//! Each entry is a nested message, not a pair of sibling fields on the outer
//! message — verified by decoding a real store on 2026-08-25, where the
//! entries were 545 and 249 bytes and the token-info blob was 384 bytes.
//!
//! The same DB also carries `antigravityAuthStatus` (plain JSON with
//! `apiKey`/`email`/`name`), which is the credential the current app
//! actively maintains. This module exposes both readers; it never writes,
//! never refreshes, and never logs token values.

use base64::{
    Engine as _, engine::general_purpose::STANDARD, engine::general_purpose::STANDARD_NO_PAD,
    engine::general_purpose::URL_SAFE, engine::general_purpose::URL_SAFE_NO_PAD,
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

/// Marker naming the entry that carries the OAuth token.
const OAUTH_TOKEN_SENTINEL: &[u8] = b"oauthTokenInfoSentinelKey";

/// Whether a stored value is recognisably this envelope.
///
/// Used to fail closed: a value that is an envelope but does not parse must
/// never fall through to the legacy bare-token reader, which would hand the
/// whole base64 blob back as if it were a credential.
pub(crate) fn looks_like_envelope(raw: &str) -> bool {
    base64_decode_any(raw.trim().as_bytes())
        .is_some_and(|bytes| contains_window(&bytes, OAUTH_TOKEN_SENTINEL))
}

fn contains_window(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Parse the `antigravityUnifiedStateSync.oauthToken` protobuf envelope.
pub(crate) fn parse_token_envelope(raw: &str) -> Option<AgyExternalCredential> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let outer = base64_decode_any(trimmed.as_bytes())?;
    // The outer message holds one nested entry per sentinel. Each entry names
    // itself in field 1 and carries its payload in field 2.
    for (_, entry) in protobuf_fields(&outer)? {
        let Some(entry_fields) = protobuf_fields(&entry) else {
            continue;
        };
        let names_token_info = entry_fields
            .iter()
            .any(|(number, value)| *number == 1 && value.as_slice() == OAUTH_TOKEN_SENTINEL);
        if !names_token_info {
            continue;
        }
        let holder = entry_fields
            .iter()
            .find(|(number, _)| *number == 2)
            .map(|(_, value)| value)?;
        // The holder wraps a single base64 string which decodes directly to
        // the token-info message.
        let blob = protobuf_fields(holder)?
            .into_iter()
            .find(|(number, _)| *number == 1)
            .map(|(_, value)| value)?;
        let info = base64_decode_any(&blob)?;
        return credential_from_token_info(&info);
    }
    None
}

/// Read the token-info message: field 1 is the access token, field 3 is the
/// refresh token (parsed past, never kept), field 4 wraps the expiry varint.
fn credential_from_token_info(info: &[u8]) -> Option<AgyExternalCredential> {
    let mut access_token = None;
    let mut expires_at = None;
    for (number, value) in protobuf_fields(info)? {
        match number {
            1 if access_token.is_none() => {
                access_token = Some(String::from_utf8_lossy(&value).into_owned());
            }
            // Field 2 is the literal "Bearer"; field 3 is the refresh token.
            4 => {
                for (inner, raw) in protobuf_fields(&value)? {
                    if inner == 1 {
                        expires_at = Some(varint_u64(&raw)? as i64);
                    }
                }
            }
            _ => {}
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

/// Decode base64 in whichever alphabet the store happens to use.
///
/// The real store writes STANDARD base64 — padded, and using `+` — while the
/// inner token blob is unpadded. An earlier version of this function only
/// tried the URL-safe alphabets, which reject `+`, so the outer envelope
/// never decoded at all and every sign-in fell through to the legacy
/// bare-token reader. Try all four rather than betting on one.
fn base64_decode_any(input: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(input).ok()?;
    let cleaned: String = text.chars().filter(|c| c != &'\n' && c != &'\r').collect();
    let unpadded = cleaned.trim_end_matches('=');
    STANDARD
        .decode(&cleaned)
        .or_else(|_| STANDARD_NO_PAD.decode(unpadded))
        .or_else(|_| URL_SAFE.decode(&cleaned))
        .or_else(|_| URL_SAFE_NO_PAD.decode(unpadded))
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

    /// Build an envelope with the nesting decoded from a real store on
    /// 2026-08-25: the outer message holds one nested entry per sentinel,
    /// each naming itself in field 1 and holding its payload in field 2.
    fn fixture_envelope(access: &str, refresh: &str, expires_at: Option<u64>) -> String {
        let mut token_info = Vec::new();
        token_info.extend(delimited(1, access.as_bytes()));
        token_info.extend(delimited(2, b"Bearer"));
        token_info.extend(delimited(3, refresh.as_bytes()));
        if let Some(expiry) = expires_at {
            token_info.extend(delimited(4, &varint_field(1, expiry)));
        }
        // The holder carries one base64 string that decodes straight to the
        // token-info message.
        let blob = URL_SAFE_NO_PAD.encode(&token_info);
        let holder = delimited(1, blob.as_bytes());

        let mut token_entry = Vec::new();
        token_entry.extend(delimited(1, b"oauthTokenInfoSentinelKey"));
        token_entry.extend(delimited(2, &holder));

        let state_blob = URL_SAFE_NO_PAD.encode(br#"{"state":"signedIn"}"#);
        let mut state_entry = Vec::new();
        state_entry.extend(delimited(1, b"authStateWithContextSentinelKey"));
        state_entry.extend(delimited(2, &delimited(1, state_blob.as_bytes())));

        let mut outer = Vec::new();
        outer.extend(delimited(1, &token_entry));
        outer.extend(delimited(1, &state_entry));
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
    fn an_envelope_is_recognised_even_when_it_does_not_parse() {
        // The fail-closed hook: a value carrying the sentinel is an envelope,
        // so a parse failure must never be handed to the legacy bare-token
        // reader, which would return the whole blob as a "credential".
        let raw = fixture_envelope("ya29.fixture-access", "1//fixture-refresh", None);
        assert!(looks_like_envelope(&raw));
        assert!(!looks_like_envelope("ya29.bare-token"));
        assert!(!looks_like_envelope(""));
        assert!(!looks_like_envelope(
            &URL_SAFE_NO_PAD.encode(b"some other base64 payload")
        ));
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
