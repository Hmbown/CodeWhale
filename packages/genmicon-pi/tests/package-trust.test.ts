import { test } from "node:test";
import { join } from "node:path";

import { assert, repoRoot } from "./test-harness.js";
import {
  canEnablePlayerMode,
  collectResourceInventory,
  reviewPackageSource,
  validatePackageFilters,
} from "../extensions/package-trust.js";

test("local package source is reviewed and unpinned remote source is blocked", () => {
  assert.equal(reviewPackageSource("./packages/genmicon-pi").status, "reviewed");
  assert.equal(reviewPackageSource("npm:genmicon-pi").status, "blocked");
  assert.equal(reviewPackageSource("git:github.com/example/genmicon@v1").status, "unreviewed");
  assert.equal(canEnablePlayerMode(reviewPackageSource("git:github.com/example/genmicon@v1")), false);
});

test("package filters warn on broad extension globs", () => {
  assert.deepEqual(validatePackageFilters({ source: "./pkg", extensions: ["extensions/index.ts"] }), []);
  assert.deepEqual(validatePackageFilters({ source: "./pkg", extensions: ["extensions/**/*.ts"] }), [
    "extension filters should avoid broad globs",
  ]);
});

test("resource inventory discovers package resources", () => {
  const inventory = collectResourceInventory(join(repoRoot(), "packages/genmicon-pi"));
  assert.ok(inventory.extensions.includes("index.ts"));
  assert.ok(inventory.skills.includes("game-driver/SKILL.md"));
  assert.ok(inventory.prompts.includes("game-console.md"));
  assert.ok(inventory.themes.includes("genmicon.json"));
});
