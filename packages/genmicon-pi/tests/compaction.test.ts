import { test } from "node:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { assert, repoRoot } from "./test-harness.js";

test("game compaction prompt preserves save authority", () => {
  const prompt = readFileSync(join(repoRoot(), "packages/genmicon-pi/prompts/compact-game-context.md"), "utf8");
  assert.match(prompt, /`STATE\.json` and `TURN_LOG\.jsonl`/);
  assert.match(prompt, /Never convert compacted transcript text into a state patch/);
  assert.match(prompt, /status` and `render` commands/);
});
