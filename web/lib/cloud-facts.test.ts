import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  envelopeFromRow,
  etagFor,
  resolveCloudFacts,
  verifyEnvelope,
  type CloudFactsEnvelope,
  type FactsCurrentRow,
} from "./cloud-facts";
import { DOMAIN, TRUSTED_KEYS, type TrustedKey } from "./cloud-facts/keys";
import { responseFor } from "../app/api/facts/v1/[channel]/route";

const fixture = JSON.parse(
  readFileSync(new URL("../../docs/cloud-facts/fixtures/envelope-stable-v7.json", import.meta.url), "utf8"),
) as CloudFactsEnvelope;
const TEST_KEY: TrustedKey = { keyId: "cwf-test-only", publicKey: "8+FLDW4OorUETUVks0hpQAi5Lj4wg3kjKjfYFzLbJ7U=", status: "active" };

function rowFromFixture(overrides: Partial<FactsCurrentRow> = {}): FactsCurrentRow {
  return {
    channel: fixture.channel,
    release_id: "00000000-0000-0000-0000-000000000001",
    facts_version: fixture.facts_version,
    schema_version: fixture.schema_version,
    envelope_version: fixture.envelope,
    applies_to: fixture.applies_to,
    key_id: fixture.key_id,
    payload_b64: fixture.payload_b64,
    sig_b64: fixture.sig_b64,
    sigs: [],
    payload_sha256: fixture.sha256,
    published_at: fixture.published_at,
    not_after: null,
    ...overrides,
  };
}

function supabaseFetch(rows: unknown, status = 200): typeof fetch {
  return (async () => new Response(JSON.stringify(rows), { status, headers: { "Content-Type": "application/json" } })) as typeof fetch;
}

class MemKV {
  store = new Map<string, string>();
  async get(key: string) {
    return this.store.get(key) ?? null;
  }
  async put(key: string, value: string) {
    this.store.set(key, value);
  }
  async list() {
    return { keys: [...this.store.keys()].map((name) => ({ name })) };
  }
  async delete(key: string) {
    this.store.delete(key);
  }
}

const env = { SUPABASE_URL: "https://example.supabase.co", SUPABASE_PUBLISHABLE_KEY: "sb_publishable_test" };

describe("cloud facts verification", () => {
  it("verifies the node-signed fixture under the test-only key", async () => {
    const result = await verifyEnvelope(fixture, [TEST_KEY]);
    expect(result).toEqual({ ok: true, keyId: "cwf-test-only", mode: "verified" });
  });

  it("rejects a tampered signature, an unknown key and a retired key", async () => {
    const badSig = { ...fixture, sig_b64: `A${fixture.sig_b64.slice(1)}` };
    expect(await verifyEnvelope(badSig, [TEST_KEY])).toEqual({ ok: false, reason: "bad-signature" });
    expect(await verifyEnvelope(fixture, [{ ...TEST_KEY, keyId: "cwf-other" }])).toEqual({ ok: false, reason: "unknown-key" });
    expect(await verifyEnvelope(fixture, [{ ...TEST_KEY, status: "retired" }, { ...TEST_KEY, keyId: "cwf-next" }])).toEqual({ ok: false, reason: "retired-key" });
    expect(await verifyEnvelope({ ...fixture, alg: "rsa" as "ed25519" }, [TEST_KEY])).toEqual({ ok: false, reason: "bad-envelope" });
  });

  it("skips (and reports) verification only when no key is pinned at all", async () => {
    expect(await verifyEnvelope(fixture, [])).toEqual({ ok: true, keyId: "cwf-test-only", mode: "skipped-no-keys" });
  });

  it("signs the key id into the message (domain separation)", () => {
    expect(DOMAIN).toBe("codewhale-facts/v1\0");
  });

  it("pins at least one active key that is not the test-only key", () => {
    expect(TRUSTED_KEYS.some((k) => k.status === "active")).toBe(true);
    expect(TRUSTED_KEYS.some((k) => k.publicKey === TEST_KEY.publicKey)).toBe(false);
  });
});

