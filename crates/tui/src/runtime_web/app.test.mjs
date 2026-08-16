// Automated coverage for the dashboard's target-resolution rail (#4397).
//
// These functions are the only thing standing between "the user clicked a row"
// and "a reply or an approval was POSTed somewhere". Every branch here is a
// fail-closed decision, so every branch is asserted: a saved session is never
// replied to, a stale selection never sends, and an approval that is not in the
// live thread's own approval set is never answered.
//
// Run by `npm test` in `web/` — see `web/vitest.config.ts`, whose `include`
// reaches this file deliberately so the rail cannot regress without CI saying
// so. The module is import-safe outside a browser: `app.mjs` only calls
// `startBrowserClient()` when `document` exists.

import { describe, expect, it } from "vitest";

import {
  NO_TARGET,
  canReply,
  refusalMessage,
  receiptPresentation,
  resolveApprovalTarget,
  resolveReplyTarget,
  sessionTarget,
  streamCursor,
  threadTarget,
} from "./app.mjs";

describe("receiptPresentation", () => {
  it("keeps a failed MCP transport compact while preserving the raw receipt", () => {
    const raw = "Failed to connect MCP server 'github': Stdio transport closed MCP server stderr (last 1 line): Docker is not running";
    expect(receiptPresentation({ kind: "status", status: "completed", summary: raw })).toEqual({
      label: "MCP · Unavailable",
      summary: "github could not connect",
      raw,
      failed: true,
    });
  });

  it("does not rewrite ordinary receipts", () => {
    expect(receiptPresentation({ kind: "tool_result", status: "completed", summary: "3 tests passed" })).toEqual({
      label: "Tool Result · Completed",
      summary: "3 tests passed",
      raw: "3 tests passed",
      failed: false,
    });
  });
});

/** Minimal stand-in for the live stream state the real client threads through. */
function streamState(threadId, approvalIds = []) {
  return { threadId, approvals: new Map(approvalIds.map((id) => [id, {}])) };
}

describe("canReply", () => {
  it("admits only a live thread with an id", () => {
    expect(canReply(threadTarget("thread-a"))).toBe(true);
    expect(canReply(sessionTarget("session-a"))).toBe(false);
    expect(canReply(NO_TARGET)).toBe(false);
    expect(canReply(threadTarget(""))).toBe(false);
    expect(canReply(undefined)).toBe(false);
  });
});

describe("resolveReplyTarget", () => {
  it("resolves a live thread that the stream is following", () => {
    const resolved = resolveReplyTarget(threadTarget("thread-a"), streamState("thread-a"));
    expect(resolved).toEqual({ ok: true, threadId: "thread-a" });
  });

  it("resolves a live thread before any stream state exists", () => {
    // A freshly created thread has no stream yet; refusing here would make the
    // first message of every new thread impossible to send.
    expect(resolveReplyTarget(threadTarget("thread-a"), null)).toEqual({
      ok: true,
      threadId: "thread-a",
    });
    expect(resolveReplyTarget(threadTarget("thread-a"), { threadId: "" })).toEqual({
      ok: true,
      threadId: "thread-a",
    });
  });

  it("refuses a saved session — a recording has no runtime to receive a reply", () => {
    expect(resolveReplyTarget(sessionTarget("session-a"), streamState("thread-a"))).toEqual({
      ok: false,
      reason: "session-not-live",
    });
  });

  it("refuses when nothing is selected", () => {
    expect(resolveReplyTarget(NO_TARGET, streamState("thread-a"))).toEqual({
      ok: false,
      reason: "no-target",
    });
    expect(resolveReplyTarget(null, streamState("thread-a"))).toEqual({
      ok: false,
      reason: "no-target",
    });
    expect(resolveReplyTarget(threadTarget(""), streamState("thread-a"))).toEqual({
      ok: false,
      reason: "no-target",
    });
  });

  it("refuses a stale target: the stream moved on while the user did not", () => {
    expect(resolveReplyTarget(threadTarget("thread-a"), streamState("thread-b"))).toEqual({
      ok: false,
      reason: "stale-target",
    });
  });

  it("never returns an id on refusal", () => {
    for (const [target, state] of [
      [sessionTarget("session-a"), streamState("thread-a")],
      [NO_TARGET, streamState("thread-a")],
      [threadTarget("thread-a"), streamState("thread-b")],
    ]) {
      expect(resolveReplyTarget(target, state).threadId).toBeUndefined();
    }
  });
});

