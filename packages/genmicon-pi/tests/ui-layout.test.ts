import { test } from "node:test";

import { assert } from "./test-harness.js";
import {
  createActionComposerState,
  createGameConsoleModel,
  layoutForWidth,
  renderCompactGameView,
  updateActionComposer,
} from "../extensions/ui/game-console.js";

test("game console model contains expected player regions", () => {
  const model = createGameConsoleModel();
  assert.equal(model.id, "genmicon.gameConsole");
  assert.ok(model.regions.includes("scene"));
  assert.ok(model.regions.includes("composer"));
});

test("layout switches across compact medium and wide widths", () => {
  assert.equal(layoutForWidth(60).mode, "compact");
  assert.equal(layoutForWidth(100).mode, "medium");
  assert.equal(layoutForWidth(140).mode, "wide");
});

test("compact fallback preserves playable scene choices and dialogue", () => {
  const text = renderCompactGameView({
    revision: 5,
    scene_title: "Station overpass",
    scene: "Rain hits the roof.",
    figure_title: "Rei",
    figure: "She waits.",
    status: ["relationship: -100"],
    items: [],
    tasks: [],
    dialogue: ["I cannot keep guessing."],
    choices: ["Apologize", "Let her leave"],
    validation: "ready",
  });
  assert.match(text, /Station overpass/);
  assert.match(text, /I cannot keep guessing/);
  assert.match(text, /Apologize/);
});

test("action composer submits only non-empty player input", () => {
  assert.equal(createActionComposerState().canSubmit, false);
  assert.equal(updateActionComposer("  I apologize.  ").canSubmit, true);
});
