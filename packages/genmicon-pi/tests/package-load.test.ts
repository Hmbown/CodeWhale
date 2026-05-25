import { test } from "node:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { assert, repoRoot } from "./test-harness.js";

test("package manifest declares reviewed Pi resources", () => {
  const manifest = JSON.parse(readFileSync(join(repoRoot(), "packages/genmicon-pi/package.json"), "utf8"));
  assert.ok(manifest.keywords.includes("pi-package"));
  assert.deepEqual(manifest.pi.extensions, ["./extensions/index.ts"]);
  assert.deepEqual(manifest.pi.skills, ["./skills"]);
  assert.deepEqual(manifest.pi.prompts, ["./prompts"]);
  assert.deepEqual(manifest.pi.themes, ["./themes"]);
  assert.equal(manifest.peerDependencies["@earendil-works/pi-coding-agent"], "*");
  assert.equal(manifest.peerDependencies["@earendil-works/pi-tui"], "*");
  assert.equal(manifest.peerDependencies.typebox, "*");
});

test("project settings load only filtered package resources", () => {
  const settings = JSON.parse(readFileSync(join(repoRoot(), ".pi/settings.json"), "utf8"));
  assert.deepEqual(settings.packages, [
    {
      source: "./packages/genmicon-pi",
      extensions: ["extensions/index.ts"],
      skills: ["skills/**/SKILL.md"],
      prompts: ["prompts/*.md"],
      themes: ["themes/genmicon.json"],
    },
  ]);
});

test("package manifest and settings agree on extension entrypoint", () => {
  const manifest = JSON.parse(readFileSync(join(repoRoot(), "packages/genmicon-pi/package.json"), "utf8"));
  const settings = JSON.parse(readFileSync(join(repoRoot(), ".pi/settings.json"), "utf8"));
  assert.equal(settings.packages[0].extensions[0], manifest.pi.extensions[0].replace(/^\.\//, ""));
});
