import { test } from "node:test";

import { assert } from "./test-harness.js";
import {
  createGameToolDefinitions,
  executeGameTool,
  extractRefreshedView,
  gameToolDefinition,
  gameToolNames,
} from "../extensions/tools.js";

test("game-safe tool definitions register the full player allowlist", () => {
  const definitions = createGameToolDefinitions();
  assert.deepEqual(definitions.map((definition) => definition.name), gameToolNames);
  assert.ok(definitions.every((definition) => definition.playerSafe));
});

test("commit tool is mutating and sequential while read tools are not", () => {
  const commit = gameToolDefinition("game_commit_turn");
  assert.equal(commit.runtimeCommand, "commit_turn");
  assert.equal(commit.mutatesSave, true);
  assert.equal(commit.sequential, true);

  const status = gameToolDefinition("game_status");
  assert.equal(status.runtimeCommand, "status");
  assert.equal(status.mutatesSave, false);
  assert.equal(status.sequential, false);
});

test("tool definitions expose typebox object input schemas", () => {
  for (const definition of createGameToolDefinitions()) {
    const schema = definition.inputSchema as { type?: string; properties?: Record<string, unknown> };
    assert.equal(schema.type, "object");
    assert.ok("game_root" in (schema.properties ?? {}));
  }
});

test("tool execution maps package tool calls to runtime commands", async () => {
  const response = await executeGameTool(
    "game_lookup",
    {
      game_root: "game",
      save_id: "default",
      query: "rain",
      max_bytes: 1000,
    },
    async (request) => ({
      ok: true,
      data: request,
      warnings: [],
      error: null,
    }),
  );

  assert.equal(response.ok, true);
  assert.equal((response.data as { command?: string }).command, "lookup");
  assert.equal((response.data as { save_id?: string }).save_id, "default");
  assert.deepEqual((response.data as { payload?: unknown }).payload, {
    query: "rain",
    max_bytes: 1000,
  });
});

test("commit responses expose refreshed runtime view", () => {
  const view = { scene_title: "Station", revision: 6 };
  assert.deepEqual(
    extractRefreshedView({
      ok: true,
      data: { view },
      warnings: [],
      error: null,
    }),
    view,
  );
});
