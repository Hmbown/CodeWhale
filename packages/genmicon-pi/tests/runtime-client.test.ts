import { test } from "node:test";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { assert, withTempDir } from "./test-harness.js";
import {
  callRuntime,
  encodeRuntimeRequest,
  listGameSaves,
  loadResumeSnapshot,
  validateGameReadiness,
} from "../extensions/runtime-client.js";

test("runtime request encoding supplies defaults", () => {
  const encoded = encodeRuntimeRequest({
    command: "validate",
    game_root: "examples/games/reconciliation-demo",
  });
  const parsed = JSON.parse(encoded);
  assert.equal(parsed.developer, false);
  assert.deepEqual(parsed.payload, {});
});

test("runtime client reads one JSON response from a helper process", async () => {
  await withTempDir("runtime-client", async (dir) => {
    const helper = join(dir, "helper.mjs");
    writeFileSync(
      helper,
      "process.stdin.resume(); process.stdin.on('end', () => console.log(JSON.stringify({ ok: true, data: { ready: true }, warnings: [], error: null })));",
    );

    const response = await callRuntime<{ ready: boolean }>(
      { command: "validate", game_root: "game" },
      { binary: process.execPath, args: [helper] },
    );

    assert.equal(response.ok, true);
    assert.equal(response.data?.ready, true);
  });
});

test("validation readiness wrapper calls validate command", async () => {
  await withTempDir("validate-readiness", async (dir) => {
    const helper = join(dir, "helper.mjs");
    writeFileSync(
      helper,
      "let raw = ''; process.stdin.on('data', (chunk) => raw += chunk); process.stdin.on('end', () => { const request = JSON.parse(raw); console.log(JSON.stringify({ ok: request.command === 'validate', data: request, warnings: [], error: null })); });",
    );

    const response = await validateGameReadiness("game", {
      binary: process.execPath,
      args: [helper],
      saveId: "default",
    });

    assert.equal(response.ok, true);
    assert.equal((response.data as { command?: string }).command, "validate");
    assert.equal((response.data as { save_id?: string }).save_id, "default");
  });
});

test("save listing wrapper calls list_saves command", async () => {
  await withTempDir("list-saves", async (dir) => {
    const helper = join(dir, "helper.mjs");
    writeFileSync(
      helper,
      "let raw = ''; process.stdin.on('data', (chunk) => raw += chunk); process.stdin.on('end', () => { const request = JSON.parse(raw); console.log(JSON.stringify({ ok: request.command === 'list_saves', data: request, warnings: [], error: null })); });",
    );

    const response = await listGameSaves("game", {
      binary: process.execPath,
      args: [helper],
    });

    assert.equal(response.ok, true);
    assert.equal((response.data as { command?: string }).command, "list_saves");
  });
});

test("resume snapshot wrapper combines status and render responses", async () => {
  await withTempDir("resume-snapshot", async (dir) => {
    const helper = join(dir, "helper.mjs");
    writeFileSync(
      helper,
      "let raw = ''; process.stdin.on('data', (chunk) => raw += chunk); process.stdin.on('end', () => { const request = JSON.parse(raw); console.log(JSON.stringify({ ok: true, data: { command: request.command }, warnings: [], error: null })); });",
    );

    const response = await loadResumeSnapshot("game", {
      binary: process.execPath,
      args: [helper],
      saveId: "default",
    });

    assert.equal(response.ok, true);
    assert.equal((response.data as { status?: { command?: string } }).status?.command, "status");
    assert.equal((response.data as { render?: { command?: string } }).render?.command, "render");
  });
});