describe("resolveCloudFacts", () => {
  it("serves a verified row from Supabase and writes the KV last-good copy", async () => {
    const kv = new MemKV();
    const result = await resolveCloudFacts("stable", { ...env, CURATED_KV: kv }, { fetchImpl: supabaseFetch([rowFromFixture()]), keys: [TEST_KEY] });
    expect(result.kind).toBe("ok");
    if (result.kind !== "ok") return;
    expect(result.source).toBe("supabase");
    expect(result.verified).toBe("verified");
    expect(result.etag).toBe(`"stable-v7-${fixture.sha256.slice(0, 16)}"`);
    expect(kv.store.get("facts:cloud:stable")).toBe(result.body);
    const served = JSON.parse(result.body) as CloudFactsEnvelope;
    expect(served.payload_b64).toBe(fixture.payload_b64);
    expect(served.sig_b64).toBe(fixture.sig_b64);
  });

  it("returns none for an empty channel or an invalid slug", async () => {
    expect((await resolveCloudFacts("stable", env, { fetchImpl: supabaseFetch([]), keys: [TEST_KEY] })).kind).toBe("none");
    expect((await resolveCloudFacts("Bad Slug", env, { fetchImpl: supabaseFetch([]), keys: [TEST_KEY] })).kind).toBe("none");
  });

  it("refuses to serve a row whose stored digest does not match its bytes", async () => {
    const result = await resolveCloudFacts("stable", env, {
      fetchImpl: supabaseFetch([rowFromFixture({ payload_sha256: "0".repeat(64) })]),
      keys: [TEST_KEY],
    });
    expect(result.kind).toBe("sha-mismatch");
  });

  it("refuses to serve a row that does not verify under the pinned keys", async () => {
    const result = await resolveCloudFacts("stable", env, {
      fetchImpl: supabaseFetch([rowFromFixture({ sig_b64: `A${fixture.sig_b64.slice(1)}` })]),
      keys: [TEST_KEY],
    });
    expect(result).toMatchObject({ kind: "unverifiable", reason: "bad-signature" });
  });

  it("falls back to the KV last-good copy when Supabase fails, else unavailable", async () => {
    const kv = new MemKV();
    const failing = (async () => new Response("down", { status: 503 })) as typeof fetch;
    expect((await resolveCloudFacts("stable", { ...env, CURATED_KV: kv }, { fetchImpl: failing, keys: [TEST_KEY] })).kind).toBe("unavailable");
    await kv.put("facts:cloud:stable", JSON.stringify(envelopeFromRow(rowFromFixture())));
    const result = await resolveCloudFacts("stable", { ...env, CURATED_KV: kv }, { fetchImpl: failing, keys: [TEST_KEY] });
    expect(result).toMatchObject({ kind: "ok", source: "kv-stale", verified: "verified" });
    // Missing configuration is also "unavailable", never a thrown error.
    expect((await resolveCloudFacts("stable", {}, { keys: [TEST_KEY] })).kind).toBe("unavailable");
  });
});

describe("/api/facts/v1/[channel] responses", () => {
  const envelope = envelopeFromRow(rowFromFixture());
  const ok = { kind: "ok" as const, envelope, body: JSON.stringify(envelope), etag: etagFor(envelope), source: "supabase" as const, verified: "verified" as const };
  const req = (headers: Record<string, string> = {}) => new Request("https://codewhale.net/api/facts/v1/stable", { headers });

  it("200 carries a strong ETag, CDN cache headers, and no cookies or Vary", async () => {
    const res = responseFor(ok, req(), "stable", "GET");
    expect(res.status).toBe(200);
    expect(res.headers.get("etag")).toBe(ok.etag);
    expect(res.headers.get("cache-control")).toContain("s-maxage=300");
    expect(res.headers.get("cache-control")).toContain("stale-if-error=604800");
    expect(res.headers.get("x-facts-version")).toBe("7");
    expect(res.headers.get("x-facts-verified")).toBe("verified");
    expect(res.headers.get("access-control-allow-origin")).toBe("*");
    expect(res.headers.get("set-cookie")).toBeNull();
    expect(res.headers.get("vary")).toBeNull();
    expect(JSON.parse(await res.text()).sha256).toBe(fixture.sha256);
  });

  it("304 on a matching If-None-Match (weak validators tolerated)", () => {
    expect(responseFor(ok, req({ "if-none-match": ok.etag }), "stable", "GET").status).toBe(304);
    expect(responseFor(ok, req({ "if-none-match": `W/${ok.etag}` }), "stable", "GET").status).toBe(304);
    expect(responseFor(ok, req({ "if-none-match": '"other"' }), "stable", "GET").status).toBe(200);
  });

  it("HEAD returns headers without a body", async () => {
    const res = responseFor(ok, req(), "stable", "HEAD");
    expect(res.status).toBe(200);
    expect(res.headers.get("etag")).toBe(ok.etag);
    expect(await res.text()).toBe("");
  });

  it("maps failure kinds to 404 / 502 / 503 with Retry-After and no-store", async () => {
    expect(responseFor({ kind: "none" }, req(), "stable", "GET").status).toBe(404);
    const sha = responseFor({ kind: "sha-mismatch", channel: "stable", factsVersion: 7 }, req(), "stable", "GET");
    expect(sha.status).toBe(502);
    expect(sha.headers.get("cache-control")).toBe("no-store");
    const bad = responseFor({ kind: "unverifiable", reason: "bad-signature", channel: "stable", factsVersion: 7 }, req(), "stable", "GET");
    expect(bad.status).toBe(503);
    expect(bad.headers.get("retry-after")).toBe("600");
    expect(JSON.parse(await bad.text()).error).toBe("facts-unverifiable");
    const down = responseFor({ kind: "unavailable", reason: "supabase-http-503" }, req(), "stable", "GET");
    expect(down.status).toBe(503);
    expect(down.headers.get("retry-after")).toBe("600");
  });
});
