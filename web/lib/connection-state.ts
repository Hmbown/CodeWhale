/**
 * connection-state.ts — the typed connection model behind the signed-in
 * shell's offline/reconnect banner (`components/connection-banner.tsx`).
 *
 * Pure: a reducer over browser and probe events, plus the retry schedule.
 * The component owns timers and `fetch`; this module owns the meaning of
 * each state so it can be unit-tested without a DOM and so no surface can
 * invent a fourth kind of "online".
 *
 *   online       — the browser reports a network and the last probe (if any)
 *                  succeeded. No banner.
 *   offline      — `navigator.onLine` is false. Banner; probes wait for the
 *                  browser's `online` event before retrying.
 *   reconnecting — the browser reports a network but the server has not
 *                  answered a probe yet. Banner with the attempt count;
 *                  probes retry on a capped exponential backoff.
 *   degraded     — a probe failed while the browser still reports a network.
 *                  Banner; the next probe is scheduled.
 */

export type ConnectionStatus = "online" | "offline" | "reconnecting" | "degraded";

export interface ConnectionState {
  status: ConnectionStatus;
  /** Failed or pending probes since the last success. 0 while online. */
  attempt: number;
  /** Epoch ms of the last completed probe, success or failure. */
  lastCheckedAt: number | null;
  /** True for one render cycle after a recovery, so the banner can say so. */
  restored: boolean;
}

export type ConnectionEvent =
  | { type: "browser-online" }
  | { type: "browser-offline" }
  | { type: "probe-start" }
  | { type: "probe-ok"; at: number }
  | { type: "probe-failed"; at: number }
  | { type: "restored-seen" };

export const INITIAL_CONNECTION_STATE: ConnectionState = {
  status: "online",
  attempt: 0,
  lastCheckedAt: null,
  restored: false,
};

/** Base delay and ceiling for the reconnect schedule, in milliseconds. */
export const RETRY_BASE_MS = 2_000;
export const RETRY_MAX_MS = 30_000;

/**
 * Delay before the next probe for a given attempt count: 2s, 4s, 8s, 16s,
 * then 30s forever. Attempt 0 (first failure) retries after the base delay.
 */
export function backoffDelayMs(attempt: number): number {
  const exponent = Math.max(0, Math.min(attempt, 10));
  return Math.min(RETRY_MAX_MS, RETRY_BASE_MS * 2 ** exponent);
}

export function nextConnectionState(
  state: ConnectionState,
  event: ConnectionEvent,
): ConnectionState {
  switch (event.type) {
    case "browser-offline":
      return { ...state, status: "offline", restored: false };

    case "browser-online":
      // The browser thinks it has a network; the server has not confirmed.
      if (state.status === "online") return state;
      return { ...state, status: "reconnecting", restored: false };

    case "probe-start":
      if (state.status === "offline") return state;
      return {
        ...state,
        status: state.status === "online" ? "online" : "reconnecting",
      };

    case "probe-ok": {
      // A success that lands after the browser went `offline` belongs to a
      // stale probe: the browser emits no second event, so honoring it would
      // hide the banner with no network to back it. Record the check, keep
      // the banner.
      if (state.status === "offline") {
        return { ...state, lastCheckedAt: event.at };
      }
      const wasDown = state.status !== "online";
      return {
        status: "online",
        attempt: 0,
        lastCheckedAt: event.at,
        restored: wasDown,
      };
    }

    case "probe-failed":
      if (state.status === "offline") {
        return { ...state, lastCheckedAt: event.at };
      }
      return {
        status: "degraded",
        attempt: state.attempt + 1,
        lastCheckedAt: event.at,
        restored: false,
      };

    case "restored-seen":
      return state.restored ? { ...state, restored: false } : state;
  }
}

/** Whether a probe should be scheduled from this state. */
export function shouldProbe(state: ConnectionState): boolean {
  return state.status === "reconnecting" || state.status === "degraded";
}
