#!/usr/bin/env node
// Refresh web/data/latest-published-release.json from the real GitHub release.
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

const current = (() => {
  try { return JSON.parse(readFileSync(target, "utf8")); } catch { return null; }
})();

const serialized = `${JSON.stringify(next, null, 2)}\n`;
if (current && current.tag === next.tag && current.publishedAt === next.publishedAt) {
  console.log(`[sync-latest-release] already current at ${next.tag}`);
  process.exit(0);
}

if (checkOnly) {
  console.error(`[sync-latest-release] stale: file says ${current?.tag ?? "(missing)"}, GitHub says ${next.tag}`);
  console.error("Run: npm --prefix web run sync:latest-release && npm --prefix web run build");
  process.exit(1);
}

writeFileSync(target, serialized);
console.log(`[sync-latest-release] wrote ${next.tag} (${next.publishedAt})`);
