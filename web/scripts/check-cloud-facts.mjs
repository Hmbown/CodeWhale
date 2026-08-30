#!/usr/bin/env node
/**
 * CI gate for the cloud facts channel (runs inside `npm run check:facts`):
 *   1. docs/cloud-facts/stable.json validates against the facts/v1 source schema;
 *   2. its release.latest agrees with web/data/latest-published-release.json
 *      (release truth stays single-sourced);
 *   3. the pinned key table in crates/config/src/cloud_facts/keys.rs matches
 *      web/lib/cloud-facts/keys.ts byte for byte (key id, public key, status);
 *   4. the committed test fixtures still verify under the TEST-ONLY key and the
 *      test-only key is never pinned as a trust anchor.
 */
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { validateSource, verifyEnvelope } from "./facts-publish.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const WEB_ROOT = resolve(here, "..");
const REPO_ROOT = resolve(WEB_ROOT, "..");
const failures = [];

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

// 1. source validates
const sourcePath = resolve(REPO_ROOT, "docs/cloud-facts/stable.json");
const source = readJson(sourcePath);
for (const err of validateSource(source)) failures.push(`stable.json: ${err}`);
if (source.channel !== "stable") failures.push("stable.json: channel must be 'stable'");

// 2. release truth agreement
const latest = readJson(resolve(WEB_ROOT, "data/latest-published-release.json"));
if (source.release?.latest !== latest.version) {
  failures.push(`stable.json release.latest (${source.release?.latest}) != latest-published-release.json version (${latest.version})`);
}
if (source.release?.release_url && source.release.release_url !== latest.url) {
  failures.push(`stable.json release.release_url (${source.release.release_url}) != latest-published-release.json url (${latest.url})`);
}

// 3. keys.rs == keys.ts
export function parseRustKeys(text) {
  const out = [];
  const re = /key_id:\s*"([^"]+)",[\s\S]*?public_key:\s*\[([^\]]+)\],[\s\S]*?status:\s*KeyStatus::(\w+)/g;
  let m;
  while ((m = re.exec(text))) {
    const bytes = m[2].split(",").map((s) => s.trim()).filter(Boolean).map(Number);
    if (bytes.length !== 32 || bytes.some((b) => !Number.isInteger(b) || b < 0 || b > 255)) {
      failures.push(`keys.rs: ${m[1]} public key is not 32 bytes`);
    }
    out.push({ keyId: m[1], publicKey: Buffer.from(bytes).toString("base64"), status: m[3].toLowerCase() });
  }
  return out;
}

export function parseTsKeys(text) {
  const out = [];
  const re = /keyId:\s*"([^"]+)",\s*publicKey:\s*"([^"]+)",\s*status:\s*"([^"]+)"/g;
  let m;
  while ((m = re.exec(text))) out.push({ keyId: m[1], publicKey: m[2], status: m[3] });
  return out;
}

const rustKeys = parseRustKeys(readFileSync(resolve(REPO_ROOT, "crates/config/src/cloud_facts/keys.rs"), "utf8"));
const tsKeys = parseTsKeys(readFileSync(resolve(WEB_ROOT, "lib/cloud-facts/keys.ts"), "utf8"));
const rustJson = JSON.stringify(rustKeys);
const tsJson = JSON.stringify(tsKeys);
if (rustJson !== tsJson) {
  failures.push(`pinned keys diverge:\n  keys.rs: ${rustJson}\n  keys.ts: ${tsJson}`);
}

// 4. fixtures verify under the test-only key; test-only key never pinned
const testOnlyPub = readFileSync(resolve(REPO_ROOT, "docs/cloud-facts/fixtures/test-only.pub"), "utf8").trim();
for (const name of ["envelope-stable-v7.json", "envelope-future-only-v8.json"]) {
  const envelope = readJson(resolve(REPO_ROOT, "docs/cloud-facts/fixtures", name));
  const result = verifyEnvelope(envelope, testOnlyPub);
  if (!result.ok) failures.push(`fixture ${name} does not verify: ${result.errors.join("; ")}`);
}
if (tsKeys.some((k) => k.publicKey === testOnlyPub) || rustKeys.some((k) => k.publicKey === testOnlyPub)) {
  failures.push("the TEST-ONLY key must never be pinned as a trust anchor");
}

if (failures.length) {
  console.error("check-cloud-facts: FAIL");
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log(
  `check-cloud-facts: OK (stable.json facts_version=${source.facts_version}, release.latest=${source.release?.latest}, ${tsKeys.length} pinned key(s): ${tsKeys.map((k) => `${k.keyId}/${k.status}`).join(", ")})`,
);
