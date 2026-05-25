import { test } from "node:test";

import { assert } from "./test-harness.js";
import {
  developerToolProfile,
  installPlayerActiveTools,
  isPlayerToolAllowed,
  playerToolProfile,
  preservesPlayerToolProfile,
} from "../extensions/active-tools.js";
import { createInitialGameState, setDiagnosticsVisible } from "../extensions/state.js";
import { gameToolNames } from "../extensions/tools.js";

test("player profile exposes only game-safe tools", () => {
  assert.deepEqual(playerToolProfile.activeTools, gameToolNames);
  assert.equal(isPlayerToolAllowed("game_commit_turn"), true);
  assert.equal(isPlayerToolAllowed("shell"), false);
  assert.equal(isPlayerToolAllowed("file_edit"), false);
  assert.equal(isPlayerToolAllowed("git_apply_patch"), false);
  assert.equal(isPlayerToolAllowed("package_install"), false);
  assert.equal(isPlayerToolAllowed("provider_config"), false);
});

test("developer diagnostics do not remove player game tools", () => {
  assert.deepEqual(developerToolProfile.activeTools, playerToolProfile.activeTools);
  assert.deepEqual(developerToolProfile.developerOnlyTools, ["genmicon_diagnostics"]);
  assert.equal(preservesPlayerToolProfile(playerToolProfile, developerToolProfile), true);
});

test("player launch installs the player active-tool allowlist", () => {
  const installed: string[][] = [];
  const profile = installPlayerActiveTools({
    setActiveTools: (tools) => installed.push([...tools]),
  });
  assert.equal(profile.mode, "player");
  assert.deepEqual(installed, [[...gameToolNames]]);
});

test("diagnostics visibility does not widen player active tools", () => {
  const state = createInitialGameState();
  const diagnostics = setDiagnosticsVisible(state, true);
  assert.deepEqual(diagnostics.activeToolProfile.activeTools, state.activeToolProfile.activeTools);
  assert.deepEqual(diagnostics.activeToolProfile.developerOnlyTools, state.activeToolProfile.developerOnlyTools);
});
