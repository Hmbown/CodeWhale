#!/usr/bin/env node
/**
 * CodeWhale cloud facts (facts/v1) authoring tool. Zero npm dependencies.
 *
 *   node scripts/facts-publish.mjs keygen  --key-id cwf-2026-09 --out /secure/path.key
 *   node scripts/facts-publish.mjs sign    --source ../docs/cloud-facts/stable.json --channel stable \
 *                                          --key-id cwf-2026-09 [--facts-version N] [--out envelope.json]
 *   node scripts/facts-publish.mjs verify  envelope.json [--public-key <base64>]
 *   node scripts/facts-publish.mjs emit-sql envelope.json [--published-by who] [--public-key <base64>]
 *   node scripts/facts-publish.mjs publish envelope.json [--dry-run] [--published-by who]
 *   node scripts/facts-publish.mjs revoke  --channel stable --version N --reason "..." [--dry-run]
 *
 * Secrets are read ONLY from the environment at sign/publish time and are never
 * printed:
 *   CODEWHALE_FACTS_SIGNING_KEY       PEM (PKCS#8) Ed25519 private key contents
 *   CODEWHALE_FACTS_SIGNING_KEY_FILE  path to that PEM (alternative)
 *   SUPABASE_URL                      https://<ref>.supabase.co  (publish/revoke)
 *   SUPABASE_SERVICE_ROLE_KEY         service-role key (publish/revoke only; never embed)
 *
 * Signing contract (must match crates/config/src/cloud_facts/verify.rs and
 * web/lib/cloud-facts.ts): Ed25519 detached signature over
 *   "codewhale-facts/v1\0" || key_id || "\0" || payload_bytes
 * where payload_bytes is canonical JSON (sorted keys, no whitespace, UTF-8).
 * Clients verify the exact bytes carried in payload_b64; they never re-canonicalize.
 */
