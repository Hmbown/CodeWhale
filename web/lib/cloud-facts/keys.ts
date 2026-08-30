/**
 * Ed25519 verifying keys for the CodeWhale cloud facts channel (facts/v1).
 *
 * Mirror of `crates/config/src/cloud_facts/keys.rs` — `check:facts` fails if
 * the two diverge. Keys are pinned here (and in the binary); the Supabase
 * `facts_key` table is informational and never a trust root.
 *
 * `cwf-dogfood-2026-08` is a throwaway dogfood key generated on 2026-08-30 so
 * the channel could be proven end to end. Its private half lives outside every
 * repository (founder custody). Rotate it before relying on the channel for
 * customer-facing facts: pin the new key here and in keys.rs, ship a release,
 * then retire this one.
 */
export type KeyStatus = "active" | "retired";

export interface TrustedKey {
  keyId: string;
  /** Standard base64 of the raw 32-byte Ed25519 public key. */
  publicKey: string;
  status: KeyStatus;
}

export const DOMAIN = "codewhale-facts/v1\0";
export const ENVELOPE_VERSION = 1;
export const SUPPORTED_SCHEMA_VERSION = 1;
export const MAX_PAYLOAD_BYTES = 512 * 1024;

export const TRUSTED_KEYS: readonly TrustedKey[] = [
  { keyId: "cwf-dogfood-2026-08", publicKey: "MfA1//Uvi7rjlUEh8fuem8SHpqMoGnWEJxsfhcbEPX8=", status: "active" },
];

export function trustedKey(keyId: string): TrustedKey | undefined {
  return TRUSTED_KEYS.find((key) => key.keyId === keyId);
}
