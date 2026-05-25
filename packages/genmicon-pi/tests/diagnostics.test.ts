import { test } from "node:test";

import { assert } from "./test-harness.js";
import { createInitialGameState } from "../extensions/state.js";
import {
  buildDiagnosticRows,
  createDiagnosticPanelModel,
  formatDiagnostics,
} from "../extensions/ui/diagnostics.js";

test("diagnostic panel exposes package resources tools save driver and warnings", () => {
  const state = {
    ...createInitialGameState(),
    saveId: "default",
    saveRevision: 5,
    driverId: "galgame",
    driverVersion: "0.1.0",
    warnings: ["optional asset missing"],
  };
  const rows = buildDiagnosticRows(state, {
    renderSnapshot: { scene_title: "Station" },
    lastRuntimeCommand: "render",
  });
  const text = rows.map((row) => `${row.label}: ${row.value}`).join("\n");
  assert.match(text, /Package: \.\/packages\/genmicon-pi \(reviewed\)/);
  assert.match(text, /Active tools: game_status/);
  assert.match(text, /Save: default @ 5/);
  assert.match(text, /Driver: galgame@0.1.0/);
  assert.match(text, /Render: available/);
  assert.match(text, /Warnings: optional asset missing/);
});

test("diagnostic model visibility is explicit", () => {
  const model = createDiagnosticPanelModel(true, createInitialGameState());
  assert.equal(model.id, "genmicon.diagnostics");
  assert.equal(model.visible, true);
  assert.ok(model.sections.includes("warnings"));
  assert.ok(model.rows.length > 0);
});

test("diagnostic formatter is stable plain text", () => {
  const text = formatDiagnostics(createInitialGameState(), { lastRuntimeCommand: "status" });
  assert.match(text, /Last runtime: status/);
  assert.match(text, /Warnings: none/);
});
