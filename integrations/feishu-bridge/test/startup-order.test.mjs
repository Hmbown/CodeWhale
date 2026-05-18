import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

test("ThreadStore is initialized before bridge startup opens it", async () => {
  const source = await fs.readFile(path.join(__dirname, "../src/index.mjs"), "utf8");
  const declaration = source.indexOf("class ThreadStore");
  const startupUse = source.indexOf("await ThreadStore.open");

  assert.notEqual(declaration, -1);
  assert.notEqual(startupUse, -1);
  assert.ok(declaration < startupUse);
});
