import { test } from "node:test";

import { assert } from "./test-harness.js";
import { installPlayerActiveTools } from "../extensions/active-tools.js";
import { createCommandRegistry } from "../extensions/commands.js";
import { executeGameTool, extractRefreshedView } from "../extensions/tools.js";

test("play command validates before installing player tools and opening console", async () => {
  const installedTools: string[][] = [];
  const openedViews: string[] = [];
  const messages: string[] = [];
  const play = createCommandRegistry({
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
    loadResume: async () => ({
      ok: true,
      data: {
        status: {
          game: { id: "reconciliation-demo" },
          save: { id: "default", revision: 5 },
          driver: { id: "galgame" },
        },
        render: {
          view: { scene_title: "Station overpass" },
        },
      },
      warnings: [],
      error: null,
    }),
    installPlayerTools: (target = {}) => installPlayerActiveTools(target),
  }).find((command) => command.id === "genmicon:play");

  assert.ok(play);
  await play.handler("examples/games/reconciliation-demo --save default", {
    cwd: "/repo",
    tools: {
      setActiveTools: (tools) => installedTools.push([...tools]),
    },
    ui: {
      openView: (viewId) => openedViews.push(viewId),
      notify: (message) => messages.push(message),
    },
  });

  assert.deepEqual(installedTools[0], [
    "game_status",
    "game_render",
    "game_playbook",
    "game_lookup",
    "game_fact_check",
    "game_run_driver",
    "game_commit_turn",
  ]);
  assert.deepEqual(openedViews, ["genmicon.gameConsole"]);
  assert.equal(messages[0], "GENmicon player mode started for reconciliation-demo at revision 5.");
});

test("commit tool rejects transcript-derived state writes", async () => {
  await assert.rejects(
    () =>
      executeGameTool("game_commit_turn", {
        game_root: "game",
        expected_revision: 5,
        player_input: "continue",
        resolution: "continued",
        state_patch: {},
        transcript_state: { invented: true },
      }),
    /transcript-derived/,
  );
});

test("player turn commit sends one sequential runtime commit and returns refreshed view", async () => {
  const calls: string[] = [];
  const response = await executeGameTool(
    "game_commit_turn",
    {
      game_root: "game",
      save_id: "default",
      expected_revision: 5,
      player_input: "I admit I was scared.",
      resolution: "She stops.",
      state_patch: {
        scene: {
          summary: "The apology lands.",
        },
      },
    },
    async (request) => {
      calls.push(request.command);
      return {
        ok: true,
        data: {
          revision: 6,
          view: { revision: 6, scene_title: "Station overpass" },
        },
        warnings: [],
        error: null,
      };
    },
  );

  assert.deepEqual(calls, ["commit_turn"]);
  assert.deepEqual(extractRefreshedView(response), { revision: 6, scene_title: "Station overpass" });
});
