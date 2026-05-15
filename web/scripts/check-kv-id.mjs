#!/usr/bin/env node
/**
 * check-kv-id.mjs — pre-deploy check that wrangler.jsonc has
 * real KV namespace IDs, not placeholders.
 *
 * Prints the exact `wrangler kv namespace create` command to run
 * when a placeholder is found, then exits non-zero.
 */
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const cfgPath = join(__dirname, "..", "wrangler.jsonc");
const raw = readFileSync(cfgPath, "utf-8");

// Parse JSONC (strip comments, trailing commas).
// Use a two-pass approach to avoid mangling URLs: first strip
// line comments that look like comments (preceded by whitespace
// or comma, not part of ://), then strip block comments.
const stripped = raw
  .replace(/(^|[,\s])\/\/[^\n]*/gm, "$1")  // line comments (skips :// in URLs)
  .replace(/\/\*[\s\S]*?\*\//g, "")          // block comments
  .replace(/,\s*}/g, "}")                    // trailing commas
  .replace(/,\s*]/g, "]");
const cfg = JSON.parse(stripped);

const nss = cfg.kv_namespaces;
if (!Array.isArray(nss) || nss.length === 0) {
  console.log("No KV namespaces defined — skipping check.");
  process.exit(0);
}

let dirty = false;
for (const ns of nss) {
  if (ns.id === "REPLACE_WITH_KV_ID") {
    dirty = true;
    console.error("");
    console.error("❌  KV namespace %s has placeholder id.", ns.binding);
    console.error("    Run this command and paste the returned id into wrangler.jsonc:");
    console.error("");
    console.error("      npx wrangler kv namespace create %s", ns.binding);
    console.error("");
  }
}

if (dirty) {
  process.exit(1);
}

console.log("✅  All KV namespace IDs are set.");