import { createPrivateKey, createPublicKey, generateKeyPairSync, sign, verify, createHash } from "node:crypto";
import { readFileSync, writeFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const DOMAIN = "codewhale-facts/v1\0";
export const ENVELOPE_VERSION = 1;
export const SCHEMA_VERSION = 1;
export const MAX_PAYLOAD_BYTES = 512 * 1024;
const KEY_ID_RE = /^cwf-[a-z0-9-]{1,32}$/;
const CHANNEL_RE = /^[a-z0-9][a-z0-9-]{0,31}$/;
const CI_MARKERS = ["CI", "GITHUB_ACTIONS", "GITLAB_CI", "BUILDKITE", "CIRCLECI", "JENKINS_URL", "TF_BUILD"];

const here = dirname(fileURLToPath(import.meta.url));
const WEB_ROOT = resolve(here, "..");
const REPO_ROOT = resolve(WEB_ROOT, "..");

// ---------------------------------------------------------------------------
// Canonical JSON + signing primitives (exported for tests)
// ---------------------------------------------------------------------------

export function canonicalize(value) {
  if (value === null || typeof value !== "object") {
    if (typeof value === "number" && !Number.isFinite(value)) {
      throw new Error("non-finite number in payload");
    }
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalize).join(",")}]`;
  const keys = Object.keys(value).sort();
  const parts = [];
  for (const key of keys) {
    const v = value[key];
    if (v === undefined) continue;
    parts.push(`${JSON.stringify(key)}:${canonicalize(v)}`);
  }
  return `{${parts.join(",")}}`;
}

export function signingMessage(keyId, payloadBytes) {
  return Buffer.concat([Buffer.from(DOMAIN, "utf8"), Buffer.from(keyId, "utf8"), Buffer.from([0]), payloadBytes]);
}

export function rawPublicKeyFromKeyObject(keyObject) {
  const spki = keyObject.export({ type: "spki", format: "der" });
  // Ed25519 SPKI DER is a fixed 12-byte prefix followed by the 32-byte key.
  return spki.subarray(spki.length - 32);
}

export function publicKeyObjectFromRaw(rawB64) {
  const raw = Buffer.from(rawB64, "base64");
  if (raw.length !== 32) throw new Error("public key must decode to 32 bytes");
  const prefix = Buffer.from("302a300506032b6570032100", "hex");
  return createPublicKey({ key: Buffer.concat([prefix, raw]), type: "spki", format: "der" });
}

export function signPayload(privateKey, keyId, payloadBytes) {
  return sign(null, signingMessage(keyId, payloadBytes), privateKey);
}

export function verifyEnvelope(envelope, publicKeyB64) {
  const errors = [];
  if (envelope.envelope !== ENVELOPE_VERSION) errors.push(`envelope version ${envelope.envelope} != ${ENVELOPE_VERSION}`);
  if (envelope.alg !== "ed25519") errors.push(`alg ${envelope.alg} != ed25519`);
  if (!KEY_ID_RE.test(String(envelope.key_id))) errors.push("bad key_id");
  const payloadBytes = Buffer.from(String(envelope.payload_b64), "base64");
  if (payloadBytes.length === 0 || payloadBytes.length > MAX_PAYLOAD_BYTES) errors.push("payload size out of range");
  const sig = Buffer.from(String(envelope.sig_b64), "base64");
  if (sig.length !== 64) errors.push("signature must be 64 bytes");
  const sha = createHash("sha256").update(payloadBytes).digest("hex");
  if (envelope.sha256 && envelope.sha256 !== sha) errors.push("sha256 mismatch");
  if (errors.length) return { ok: false, errors };
  let payload;
  try {
    payload = JSON.parse(payloadBytes.toString("utf8"));
  } catch (err) {
    return { ok: false, errors: [`payload is not JSON: ${err.message}`] };
  }
  for (const field of ["channel", "facts_version", "applies_to", "schema_version"]) {
    if (envelope[field] !== undefined && envelope[field] !== payload[field]) {
      errors.push(`outer ${field} (${envelope[field]}) != inner (${payload[field]})`);
    }
  }
  if (errors.length) return { ok: false, errors };
  const key = publicKeyObjectFromRaw(publicKeyB64);
  const ok = verify(null, signingMessage(envelope.key_id, payloadBytes), key, sig);
  if (!ok) return { ok: false, errors: ["bad signature"] };
  return { ok: true, errors: [], payload, sha256: sha };
}

// ---------------------------------------------------------------------------
// Source validation (docs/cloud-facts/<channel>.json)
// ---------------------------------------------------------------------------

const MODEL_OPS = new Set(["upsert", "deprecate", "hide"]);
const LEVELS = new Set(["info", "warn"]);
const SURFACES = new Set(["tui", "desktop", "web"]);
const VERSION_REQ_RE = /^(\*|[<>=^~]*\s*\d+(\.\d+){0,2}(-[0-9A-Za-z.-]+)?(\s*,\s*[<>=^~]*\s*\d+(\.\d+){0,2}(-[0-9A-Za-z.-]+)?)*)$/;

function isPlainObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v);
}

function optString(errors, where, obj, key, max = 500) {
  const v = obj[key];
  if (v === undefined || v === null) return;
  if (typeof v !== "string" || v.length > max) errors.push(`${where}.${key} must be a string (<= ${max} chars)`);
}

function optVersionReq(errors, where, obj, key = "applies_to") {
  const v = obj[key];
  if (v === undefined || v === null) return;
  if (typeof v !== "string" || !VERSION_REQ_RE.test(v.trim())) errors.push(`${where}.${key} is not a semver requirement: ${JSON.stringify(v)}`);
}

export function validateSource(source) {
  const errors = [];
  if (!isPlainObject(source)) return ["source must be an object"];
  if (source.schema_version !== undefined && source.schema_version !== SCHEMA_VERSION) {
    errors.push(`schema_version must be ${SCHEMA_VERSION}`);
  }
  if (source.channel !== undefined && !CHANNEL_RE.test(String(source.channel))) errors.push("channel slug invalid");
  if (source.facts_version !== undefined && !(Number.isInteger(source.facts_version) && source.facts_version > 0)) {
    errors.push("facts_version must be a positive integer");
  }
  optVersionReq(errors, "root", source);
  optString(errors, "root", source, "not_after", 40);
  const models = source.models ?? [];
  if (!Array.isArray(models)) errors.push("models must be an array");
  else {
    models.forEach((m, i) => {
      const where = `models[${i}]`;
      if (!isPlainObject(m)) return errors.push(`${where} must be an object`);
      if (typeof m.provider !== "string" || !m.provider) errors.push(`${where}.provider required`);
      if (typeof m.id !== "string" || !m.id) errors.push(`${where}.id required`);
      if (m.op !== undefined && !MODEL_OPS.has(m.op)) errors.push(`${where}.op must be one of ${[...MODEL_OPS].join("/")}`);
      for (const k of ["context_window", "max_output"]) {
        if (m[k] !== undefined && !(Number.isInteger(m[k]) && m[k] > 0)) errors.push(`${where}.${k} must be a positive integer`);
      }
      if (m.pricing !== undefined) {
        if (!isPlainObject(m.pricing)) errors.push(`${where}.pricing must be an object`);
        else for (const k of Object.keys(m.pricing)) {
          if (!["input_per_m", "output_per_m", "cache_read_per_m"].includes(k)) errors.push(`${where}.pricing.${k} unknown`);
          else if (typeof m.pricing[k] !== "number" || m.pricing[k] < 0) errors.push(`${where}.pricing.${k} must be a non-negative number`);
        }
      }
      if (m.reasoning !== undefined && typeof m.reasoning !== "boolean") errors.push(`${where}.reasoning must be boolean`);
      optString(errors, where, m, "display_name", 120);
      optString(errors, where, m, "deprecated_at", 40);
      optString(errors, where, m, "replacement", 200);
      optString(errors, where, m, "note", 300);
      optVersionReq(errors, where, m);
    });
  }
  const defaults = source.provider_defaults ?? {};
  if (!isPlainObject(defaults)) errors.push("provider_defaults must be an object");
  else for (const [provider, d] of Object.entries(defaults)) {
    const where = `provider_defaults.${provider}`;
    if (!isPlainObject(d)) { errors.push(`${where} must be an object`); continue; }
    optString(errors, where, d, "default_model", 200);
    optString(errors, where, d, "base_url", 300);
    if (typeof d.base_url === "string" && !/^https:\/\//.test(d.base_url)) errors.push(`${where}.base_url must be https`);
    optVersionReq(errors, where, d);
  }
  if (source.release !== undefined && source.release !== null) {
    const r = source.release;
    const where = "release";
    if (!isPlainObject(r)) errors.push("release must be an object");
    else {
      if (typeof r.latest !== "string" || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(r.latest)) errors.push("release.latest must be a semver version");
      if (r.yanked !== undefined && !(Array.isArray(r.yanked) && r.yanked.every((v) => typeof v === "string"))) errors.push("release.yanked must be a string array");
      optString(errors, where, r, "min_supported", 40);
      optString(errors, where, r, "notice", 300);
      optString(errors, where, r, "release_url", 300);
      optVersionReq(errors, where, r);
    }
  }
  const ann = source.announcements ?? [];
  if (!Array.isArray(ann)) errors.push("announcements must be an array");
  else {
    const seen = new Set();
    ann.forEach((a, i) => {
      const where = `announcements[${i}]`;
      if (!isPlainObject(a)) return errors.push(`${where} must be an object`);
      if (typeof a.id !== "string" || !/^[a-z0-9][a-z0-9-]{0,63}$/.test(a.id)) errors.push(`${where}.id invalid`);
      if (seen.has(a.id)) errors.push(`${where}.id duplicated`);
      seen.add(a.id);
      if (a.level !== undefined && !LEVELS.has(a.level)) errors.push(`${where}.level must be info|warn`);
      if (typeof a.text !== "string" || !a.text.trim() || a.text.length > 200) errors.push(`${where}.text required (<= 200 chars)`);
      optString(errors, where, a, "url", 300);
      if (a.surfaces !== undefined && !(Array.isArray(a.surfaces) && a.surfaces.every((s) => SURFACES.has(s)))) errors.push(`${where}.surfaces invalid`);
      optVersionReq(errors, where, a);
      optString(errors, where, a, "starts_at", 40);
      optString(errors, where, a, "expires_at", 40);
    });
  }
  const allowed = new Set(["$schema", "_meta", "schema_version", "channel", "facts_version", "published_at", "not_after", "applies_to", "models", "provider_defaults", "release", "announcements"]);
  for (const k of Object.keys(source)) if (!allowed.has(k)) errors.push(`unknown top-level field ${k}`);
  return errors;
}

/** Build the signed payload object (no signing) from a source file. */
export function buildPayload(source, { channel, factsVersion, publishedAt }) {
  const errors = validateSource(source);
  if (errors.length) throw new Error(`source invalid:\n  - ${errors.join("\n  - ")}`);
  const payload = {
    schema_version: SCHEMA_VERSION,
    channel,
    facts_version: factsVersion,
    published_at: publishedAt,
    applies_to: typeof source.applies_to === "string" ? source.applies_to.trim() : "*",
    models: source.models ?? [],
    provider_defaults: source.provider_defaults ?? {},
    release: source.release ?? null,
    announcements: source.announcements ?? [],
  };
  if (source.not_after) payload.not_after = source.not_after;
  return payload;
}

export function buildEnvelope({ privateKey, keyId, payload }) {
  if (!KEY_ID_RE.test(keyId)) throw new Error(`key_id must match ${KEY_ID_RE}`);
  const payloadBytes = Buffer.from(canonicalize(payload), "utf8");
  if (payloadBytes.length > MAX_PAYLOAD_BYTES) throw new Error(`payload exceeds ${MAX_PAYLOAD_BYTES} bytes`);
  const sig = signPayload(privateKey, keyId, payloadBytes);
  const sha256 = createHash("sha256").update(payloadBytes).digest("hex");
  const envelope = {
    envelope: ENVELOPE_VERSION,
    channel: payload.channel,
    facts_version: payload.facts_version,
    schema_version: payload.schema_version,
    key_id: keyId,
    alg: "ed25519",
    applies_to: payload.applies_to,
    published_at: payload.published_at,
    payload_b64: payloadBytes.toString("base64"),
    sig_b64: sig.toString("base64"),
    sigs: [],
    sha256,
  };
  const pub = rawPublicKeyFromKeyObject(createPublicKey(privateKey)).toString("base64");
  const check = verifyEnvelope(envelope, pub);
  if (!check.ok) throw new Error(`self-verify failed: ${check.errors.join("; ")}`);
  return envelope;
}

// ---------------------------------------------------------------------------
// CLI helpers
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const positional = [];
  const flags = {};
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg.startsWith("--")) {
      const key = arg.slice(2);
      const next = argv[i + 1];
      if (next === undefined || next.startsWith("--")) flags[key] = true;
      else { flags[key] = next; i += 1; }
    } else positional.push(arg);
  }
  return { positional, flags };
}

function loadPrivateKeyFromEnv() {
  let pem = process.env.CODEWHALE_FACTS_SIGNING_KEY;
  const file = process.env.CODEWHALE_FACTS_SIGNING_KEY_FILE;
  if (!pem && file) pem = readFileSync(file, "utf8");
  if (!pem) throw new Error("set CODEWHALE_FACTS_SIGNING_KEY (PEM) or CODEWHALE_FACTS_SIGNING_KEY_FILE");
  const key = createPrivateKey({ key: pem, format: "pem" });
  if (key.asymmetricKeyType !== "ed25519") throw new Error("signing key must be Ed25519");
  return key;
}

function loadTrustedKeysFromRepo() {
  const src = readFileSync(resolve(WEB_ROOT, "lib/cloud-facts/keys.ts"), "utf8");
  const out = new Map();
  const re = /keyId:\s*"([^"]+)",\s*publicKey:\s*"([^"]+)",\s*status:\s*"([^"]+)"/g;
  let m;
  while ((m = re.exec(src))) out.set(m[1], { publicKey: m[2], status: m[3] });
  return out;
}

function refuseUnderCi() {
  for (const marker of CI_MARKERS) {
    if (process.env[marker] && !/^(0|false|no|off)$/i.test(process.env[marker])) {
      throw new Error(`refusing to run with a secret under CI (${marker} is set); publish from the founder's machine`);
    }
  }
}

