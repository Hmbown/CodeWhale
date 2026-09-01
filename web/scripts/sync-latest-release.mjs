#!/usr/bin/env node
// Refresh the checked-in "latest published release" fact from the real GitHub
// release. The fact is mirrored in two places and BOTH must move together:
//
//   web/data/latest-published-release.json   (read by derive-facts.mjs)
//   docs/public-surface-facts.json           (latestPublishedRelease, which
//                                             names the file above as its
//                                             `sources`)
//
// web/lib/public-surface-contract.test.ts asserts the two agree, so updating
// only the first turns a stale marketing fact into a red Lint & Type Check.
//
// Facts must be derivable from the repo with no network (derive-facts.mjs reads
// this file, it does not call GitHub), so the file is checked in. Nothing wrote
// it, which is why it drifted: the marketing deploy's post-deploy comparison
// failed on latestPublishedRelease.tag because this said v0.9.10 while the
// published release was v0.9.11.
//
//   node web/scripts/sync-latest-release.mjs          # write if changed
//   node web/scripts/sync-latest-release.mjs --check  # exit 1 if stale
//
// --check is the CI form: it makes drift a failing gate at PR time instead of a
// surprise after a production deploy.

import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const REPO = "Hmbown/CodeWhale";
const here = dirname(fileURLToPath(import.meta.url));
const target = resolve(here, "..", "data", "latest-published-release.json");
const mirror = resolve(here, "..", "..", "docs", "public-surface-facts.json");
const checkOnly = process.argv.includes("--check");

const headers = {
  accept: "application/vnd.github+json",
  "user-agent": "codewhale-facts-sync",
};
if (process.env.GITHUB_TOKEN) headers.authorization = `Bearer ${process.env.GITHUB_TOKEN}`;

const response = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers });
if (!response.ok) {
  console.error(`[sync-latest-release] GitHub returned ${response.status}; leaving the file alone.`);
  process.exit(checkOnly ? 0 : 1);
}
const release = await response.json();

const tag = String(release.tag_name || "");
const version = tag.startsWith("v") ? tag.slice(1) : "";
const next = {
  tag,
  version,
  publishedAt: String(release.published_at || ""),
  url: `https://github.com/${REPO}/releases/tag/${tag}`,
};

// deriveLatestPublishedRelease() silently returns null on any shape violation,
// which would drop the fact entirely rather than report a bad one. Fail loudly.
if (!tag || !version || tag !== `v${version}` || !Number.isFinite(Date.parse(next.publishedAt))) {
  console.error(`[sync-latest-release] refusing to write an unusable release fact: ${JSON.stringify(next)}`);
  process.exit(1);
}

const readJson = (path) => {
  try { return JSON.parse(readFileSync(path, "utf8")); } catch { return null; }
};

const current = readJson(target);
const matrix = readJson(mirror);
const currentMirror = matrix?.latestPublishedRelease ?? null;

const isCurrent = (fact) =>
  Boolean(fact) && fact.tag === next.tag && fact.publishedAt === next.publishedAt;

if (isCurrent(current) && isCurrent(currentMirror)) {
  console.log(`[sync-latest-release] already current at ${next.tag}`);
  process.exit(0);
}

if (checkOnly) {
  if (!isCurrent(current)) {
    console.error(
      `[sync-latest-release] stale: ${target} says ${current?.tag ?? "(missing)"}, GitHub says ${next.tag}`,
    );
  }
  if (!isCurrent(currentMirror)) {
    console.error(
      `[sync-latest-release] stale: docs/public-surface-facts.json says ${currentMirror?.tag ?? "(missing)"}, GitHub says ${next.tag}`,
    );
  }
  console.error("Run: npm --prefix web run sync:latest-release && npm --prefix web run build");
  process.exit(1);
}

writeFileSync(target, `${JSON.stringify(next, null, 2)}\n`);

if (!matrix) {
  console.error(`[sync-latest-release] could not read ${mirror}; the mirror is now stale.`);
  process.exit(1);
}

// Preserve every key the matrix carries beyond the four synced fields (notably
// `sources`), so this stays a fact refresh and not a schema rewrite.
matrix.latestPublishedRelease = { ...currentMirror, ...next };
writeFileSync(mirror, `${JSON.stringify(matrix, null, 2)}\n`);

console.log(`[sync-latest-release] wrote ${next.tag} (${next.publishedAt}) to both facts`);
