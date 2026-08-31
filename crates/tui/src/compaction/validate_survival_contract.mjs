#!/usr/bin/env node
// Language-invariant coverage floor for the compaction survival contract.
// Later TypeScript strategies should keep this check; Rust remains the B1
// enforcement path. Run: node validate_survival_contract.mjs

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const MARKERS = [
  "Another language model started to solve this problem",
  "Conversation Summary (Auto-Generated)",
];

const root = dirname(fileURLToPath(import.meta.url));
const matrix = JSON.parse(
  readFileSync(join(root, "fixtures/matrix.json"), "utf8"),
);

function userTextOf(message) {
  if (message.role !== "user") return null;
  const text = (message.content ?? [])
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("\n")
    .trim();
  return text || null;
}

function isCheckpoint(message) {
  const text = userTextOf(message);
  return Boolean(text && MARKERS.some((marker) => text.includes(marker)));
}

function isPlainUserText(message) {
  return !isCheckpoint(message) && Boolean(userTextOf(message));
}

function lastPlainUserIndex(messages, end) {
  for (let idx = end - 1; idx >= 0; idx -= 1) {
    if (isPlainUserText(messages[idx])) return idx;
  }
  return null;
}

function sliceHasToolResult(messages, start) {
  return messages.slice(start).some((message) =>
    (message.content ?? []).some((block) => block.type === "tool_result"),
  );
}

export function lastRoundStart(messages) {
  const lastUser = lastPlainUserIndex(messages, messages.length);
  if (lastUser === null) return 0;
  if (sliceHasToolResult(messages, lastUser)) return lastUser;
  let candidate = lastUser;
  for (;;) {
    const prev = lastPlainUserIndex(messages, candidate);
    if (prev === null) return lastUser;
    if (sliceHasToolResult(messages, prev)) return prev;
    candidate = prev;
  }
}

function toolResultIds(message) {
  return (message.content ?? [])
    .filter((block) => block.type === "tool_result")
    .map((block) => block.tool_use_id);
}

function isAssistantLike(message) {
  return message.role === "assistant" || message.role === "assistant_interrupted";
}

export function validateSurvivalContract(original, replacement, anchors) {
  const start = lastRoundStart(original);
  const lastRound = original.slice(start);
  const lastUserText = lastRound.map(userTextOf).find(Boolean);
  if (lastUserText) {
    const kept = replacement.some((message) => {
      const text = userTextOf(message);
      return (
        text &&
        (text === lastUserText ||
          lastUserText.startsWith(text) ||
          text.startsWith(lastUserText))
      );
    });
    if (!kept) {
      return "last user message was dropped";
    }
  }
  for (const id of lastRound.flatMap(toolResultIds)) {
    const kept = replacement.some((message) =>
      (message.content ?? []).some(
        (block) => block.type === "tool_result" && block.tool_use_id === id,
      ),
    );
    if (!kept) {
      return `last-round tool result ${id} was dropped`;
    }
  }
  if (
    lastRound.some(isAssistantLike) &&
    !replacement.some(isAssistantLike)
  ) {
    return "last-round assistant output was dropped";
  }
  const checkpoints = replacement.filter(isCheckpoint).length;
  if (checkpoints === 0) return "checkpoint receipt was dropped";
  if (checkpoints > 1) return "prior summaries were duplicated";
  if (anchors && !replacement.some((message) =>
    (message.content ?? []).some((block) => {
      if (block.type === "text") return (block.text ?? "").includes(anchors);
      if (block.type === "tool_result") {
        return (block.content ?? "").includes(anchors);
      }
      return false;
    }),
  )) {
    return "pinned /anchor text was dropped";
  }
  return null;
}

function main() {
  if (matrix.schema_version !== 1) {
    throw new Error(`unexpected schema_version ${matrix.schema_version}`);
  }
  let failed = 0;
  for (const fixture of matrix.cases) {
    if (typeof fixture.last_round_start === "number") {
      const start = lastRoundStart(fixture.original);
      if (start !== fixture.last_round_start) {
        failed += 1;
        console.error(
          `${fixture.id}: last_round_start ${start} != ${fixture.last_round_start}`,
        );
      }
    }
    const error = validateSurvivalContract(
      fixture.original,
      fixture.replacement,
      fixture.anchors,
    );
    const passed = error === null;
    if (fixture.expect === "pass" && !passed) {
      failed += 1;
      console.error(`${fixture.id}: expected pass, got ${error}`);
    } else if (fixture.expect === "fail" && passed) {
      failed += 1;
      console.error(`${fixture.id}: expected fail closed`);
    }
  }
  if (failed > 0) {
    console.error(`${failed} fixture(s) failed`);
    process.exit(1);
  }
  console.log(`ok ${matrix.cases.length} survival-contract fixtures`);
}

const entry = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (entry || process.argv[1]?.endsWith("validate_survival_contract.mjs")) {
  main();
}