function sqlLiteral(value) {
  if (value === null || value === undefined) return "null";
  return `'${String(value).replace(/'/g, "''")}'`;
}

export function emitSql(envelope, { publishedBy = "", publicKeyB64, notes = "" }) {
  if (!publicKeyB64) throw new Error("public key required to emit the facts_key row");
  const check = verifyEnvelope(envelope, publicKeyB64);
  if (!check.ok) throw new Error(`envelope does not verify: ${check.errors.join("; ")}`);
  const payloadJson = Buffer.from(envelope.payload_b64, "base64").toString("utf8");
  return [
    "begin;",
    `insert into public.facts_key (key_id, scope, algorithm, public_key, status)`,
    `  values (${sqlLiteral(envelope.key_id)}, 'global', 'ed25519', ${sqlLiteral(publicKeyB64)}, 'active')`,
    `  on conflict (key_id) do nothing;`,
    `insert into public.facts_release (channel_id, facts_version, schema_version, envelope_version, applies_to, key_id, payload_b64, sig_b64, sigs, payload, published_at, not_after, published_by, notes)`,
    `  select c.id, ${envelope.facts_version}, ${envelope.schema_version}, ${envelope.envelope}, ${sqlLiteral(envelope.applies_to)}, ${sqlLiteral(envelope.key_id)},`,
    `         ${sqlLiteral(envelope.payload_b64)}, ${sqlLiteral(envelope.sig_b64)}, ${sqlLiteral(JSON.stringify(envelope.sigs ?? []))}::jsonb,`,
    `         ${sqlLiteral(payloadJson)}::jsonb, ${sqlLiteral(envelope.published_at)}::timestamptz, ${sqlLiteral(check.payload.not_after ?? null)}::timestamptz,`,
    `         ${sqlLiteral(publishedBy)}, ${sqlLiteral(notes)}`,
    `    from public.facts_channel c where c.scope = 'global' and c.slug = ${sqlLiteral(envelope.channel)};`,
    "commit;",
    "",
  ].join("\n");
}

