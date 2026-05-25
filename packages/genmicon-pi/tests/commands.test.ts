import { test } from "node:test";

import { assert } from "./test-harness.js";
import {
  blockingWarnings,
  createCommandRegistry,
  formatValidationResult,
  formatPlayerLaunchResult,
  formatSaveListResult,
  parseGamePathArgs,
} from "../extensions/commands.js";
import type { RuntimeResponse } from "../extensions/runtime-client.js";
import { createInitialGameState, type GameSessionState } from "../extensions/state.js";

test("command registry exposes validation command", () => {
  const commands = createCommandRegistry();
  assert.ok(commands.some((command) => command.id === "genmicon:validate"));
});

test("validation command parses game path and save id", () => {
  const parsed = parseGamePathArgs("examples/games/reconciliation-demo --save default", "/repo");
  assert.equal(parsed.gameRoot, "/repo/examples/games/reconciliation-demo");
  assert.equal(parsed.saveId, "default");
});

test("validation result formatting uses runtime envelope fields", () => {
  const response: RuntimeResponse = {
    ok: true,
    data: {
      game: { id: "reconciliation-demo" },
      driver: { id: "galgame" },
      save: { revision: 5 },
    },
    warnings: [],
    error: null,
  };
  assert.equal(
    formatValidationResult(response),
    "GENmicon validation passed for reconciliation-demo using galgame, revision 5.",
  );
});

test("blocking warnings map player unsafe validation messages", () => {
  const response: RuntimeResponse = {
    ok: true,
    data: {},
    warnings: ["optional asset missing", "remote package source blocked"],
    error: null,
  };
  assert.deepEqual(blockingWarnings(response), ["optional asset missing", "remote package source blocked"]);
});

test("validation command calls runtime readiness check and notifies", async () => {
  const messages: string[] = [];
  const [command] = createCommandRegistry({
    validateGame: async () => ({
      ok: true,
      data: {
        game: { id: "reconciliation-demo" },
        driver: { id: "galgame" },
        save: { revision: 5 },
      },
      warnings: [],
      error: null,
    }),
  });
  assert.ok(command);

  await command.handler("examples/games/reconciliation-demo --save default", {
    cwd: "/repo",
    ui: {
      notify: (message) => messages.push(message),
    },
  });

  assert.equal(messages[0], "GENmicon validation passed for reconciliation-demo using galgame, revision 5.");
});

test("player launch formatter reports game and revision", () => {
  assert.equal(
    formatPlayerLaunchResult({
      ok: true,
      data: {
        game: { id: "reconciliation-demo" },
        save: { revision: 5 },
      },
      warnings: [],
      error: null,
    }),
    "GENmicon player mode started for reconciliation-demo at revision 5.",
  );
});

test("developer command toggles diagnostics state", async () => {
  const dev = createCommandRegistry().find((command) => command.id === "genmicon:dev");
  assert.ok(dev);
  const messages: string[] = [];
  const states: GameSessionState[] = [];
  await dev.handler("on", {
    gameState: createInitialGameState(),
    setGameState: (state) => states.push(state),
    ui: {
      notify: (message) => messages.push(message),
    },
  });

  assert.equal(states[0]?.diagnosticsVisible, true);
  assert.equal(messages[0], "GENmicon diagnostics on.");
});

test("save list command reports runtime saves", async () => {
  const saves = createCommandRegistry({
    listSaves: async () => ({
      ok: true,
      data: {
        saves: [
          {
            id: "default",
            revision: 6,
            driver: { id: "galgame" },
          },
        ],
      },
      warnings: [],
      error: null,
    }),
  }).find((command) => command.id === "genmicon:saves");
  assert.ok(saves);
  const messages: string[] = [];
  await saves.handler("examples/games/reconciliation-demo", {
    cwd: "/repo",
    ui: {
      notify: (message) => messages.push(message),
    },
  });
  assert.equal(messages[0], "GENmicon saves: default rev 6 (galgame)");
});

test("save list formatter handles empty and failed responses", () => {
  assert.equal(formatSaveListResult({ ok: true, data: { saves: [] }, warnings: [], error: null }), "GENmicon saves: none found.");
  assert.equal(
    formatSaveListResult({
      ok: false,
      data: null,
      warnings: [],
      error: { code: "read_error", message: "missing saves", recoverable: true },
    }),
    "GENmicon saves failed: missing saves",
  );
});
