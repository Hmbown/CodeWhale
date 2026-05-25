import { test } from "node:test";

import { assert } from "./test-harness.js";
import {
  renderDeveloperExpansion,
  renderGameToolResult,
  renderPlayerMessage,
  renderToolResult,
} from "../extensions/renderers.js";

test("player tool renderer hides raw JSON punctuation", () => {
  const rendered = renderGameToolResult("game_commit_turn", { revision: 6, nested: { ok: true } });
  assert.equal(rendered.title, "commit turn");
  assert.doesNotMatch(rendered.body, /[{}[\]"]/);
});

test("developer tool renderer exposes diagnostic detail", () => {
  const rendered = renderToolResult("game_status", { revision: 5 }, true);
  assert.equal(rendered.title, "game_status");
  assert.match(rendered.developerDetail ?? "", /revision/);
});

test("player message renderer trims empty transcript noise", () => {
  assert.equal(renderPlayerMessage("\n  She stops. \n\n You breathe. "), "She stops.\nYou breathe.");
});

test("developer expansion keeps raw diagnostic detail out of player renderer path", () => {
  const rendered = renderDeveloperExpansion("game_render", { view: { scene_title: "Station" } });
  assert.equal(rendered.title, "game_render diagnostics");
  assert.match(rendered.developerDetail ?? "", /scene_title/);
});
