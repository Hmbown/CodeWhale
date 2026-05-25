import { test } from "node:test";

import { assert } from "./test-harness.js";
import {
  applyDiagnosticsMode,
  createInitialGameState,
  diagnosticsStatusMessage,
  parseDiagnosticsMode,
  setDiagnosticsVisible,
} from "../extensions/state.js";

test("initial package state is reviewed local player mode", () => {
  const state = createInitialGameState();
  assert.equal(state.reviewStatus, "reviewed");
  assert.equal(state.diagnosticsVisible, false);
  assert.equal(state.activeToolProfile.mode, "player");
  assert.ok(state.resources.extensions.includes("extensions/index.ts"));
});

test("diagnostics toggle preserves active tool profile", () => {
  const state = createInitialGameState();
  const toggled = setDiagnosticsVisible(state, true);
  assert.equal(toggled.diagnosticsVisible, true);
  assert.equal(toggled.activeToolProfile, state.activeToolProfile);
});

test("diagnostics mode parser and application are explicit", () => {
  const state = createInitialGameState();
  assert.equal(parseDiagnosticsMode("on"), "on");
  assert.equal(parseDiagnosticsMode("off"), "off");
  assert.equal(parseDiagnosticsMode("unexpected"), "status");
  assert.equal(applyDiagnosticsMode(state, "on").diagnosticsVisible, true);
  assert.equal(applyDiagnosticsMode(state, "off").diagnosticsVisible, false);
  assert.equal(applyDiagnosticsMode(state, "status"), state);
});

test("diagnostics status message reports visibility", () => {
  assert.equal(diagnosticsStatusMessage(createInitialGameState()), "GENmicon diagnostics off.");
  assert.equal(diagnosticsStatusMessage(setDiagnosticsVisible(createInitialGameState(), true)), "GENmicon diagnostics on.");
});