async function postgrest(path, { method = "GET", body, prefer } = {}) {
  refuseUnderCi();
  const url = process.env.SUPABASE_URL;
  const key = process.env.SUPABASE_SERVICE_ROLE_KEY || process.env.SUPABASE_SECRET_KEY;
  if (!url || !key) throw new Error("SUPABASE_URL and SUPABASE_SERVICE_ROLE_KEY are required");
  const res = await fetch(`${url.replace(/\/$/, "")}/rest/v1/${path}`, {
    method,
    headers: {
      apikey: key,
      Authorization: `Bearer ${key}`,
      "Content-Type": "application/json",
      ...(prefer ? { Prefer: prefer } : {}),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`PostgREST ${method} ${path} -> ${res.status}: ${text.slice(0, 300)}`);
  return text ? JSON.parse(text) : null;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function nowIso() {
  return new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
}

async function main(argv) {
  const { positional, flags } = parseArgs(argv);
  const cmd = positional[0];
  if (!cmd || flags.help) {
    console.log(readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n").slice(1, 26).join("\n"));
    return 0;
  }
  if (cmd === "keygen") {
    const keyId = String(flags["key-id"] ?? "");
    if (!KEY_ID_RE.test(keyId)) throw new Error("--key-id must match cwf-[a-z0-9-]{1,32}");
    const out = flags.out ? resolve(String(flags.out)) : null;
    if (!out) throw new Error("--out <path> is required (write the private key OUTSIDE any repository)");
    if (existsSync(out)) throw new Error(`${out} already exists; refusing to overwrite a private key`);
    const { privateKey, publicKey } = generateKeyPairSync("ed25519");
    mkdirSync(dirname(out), { recursive: true, mode: 0o700 });
    writeFileSync(out, privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
    const raw = rawPublicKeyFromKeyObject(publicKey);
    console.log(JSON.stringify({
      key_id: keyId,
      algorithm: "ed25519",
      public_key_b64: raw.toString("base64"),
      public_key_bytes: [...raw],
      private_key_file: out,
      note: "Private key written with mode 0600. Move it into custody (password manager); never commit it.",
    }, null, 2));
    return 0;
  }
  if (cmd === "sign") {
    const sourcePath = resolve(String(flags.source ?? resolve(REPO_ROOT, "docs/cloud-facts/stable.json")));
    const source = readJson(sourcePath);
    const channel = String(flags.channel ?? source.channel ?? "stable");
    if (!CHANNEL_RE.test(channel)) throw new Error("bad channel slug");
    const factsVersion = Number(flags["facts-version"] ?? source.facts_version);
    if (!Number.isInteger(factsVersion) || factsVersion <= 0) throw new Error("--facts-version (or source.facts_version) must be a positive integer");
    const keyId = String(flags["key-id"] ?? "");
    const privateKey = loadPrivateKeyFromEnv();
    const publishedAt = String(flags["published-at"] ?? nowIso());
    const payload = buildPayload(source, { channel, factsVersion, publishedAt });
    const envelope = buildEnvelope({ privateKey, keyId, payload });
    const text = `${JSON.stringify(envelope, null, 2)}\n`;
    if (flags.out) {
      writeFileSync(resolve(String(flags.out)), text);
      console.error(`wrote ${flags.out} (channel=${channel} facts_version=${factsVersion} key_id=${keyId} sha256=${envelope.sha256})`);
    } else process.stdout.write(text);
    return 0;
  }
  if (cmd === "verify") {
    const envelope = readJson(resolve(String(positional[1] ?? "")));
    let pub = flags["public-key"];
    if (!pub) {
      const trusted = loadTrustedKeysFromRepo().get(envelope.key_id);
      if (!trusted) throw new Error(`key_id ${envelope.key_id} is not pinned in web/lib/cloud-facts/keys.ts; pass --public-key`);
      pub = trusted.publicKey;
    }
    const result = verifyEnvelope(envelope, String(pub));
    console.log(JSON.stringify({ ok: result.ok, errors: result.errors, channel: envelope.channel, facts_version: envelope.facts_version, key_id: envelope.key_id, sha256: result.sha256 ?? null }, null, 2));
    return result.ok ? 0 : 1;
  }
  if (cmd === "emit-sql") {
    const envelope = readJson(resolve(String(positional[1] ?? "")));
    let pub = flags["public-key"];
    if (!pub) pub = loadTrustedKeysFromRepo().get(envelope.key_id)?.publicKey;
    process.stdout.write(emitSql(envelope, { publishedBy: String(flags["published-by"] ?? ""), publicKeyB64: pub ? String(pub) : undefined, notes: String(flags.notes ?? "") }));
    return 0;
  }
  if (cmd === "publish") {
    const envelope = readJson(resolve(String(positional[1] ?? "")));
    const trusted = loadTrustedKeysFromRepo().get(envelope.key_id);
    const pub = flags["public-key"] ? String(flags["public-key"]) : trusted?.publicKey;
    if (!pub) throw new Error(`key_id ${envelope.key_id} is not pinned; refusing to publish an unpinned key`);
    const check = verifyEnvelope(envelope, pub);
    if (!check.ok) throw new Error(`envelope does not verify: ${check.errors.join("; ")}`);
    const row = {
      facts_version: envelope.facts_version,
      schema_version: envelope.schema_version,
      envelope_version: envelope.envelope,
      applies_to: envelope.applies_to,
      key_id: envelope.key_id,
      payload_b64: envelope.payload_b64,
      sig_b64: envelope.sig_b64,
      sigs: envelope.sigs ?? [],
      payload: check.payload,
      published_at: envelope.published_at,
      not_after: check.payload.not_after ?? null,
      published_by: String(flags["published-by"] ?? ""),
      notes: String(flags.notes ?? ""),
    };
    if (flags["dry-run"]) {
      console.log(JSON.stringify({ dry_run: true, channel: envelope.channel, facts_key: { key_id: envelope.key_id, public_key: pub }, facts_release: { ...row, payload_b64: `<${envelope.payload_b64.length} chars>` } }, null, 2));
      return 0;
    }
    const channels = await postgrest(`facts_channel?scope=eq.global&slug=eq.${encodeURIComponent(envelope.channel)}&select=id`);
    if (!channels?.length) throw new Error(`channel ${envelope.channel} does not exist`);
    await postgrest("facts_key", { method: "POST", body: { key_id: envelope.key_id, scope: "global", algorithm: "ed25519", public_key: pub, status: "active" }, prefer: "resolution=ignore-duplicates,return=minimal" });
    const inserted = await postgrest("facts_release", { method: "POST", body: { ...row, channel_id: channels[0].id }, prefer: "return=representation" });
    console.log(JSON.stringify({ published: true, channel: envelope.channel, facts_version: envelope.facts_version, release_id: inserted?.[0]?.id ?? null, payload_sha256: inserted?.[0]?.payload_sha256 ?? null }, null, 2));
    return 0;
  }
  if (cmd === "revoke") {
    const channel = String(flags.channel ?? "");
    const version = Number(flags.version);
    const reason = String(flags.reason ?? "");
    if (!CHANNEL_RE.test(channel) || !Number.isInteger(version) || !reason) throw new Error("--channel, --version and --reason are required");
    if (flags["dry-run"]) {
      console.log(JSON.stringify({ dry_run: true, channel, facts_version: version, status: "revoked", revoke_reason: reason }, null, 2));
      return 0;
    }
    const channels = await postgrest(`facts_channel?scope=eq.global&slug=eq.${encodeURIComponent(channel)}&select=id`);
    if (!channels?.length) throw new Error(`channel ${channel} does not exist`);
    const updated = await postgrest(`facts_release?channel_id=eq.${channels[0].id}&facts_version=eq.${version}`, {
      method: "PATCH",
      body: { status: "revoked", revoked_at: nowIso(), revoke_reason: reason },
      prefer: "return=representation",
    });
    console.log(JSON.stringify({ revoked: updated?.length ?? 0, channel, facts_version: version }, null, 2));
    return 0;
  }
  throw new Error(`unknown command ${cmd}`);
}

const invokedDirectly = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  main(process.argv.slice(2)).then((code) => process.exit(code)).catch((err) => {
    console.error(`facts-publish: ${err.message}`);
    process.exit(1);
  });
}
