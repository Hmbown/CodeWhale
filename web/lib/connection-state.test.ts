import { describe, expect, it } from "vitest";
import {
  INITIAL_CONNECTION_STATE,
  RETRY_BASE_MS,
  RETRY_MAX_MS,
  backoffDelayMs,
  nextConnectionState,
  shouldProbe,
  type ConnectionState,
} from "./connection-state";

function run(events: Parameters<typeof nextConnectionState>[1][]): ConnectionState {
  return events.reduce(nextConnectionState, INITIAL_CONNECTION_STATE);
}

describe("connection state", () => {
  it("starts online and stays online on successful probes", () => {
    const state = run([{ type: "probe-start" }, { type: "probe-ok", at: 10 }]);
    expect(state).toEqual({ status: "online", attempt: 0, lastCheckedAt: 10, restored: false });
    expect(shouldProbe(state)).toBe(false);
  });

  it("goes offline on the browser event and does not probe until the browser returns", () => {
    const offline = run([{ type: "browser-offline" }]);
    expect(offline.status).toBe("offline");
    expect(shouldProbe(offline)).toBe(false);
    // A probe that happens to fail while offline records the time but does
    // not count as a reconnect attempt.
    const still = nextConnectionState(offline, { type: "probe-failed", at: 5 });
    expect(still.status).toBe("offline");
    expect(still.attempt).toBe(0);
    expect(still.lastCheckedAt).toBe(5);
    // The same for a probe that was already in flight when the browser went
    // offline and lands successfully afterward: the browser may emit no
    // second event, so a late success must not hide the banner.
    const staleOk = nextConnectionState(offline, { type: "probe-ok", at: 6 });
    expect(staleOk.status).toBe("offline");
    expect(staleOk.attempt).toBe(0);
    expect(staleOk.lastCheckedAt).toBe(6);
  });

  it("reconnects through the server, not on the browser's word alone", () => {
    const back = run([{ type: "browser-offline" }, { type: "browser-online" }]);
    expect(back.status).toBe("reconnecting");
    expect(shouldProbe(back)).toBe(true);
    const failed = nextConnectionState(back, { type: "probe-failed", at: 20 });
    expect(failed.status).toBe("degraded");
    expect(failed.attempt).toBe(1);
    const again = nextConnectionState(failed, { type: "probe-start" });
    expect(again.status).toBe("reconnecting");
    const ok = nextConnectionState(again, { type: "probe-ok", at: 30 });
    expect(ok).toEqual({ status: "online", attempt: 0, lastCheckedAt: 30, restored: true });
    expect(nextConnectionState(ok, { type: "restored-seen" }).restored).toBe(false);
  });

  it("marks a failed probe while apparently online as degraded", () => {
    const state = run([{ type: "probe-failed", at: 1 }]);
    expect(state.status).toBe("degraded");
    expect(state.attempt).toBe(1);
    expect(shouldProbe(state)).toBe(true);
  });

  it("schedules retries on a capped exponential backoff", () => {
    expect(backoffDelayMs(0)).toBe(RETRY_BASE_MS);
    expect(backoffDelayMs(1)).toBe(RETRY_BASE_MS * 2);
    expect(backoffDelayMs(2)).toBe(RETRY_BASE_MS * 4);
    expect(backoffDelayMs(3)).toBe(RETRY_BASE_MS * 8);
    expect(backoffDelayMs(4)).toBe(RETRY_MAX_MS);
    expect(backoffDelayMs(50)).toBe(RETRY_MAX_MS);
  });
});