describe("resolveApprovalTarget", () => {
  it("resolves an approval the watched thread actually holds", () => {
    const state = streamState("thread-a", ["approval-1"]);
    expect(resolveApprovalTarget("approval-1", threadTarget("thread-a"), state)).toEqual({
      ok: true,
      threadId: "thread-a",
      approvalId: "approval-1",
    });
  });

  it("inherits every reply refusal — authority cannot outrank the target check", () => {
    const state = streamState("thread-a", ["approval-1"]);
    expect(resolveApprovalTarget("approval-1", sessionTarget("session-a"), state).reason).toBe(
      "session-not-live",
    );
    expect(resolveApprovalTarget("approval-1", NO_TARGET, state).reason).toBe("no-target");
    expect(
      resolveApprovalTarget("approval-1", threadTarget("thread-b"), state).reason,
    ).toBe("stale-target");
  });

  it("refuses an approval that is not in the live approval set", () => {
    // Already decided, expired, or belonging to a thread we stopped watching.
    const state = streamState("thread-a", ["approval-1"]);
    expect(resolveApprovalTarget("approval-2", threadTarget("thread-a"), state)).toEqual({
      ok: false,
      reason: "stale-approval",
    });
  });

  it("refuses when the stream state is missing or belongs to another thread", () => {
    expect(resolveApprovalTarget("approval-1", threadTarget("thread-a"), null)).toEqual({
      ok: false,
      reason: "stale-target",
    });
    // A reply would be allowed here (no stream yet), but an approval must not:
    // there is no live approval set to check membership against.
    expect(
      resolveApprovalTarget("approval-1", threadTarget("thread-a"), { threadId: "" }),
    ).toEqual({ ok: false, reason: "stale-target" });
  });

  it("refuses an unidentified approval", () => {
    const state = streamState("thread-a", ["approval-1"]);
    expect(resolveApprovalTarget("", threadTarget("thread-a"), state)).toEqual({
      ok: false,
      reason: "no-approval",
    });
  });

  it("refuses when the thread has no approvals at all", () => {
    expect(
      resolveApprovalTarget("approval-1", threadTarget("thread-a"), { threadId: "thread-a" }),
    ).toEqual({ ok: false, reason: "stale-approval" });
  });

  it("never returns an approval id on refusal", () => {
    const state = streamState("thread-a", ["approval-1"]);
    for (const [id, target] of [
      ["approval-2", threadTarget("thread-a")],
      ["approval-1", sessionTarget("session-a")],
      ["", threadTarget("thread-a")],
    ]) {
      expect(resolveApprovalTarget(id, target, state).approvalId).toBeUndefined();
    }
  });
});

describe("refusalMessage", () => {
  it("gives every refusal reason a distinct user-visible sentence", () => {
    const reasons = ["session-not-live", "stale-target", "stale-approval", "no-approval", "no-target"];
    const messages = reasons.map(refusalMessage);
    expect(new Set(messages).size).toBe(reasons.length);
    for (const message of messages) {
      // Every refusal must state the outcome, not just the cause: the user
      // needs to know their message did not go anywhere.
      expect(message).toContain("nothing was sent");
    }
  });

  it("falls back to the select-a-thread message for an unknown reason", () => {
    expect(refusalMessage("something-new")).toBe(refusalMessage("no-target"));
  });
});

describe("streamCursor", () => {
  it("reports the resume sequence and a live label", () => {
    expect(streamCursor({ latestSeq: 12 })).toEqual({
      latestSeq: 12,
      gap: false,
      connected: true,
      label: "Live — event #12",
    });
  });

  it("names a gap and a disconnect distinctly, both carrying the resume point", () => {
    expect(streamCursor({ latestSeq: 5 }, { gap: true }).label).toBe(
      "Gap detected — re-syncing from #5",
    );
    expect(streamCursor({ latestSeq: 5 }, { connected: false }).label).toBe(
      "Reconnecting — resuming from #5",
    );
  });

  it("normalizes a missing or nonsense sequence to zero rather than NaN", () => {
    expect(streamCursor(undefined).latestSeq).toBe(0);
    expect(streamCursor({ latestSeq: "not a number" }).latestSeq).toBe(0);
  });
});
