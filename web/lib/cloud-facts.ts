/**
 * Cloud facts (facts/v1) delivery: read the latest published release for a
 * channel from Supabase (PostgREST, publishable key only), rebuild the signed
 * envelope, recompute the payload digest, verify the Ed25519 signature against
 * the pinned keys, and hand the route a cacheable body.
 *
 * The edge never amplifies a bad row: a digest mismatch or a signature that
 * fails under the pinned keys is a 5xx, not a served envelope. With an empty
 * key table verification is skipped and reported in `X-Facts-Verified`.
 *
 * Host-agnostic: plain `fetch`, no supabase-js. KV (when the Cloudflare
 * binding is present) keeps a last-good copy per channel so a Supabase outage
 * degrades to stale-but-signed facts instead of nothing.
 */
import { DOMAIN, MAX_PAYLOAD_BYTES, TRUSTED_KEYS, type TrustedKey } from "./cloud-facts/keys";
import type { KVNamespace } from "./kv";

export const CHANNEL_RE = /^[a-z0-9][a-z0-9-]{0,31}$/;
export const KV_PREFIX = "facts:cloud:";
export const SUPABASE_TIMEOUT_MS = 3000;
const KV_TTL_SECS = 60 * 60 * 24 * 30;

export interface FactsCurrentRow {
  channel: string;
  release_id: string;
  facts_version: number;
  schema_version: number;
  envelope_version: number;
  applies_to: string;
  key_id: string;
  payload_b64: string;
  sig_b64: string;
  sigs: { key_id: string; sig_b64: string }[] | null;
  payload_sha256: string;
  published_at: string;
  not_after: string | null;
}

export interface CloudFactsEnvelope {
  envelope: number;
  channel: string;
  facts_version: number;
  schema_version: number;
  key_id: string;
  alg: "ed25519";
  applies_to: string;
  published_at: string;
  not_after?: string | null;
  payload_b64: string;
  sig_b64: string;
  sigs: { key_id: string; sig_b64: string }[];
  sha256: string;
}

export interface CloudFactsEnv {
  SUPABASE_URL?: string;
  SUPABASE_PUBLISHABLE_KEY?: string;
  CURATED_KV?: KVNamespace;
}

export type Verification =
  | { ok: true; keyId: string; mode: "verified" }
  | { ok: true; keyId: string; mode: "skipped-no-keys" }
  | { ok: false; reason: "unknown-key" | "retired-key" | "bad-signature" | "bad-envelope" };

export type CloudFactsResult =
  | {
      kind: "ok";
      envelope: CloudFactsEnvelope;
      body: string;
      etag: string;
      source: "supabase" | "kv-stale";
      verified: "verified" | "skipped-no-keys";
    }
  | { kind: "none" }
  | { kind: "sha-mismatch"; channel: string; factsVersion: number }
  | { kind: "unverifiable"; reason: string; channel: string; factsVersion: number }
  | { kind: "unavailable"; reason: string };

export interface ResolveOptions {
  fetchImpl?: typeof fetch;
  keys?: readonly TrustedKey[];
  timeoutMs?: number;
  now?: () => number;
}

export function isValidChannel(slug: string): boolean {
  return CHANNEL_RE.test(slug);
}

function b64ToBytes(b64: string): Uint8Array {
  if (typeof Buffer !== "undefined") return new Uint8Array(Buffer.from(b64, "base64"));
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return out;
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes as BufferSource);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export function signingMessage(keyId: string, payload: Uint8Array): Uint8Array {
  const enc = new TextEncoder();
  const domain = enc.encode(DOMAIN);
  const id = enc.encode(keyId);
  const out = new Uint8Array(domain.length + id.length + 1 + payload.length);
  out.set(domain, 0);
  out.set(id, domain.length);
  out[domain.length + id.length] = 0;
  out.set(payload, domain.length + id.length + 1);
  return out;
}

