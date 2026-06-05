import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

test("ThreadStore is initialized before bridge startup polls Telegram", async () => {
  const source = await fs.readFile(path.join(__dirname, "../src/index.mjs"), "utf8");
  const declaration = source.indexOf("class ThreadStore");
  const startupUse = source.indexOf("await ThreadStore.open");
  const pollCall = source.indexOf("await pollTelegram()");
  const reattachCall = source.indexOf("reattachActiveTurns().catch");

  assert.notEqual(declaration, -1);
  assert.notEqual(startupUse, -1);
  assert.notEqual(pollCall, -1);
  assert.notEqual(reattachCall, -1);
  assert.ok(declaration < startupUse);
  assert.ok(startupUse < reattachCall);
  assert.ok(reattachCall < pollCall);
});
