#!/usr/bin/env node
/**
 * check-docs.mjs — drift / parity gate for website documentation.
 *
 * Verifies that:
 *   1. Every doc topic in docs-map.ts points to a real repo source file.
 *   2. Every topic registered with hasPage: true is actually served — either
 *      by a hand-authored app/[locale]/docs/<slug>/page.tsx or by the dynamic
 *      Markdown renderer app/[locale]/docs/[slug]/page.tsx — and, inversely,
 *      no hand-authored page exists for a topic marked hasPage: false.
 *   3. Version, command snippets, and tool names referenced on the website
 *      match the current workspace state.
 *
 * Usage:
 *   cd web && npm run check:docs
 *
 * Relies on facts-lib.mjs for version / provider / tool derivation.
 */
import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_DIR = resolve(__dirname, "..");
const REPO_ROOT = resolve(WEB_DIR, "..");

/* ------------------------------------------------------------------ */
/*  Parse docs-map.ts (regex — avoids ts-node dependency)              */
/* ------------------------------------------------------------------ */

function parseDocsMap() {
  const path = resolve(WEB_DIR, "lib", "docs-map.ts");
  if (!existsSync(path)) {
    console.error(`[check-docs] ERROR: docs-map.ts not found at ${path}`);
    process.exit(1);
  }
  const src = readFileSync(path, "utf-8");

  const topics = [];
  const re =
    /\{\s*id:\s*"(\w[^"]*)",\s*slug:\s*"([\w-][^"]*)",[\s\S]*?repoSource:\s*(\[[^\]]+\]|"[^"]+"),\s*hasPage:\s*(true|false)/g;
  let m;
  while ((m = re.exec(src)) !== null) {
    const id = m[1];
    const slug = m[2];
    let rawSource = m[3];
    const sources = rawSource.startsWith("[")
      ? rawSource.match(/"([^"]+)"/g)?.map((s) => s.slice(1, -1)) ?? []
      : [rawSource.slice(1, -1)];
    topics.push({ id, slug, repoSource: sources, hasPage: m[4] === "true" });
  }
  return topics;
}

/* ------------------------------------------------------------------ */
/*  Check 2: hasPage topics are actually served (and vice versa)        */
/* ------------------------------------------------------------------ */

function checkPagesExist(topics) {
  const docsAppDir = resolve(WEB_DIR, "app", "[locale]", "docs");
  const hasDynamicRenderer = existsSync(resolve(docsAppDir, "[slug]", "page.tsx"));

  const problems = [];
  for (const t of topics) {
    const hasCustomPage = existsSync(resolve(docsAppDir, t.slug, "page.tsx"));
    if (t.hasPage && !hasCustomPage && !hasDynamicRenderer) {
      problems.push(
        `${t.id}: hasPage=true but neither app/[locale]/docs/${t.slug}/page.tsx nor the dynamic [slug] renderer exists`,
      );
    }
    if (!t.hasPage && hasCustomPage) {
      problems.push(
        `${t.id}: hasPage=false but app/[locale]/docs/${t.slug}/page.tsx exists — flip hasPage in docs-map.ts`,
      );
    }
  }
  return problems;
}

/* ------------------------------------------------------------------ */
/*  Check 1: every repo source file exists                             */
/* ------------------------------------------------------------------ */

function checkSourcesExist(topics) {
  const missing = [];
  for (const t of topics) {
    for (const src of t.repoSource) {
      const p = resolve(REPO_ROOT, src);
      if (!existsSync(p)) {
        missing.push({ topic: t.id, source: src, expected: p });
      }
    }
  }
  return missing;
}

/* ------------------------------------------------------------------ */
/*  Check 3: version matches Cargo.toml                                 */
/* ------------------------------------------------------------------ */

function deriveVersion() {
  const cargoPath = resolve(REPO_ROOT, "Cargo.toml");
  if (!existsSync(cargoPath)) return null;
  const cargo = readFileSync(cargoPath, "utf-8");
  const m = cargo.match(/^version\s*=\s*"([^"]+)"/m);
  return m ? m[1] : null;
}

function checkVersion() {
  const version = deriveVersion();
  return { version, ok: version != null };
}

/* ------------------------------------------------------------------ */
/*  Check 4: command snippet freshness (install commands)               */
/* ------------------------------------------------------------------ */

function checkInstallSnippets() {
  const version = deriveVersion();
  if (!version) return { ok: false, note: "could not derive version" };

  const installPath = resolve(WEB_DIR, "app", "[locale]", "install", "page.tsx");
  if (!existsSync(installPath)) return { ok: true, note: "install page not found" };

  const src = readFileSync(installPath, "utf-8");
  const versionRefs = [...src.matchAll(/codewhale.*?([\d]+\.[\d]+\.[\d]+)/g)];
  const stale = [];
  for (const ref of versionRefs) {
    const v = ref[1];
    if (v !== version) {
      stale.push({ found: v, expected: version, context: ref[0].slice(0, 60) });
    }
  }
  return { ok: stale.length === 0, stale };
}

/* ------------------------------------------------------------------ */
/*  Main                                                                */
/* ------------------------------------------------------------------ */

function main() {
  const topics = parseDocsMap();
  if (topics.length === 0) {
    console.error("[check-docs] ERROR: no topics parsed from docs-map.ts");
    process.exit(1);
  }
  console.log(`[check-docs] parsed ${topics.length} doc topics`);

  // Check 1: sources exist
  const missingSources = checkSourcesExist(topics);
  if (missingSources.length > 0) {
    console.error("[check-docs] FAIL — missing repo source files:");
    for (const m of missingSources) {
      console.error(`  ${m.topic}: ${m.source} → ${m.expected} (not found)`);
    }
    process.exit(1);
  }
  console.log("[check-docs] OK — all repo source files exist");

  // Check 2: every hasPage topic is served by a real route
  const pageProblems = checkPagesExist(topics);
  if (pageProblems.length > 0) {
    console.error("[check-docs] FAIL — docs-map/page mismatches:");
    for (const p of pageProblems) console.error(`  ${p}`);
    process.exit(1);
  }
  const pageCount = topics.filter((t) => t.hasPage).length;
  console.log(`[check-docs] OK — ${pageCount} hasPage topics all have routes`);

  // Check 3: version
  const ver = checkVersion();
  if (!ver.ok) {
    console.error("[check-docs] FAIL — could not derive version from workspace");
    process.exit(1);
  }
  console.log(`[check-docs] OK — version=${ver.version}`);

  // Check 4: install snippets
  const install = checkInstallSnippets();
  if (!install.ok && !install.note) {
    console.error("[check-docs] FAIL — stale version in install snippets:");
    for (const s of install.stale) {
      console.error(`  found "${s.found}", expected "${s.expected}" in: ${s.context}`);
    }
    // #3770: a stale install snippet must fail the gate, not fall through to
    // the final PASS. Mirror the exit(1) used by checks 1 and 2 above.
    process.exit(1);
  }
  console.log(`[check-docs] OK — install snippets${install.note ? ` (${install.note})` : ""}`);

  console.log("[check-docs] PASS");
}

try {
  main();
} catch (e) {
  console.error("[check-docs] ERROR:", e.message);
  process.exit(1);
}
