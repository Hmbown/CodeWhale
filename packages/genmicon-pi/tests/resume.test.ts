import { test } from "node:test";

import { assert } from "./test-harness.js";
import { createCommandRegistry } from "../extensions/commands.js";
import type { RuntimeResponse } from "../extensions/runtime-client.js";
import { buildResumeContext, injectResumeContext } from "../extensions/session-context.js";

const resumeResponse: RuntimeResponse = {
  ok: true,
  data: {
    status: {
      game: { id: "reconciliation-demo" },
      save: { id: "default", revision: 6 },
      driver: { id: "galgame" },
    },
    render: {
      view: {
        scene_title: "Station overpass",
      },
    },
  },
  warnings: [],
  error: null,
};

test("resume context is built from save and render snapshots", () => {
  const context = buildResumeContext(resumeResponse);
  assert.match(context, /game=reconciliation-demo/);
  assert.match(context, /save=default/);
  assert.match(context, /revision=6/);
  assert.match(context, /STATE\.json and TURN_LOG\.jsonl remain authoritative/);
});

test("resume context injection does not require prior transcript", () => {
  const injected: string[] = [];
  injectResumeContext({ injectContext: (message) => injected.push(message) }, resumeResponse);
  assert.equal(injected.length, 1);
  assert.match(injected[0] ?? "", /scene=Station overpass/);
});

test("play command loads resume snapshot before opening console", async () => {
  const injected: string[] = [];
  const opened: unknown[] = [];
  const play = createCommandRegistry({
    validateGame: async () => ({
      ok: true,
      data: {
        game: { id: "reconciliation-demo" },
        driver: { id: "galgame" },
        save: { revision: 6 },
      },
      warnings: [],
      error: null,
    }),
    loadResume: async () => resumeResponse,
  }).find((command) => command.id === "genmicon:play");

  assert.ok(play);
  await play.handler("examples/games/reconciliation-demo --save default", {
    cwd: "/repo",
    session: {
      injectContext: (message) => injected.push(message),
    },
    ui: {
      openView: (_viewId, data) => opened.push(data),
      notify: () => {},
    },
  });

  assert.match(injected[0] ?? "", /revision=6/);
  assert.deepEqual((opened[0] as { resume?: unknown }).resume, resumeResponse.data);
});
