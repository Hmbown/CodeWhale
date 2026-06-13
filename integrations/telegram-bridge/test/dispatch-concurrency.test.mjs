import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

async function readBridgeSource() {
  return fs.readFile(path.join(__dirname, "../src/index.mjs"), "utf8");
}

function extractFunction(source, name) {
  const asyncMarker = `async function ${name}`;
  const marker = source.includes(asyncMarker) ? asyncMarker : `function ${name}`;
  const start = source.indexOf(marker);
  assert.notEqual(start, -1, `${name} should exist`);

  let depth = 0;
  let opened = false;
  const bodyStart = source.indexOf("{", source.indexOf(")", start));
  for (let index = bodyStart; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") {
      depth += 1;
      opened = true;
    } else if (char === "}") {
      depth -= 1;
      if (opened && depth === 0) {
        return source.slice(start, index + 1);
      }
    }
  }
  assert.fail(`${name} body should close`);
}

test("prompt command starts a tracked background turn instead of blocking update dispatch", async () => {
  const source = await readBridgeSource();
  const handleCommand = extractFunction(source, "handleCommand");
  const promptCase = handleCommand.slice(handleCommand.indexOf('case "prompt":'));

  assert.match(source, /const activeTurnTasks = new Map\(\);/);
  assert.match(promptCase, /startPromptTurn\(chatId, action\.prompt\);/);
  assert.doesNotMatch(promptCase, /await\s+runPrompt\(/);

  const starter = extractFunction(source, "startPromptTurn");
  assert.ok(
    starter.indexOf("activeTurnTasks.set(chatId") < starter.indexOf("void runPrompt"),
    "turn registry entry must be installed before runPrompt can await"
  );
});

test("stale callback acknowledgements cannot skip modal actions", async () => {
  const source = await readBridgeSource();
  const callbackHandler = extractFunction(source, "handleCallbackQuery");

  assert.doesNotMatch(callbackHandler, /await\s+answerCallback\(query\.id,\s*"Working\.\.\."\)/);
  assert.match(callbackHandler, /answerCallback\(query\.id,\s*"Working\.\.\."\)\.catch/);
  assert.match(callbackHandler, /await handleModalAction\(identity\.chatId, action, query\);/);
});

test("reattached streams are detached and shutdown preserves active turn state", async () => {
  const source = await readBridgeSource();
  const reattach = extractFunction(source, "reattachActiveTurns");
  const runPrompt = extractFunction(source, "runPrompt");

  assert.match(reattach, /startTrackedTurnStream\(chatId, state\.threadId, turnId, sinceSeq\);/);
  assert.doesNotMatch(reattach, /await\s+streamTurnEvents\(/);
  assert.match(source, /async function clearActiveTurn\(chatId\)/);
  assert.match(runPrompt, /if \(!stopping\) {\s*await clearActiveTurn\(chatId\);\s*}/);

  const trackedStream = extractFunction(source, "startTrackedTurnStream");
  assert.match(trackedStream, /if \(!stopping\) {\s*await clearActiveTurn\(chatId\);\s*}/);
});