async function ed25519Verify(publicKeyB64: string, message: Uint8Array, signature: Uint8Array): Promise<boolean> {
  try {
    const key = await crypto.subtle.importKey("raw", b64ToBytes(publicKeyB64) as BufferSource, { name: "Ed25519" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "Ed25519" }, key, signature as BufferSource, message as BufferSource);
  } catch {
    return false;
  }
}

function isRow(value: unknown): value is FactsCurrentRow {
  if (!value || typeof value !== "object") return false;
  const row = value as Record<string, unknown>;
  return (
    typeof row.channel === "string" &&
    typeof row.facts_version === "number" &&
    typeof row.key_id === "string" &&
    typeof row.payload_b64 === "string" &&
    typeof row.sig_b64 === "string" &&
    typeof row.payload_sha256 === "string"
  );
}

export function envelopeFromRow(row: FactsCurrentRow): CloudFactsEnvelope {
  return {
    envelope: row.envelope_version ?? 1,
    channel: row.channel,
    facts_version: row.facts_version,
    schema_version: row.schema_version ?? 1,
    key_id: row.key_id,
    alg: "ed25519",
    applies_to: row.applies_to ?? "*",
    published_at: row.published_at,
    not_after: row.not_after ?? null,
    payload_b64: row.payload_b64,
    sig_b64: row.sig_b64,
    sigs: Array.isArray(row.sigs) ? row.sigs : [],
    sha256: row.payload_sha256,
  };
}

export function etagFor(envelope: CloudFactsEnvelope): string {
  return `"${envelope.channel}-v${envelope.facts_version}-${envelope.sha256.slice(0, 16)}"`;
}

/** Verify the envelope under `keys`. Empty key table → skipped (reported). */
export async function verifyEnvelope(envelope: CloudFactsEnvelope, keys: readonly TrustedKey[] = TRUSTED_KEYS): Promise<Verification> {
  if (envelope.envelope !== 1 || envelope.alg !== "ed25519") return { ok: false, reason: "bad-envelope" };
  const payload = b64ToBytes(envelope.payload_b64);
  if (payload.length === 0 || payload.length > MAX_PAYLOAD_BYTES) return { ok: false, reason: "bad-envelope" };
  // Skip only when the key table is empty; a retired-only table still rejects.
  if (keys.length === 0) return { ok: true, keyId: envelope.key_id, mode: "skipped-no-keys" };
  const candidates = [{ key_id: envelope.key_id, sig_b64: envelope.sig_b64 }, ...(envelope.sigs ?? [])];
  let sawKnown = false;
  let sawRetired = false;
  for (const candidate of candidates) {
    const key = keys.find((k) => k.keyId === candidate.key_id);
    if (!key) continue;
    if (key.status !== "active") {
      sawRetired = true;
      continue;
    }
    sawKnown = true;
    const sig = b64ToBytes(candidate.sig_b64);
    if (sig.length !== 64) continue;
    if (await ed25519Verify(key.publicKey, signingMessage(candidate.key_id, payload), sig)) {
      return { ok: true, keyId: candidate.key_id, mode: "verified" };
    }
  }
  if (sawKnown) return { ok: false, reason: "bad-signature" };
  if (sawRetired) return { ok: false, reason: "retired-key" };
  return { ok: false, reason: "unknown-key" };
}

