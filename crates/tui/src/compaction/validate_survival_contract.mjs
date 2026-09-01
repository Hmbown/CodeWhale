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

function toolUseIds(message) {
  return (message.content ?? [])
    .filter((block) => block.type === "tool_use")
    .map((block) => block.id);
}

function isAssistantLike(message) {
  return message.role === "assistant" || message.role === "assistant_interrupted";
}

function assistantTextOf(message) {
  if (!isAssistantLike(message)) return null;
  const text = (message.content ?? [])
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("\n")
    .trim();
  return text || null;
}

// A retained copy may be truncated, so a prefix either way counts as survival
// -- but nothing weaker does.
function survives(text, replacement, of) {
  return replacement.some((message) => {
    const kept = of(message);
    return (
      kept &&
      (kept === text || text.startsWith(kept) || kept.startsWith(text))
    );
  });
}

export function validateSurvivalContract(original, replacement, anchors) {
  const start = lastRoundStart(original);
  const lastRound = original.slice(start);
  // Every user turn in the round, not the first one `find` reaches: the round
  // spans a tool-bearing turn plus the toolless tail after it, so checking one
  // let a rewrite drop the latest turn.
  for (const text of lastRound.map(userTextOf).filter(Boolean)) {
    if (!survives(text, replacement, userTextOf)) {
      return "a last-round user message was dropped";
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
  // The call, not just its result: a tool_result whose tool_use was summarized
  // away is an orphan providers reject.
  for (const id of lastRound.flatMap(toolUseIds)) {
    const kept = replacement.some((message) =>
      (message.content ?? []).some(
        (block) => block.type === "tool_use" && block.id === id,
      ),
    );
    if (!kept) {
      return `last-round tool call ${id} was dropped`;
    }
  }
  // Match the text: "some assistant message survived" is satisfied by the
  // summary the rewrite itself just wrote.
  for (const text of lastRound.map(assistantTextOf).filter(Boolean)) {
    if (!survives(text, replacement, assistantTextOf)) {
      return "last-round assistant output was dropped";
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