/** Read the current row for `channel` via PostgREST. Throws on transport/HTTP errors. */
export async function fetchCurrentRow(channel: string, env: CloudFactsEnv, opts: ResolveOptions = {}): Promise<FactsCurrentRow | null> {
  const base = env.SUPABASE_URL?.replace(/\/+$/, "");
  const key = env.SUPABASE_PUBLISHABLE_KEY;
  if (!base || !key) throw new Error("supabase-not-configured");
  const fetchImpl = opts.fetchImpl ?? fetch;
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), opts.timeoutMs ?? SUPABASE_TIMEOUT_MS);
  try {
    const url =
      `${base}/rest/v1/facts_current?channel=eq.${encodeURIComponent(channel)}&scope=eq.global` +
      `&select=channel,release_id,facts_version,schema_version,envelope_version,applies_to,key_id,payload_b64,sig_b64,sigs,payload_sha256,published_at,not_after&limit=1`;
    const res = await fetchImpl(url, {
      headers: { apikey: key, Authorization: `Bearer ${key}`, Accept: "application/json" },
      signal: controller.signal,
    });
    if (!res.ok) throw new Error(`supabase-http-${res.status}`);
    const rows: unknown = await res.json();
    if (!Array.isArray(rows) || rows.length === 0) return null;
    if (!isRow(rows[0])) throw new Error("supabase-bad-row");
    return rows[0];
  } finally {
    clearTimeout(timer);
  }
}

async function kvGet(env: CloudFactsEnv, channel: string): Promise<CloudFactsEnvelope | null> {
  if (!env.CURATED_KV) return null;
  try {
    const raw = await env.CURATED_KV.get(`${KV_PREFIX}${channel}`);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as CloudFactsEnvelope;
    return typeof parsed?.payload_b64 === "string" && typeof parsed?.sha256 === "string" ? parsed : null;
  } catch {
    return null;
  }
}

async function kvPut(env: CloudFactsEnv, channel: string, body: string): Promise<void> {
  if (!env.CURATED_KV) return;
  try {
    await env.CURATED_KV.put(`${KV_PREFIX}${channel}`, body, { expirationTtl: KV_TTL_SECS });
  } catch {
    // last-good is best effort
  }
}

/** Resolve the servable envelope for `channel`. */
export async function resolveCloudFacts(channel: string, env: CloudFactsEnv, opts: ResolveOptions = {}): Promise<CloudFactsResult> {
  if (!isValidChannel(channel)) return { kind: "none" };
  const keys = opts.keys ?? TRUSTED_KEYS;
  let row: FactsCurrentRow | null;
  try {
    row = await fetchCurrentRow(channel, env, opts);
  } catch (err) {
    const reason = err instanceof Error ? err.message : "supabase-error";
    const stale = await kvGet(env, channel);
    if (!stale) return { kind: "unavailable", reason };
    const verification = await verifyEnvelope(stale, keys);
    if (!verification.ok) return { kind: "unavailable", reason: `kv-${verification.reason}` };
    const body = JSON.stringify(stale);
    return { kind: "ok", envelope: stale, body, etag: etagFor(stale), source: "kv-stale", verified: verification.mode };
  }
  if (!row) return { kind: "none" };
  const envelope = envelopeFromRow(row);
  const digest = await sha256Hex(b64ToBytes(envelope.payload_b64));
  if (digest !== envelope.sha256) return { kind: "sha-mismatch", channel, factsVersion: envelope.facts_version };
  const verification = await verifyEnvelope(envelope, keys);
  if (!verification.ok) return { kind: "unverifiable", reason: verification.reason, channel, factsVersion: envelope.facts_version };
  const body = JSON.stringify(envelope);
  await kvPut(env, channel, body);
  return { kind: "ok", envelope, body, etag: etagFor(envelope), source: "supabase", verified: verification.mode };
}

/** Summary block for the existing `/api/facts` receipt (additive). */
export async function cloudFactsSummary(env: CloudFactsEnv, channel = "stable"): Promise<
  { channel: string; factsVersion: number; publishedAt: string; keyId: string; source: string } | null
> {
  try {
    const result = await resolveCloudFacts(channel, env);
    if (result.kind !== "ok") return null;
    return {
      channel: result.envelope.channel,
      factsVersion: result.envelope.facts_version,
      publishedAt: result.envelope.published_at,
      keyId: result.envelope.key_id,
      source: result.source,
    };
  } catch {
    return null;
  }
}
