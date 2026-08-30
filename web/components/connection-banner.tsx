"use client";

import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import {
  INITIAL_CONNECTION_STATE,
  backoffDelayMs,
  nextConnectionState,
  shouldProbe,
  type ConnectionState,
} from "@/lib/connection-state";
import { fill, getChrome, getStates } from "@/lib/i18n/dictionaries";

/**
 * Offline / reconnect banner for the signed-in shell.
 *
 * State meaning lives in lib/connection-state.ts; this component owns the
 * browser events, the probe, and the timers. The probe is a real request to
 * a first-party endpoint (`/api/facts`, `Cache-Control: no-store`), so
 * "back online" means the server answered — never the browser's guess
 * alone. No data is faked while disconnected: the banner says actions are
 * paused, and the page underneath keeps whatever it last rendered.
 */
export function ConnectionBanner({
  locale,
  probeUrl = "/api/facts",
  /** Periodic heartbeat while online; 0 disables it. */
  heartbeatMs = 60_000,
}: {
  locale: string;
  probeUrl?: string;
  heartbeatMs?: number;
}) {
  const t = getStates(locale);
  const chrome = getChrome(locale);
  const [state, dispatch] = useReducer(nextConnectionState, INITIAL_CONNECTION_STATE);
  const [dismissedRestored, setDismissedRestored] = useState(false);
  const timer = useRef<number | null>(null);
  const inFlight = useRef(false);

  const probe = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    dispatch({ type: "probe-start" });
    try {
      const controller = new AbortController();
      const timeout = window.setTimeout(() => controller.abort(), 8_000);
      const res = await fetch(probeUrl, {
        method: "GET",
        cache: "no-store",
        signal: controller.signal,
        headers: { Accept: "application/json" },
      });
      window.clearTimeout(timeout);
      dispatch({ type: res.ok ? "probe-ok" : "probe-failed", at: Date.now() });
    } catch {
      dispatch({ type: "probe-failed", at: Date.now() });
    } finally {
      inFlight.current = false;
    }
  }, [probeUrl]);

  // Browser network events.
  useEffect(() => {
    if (typeof navigator !== "undefined" && navigator.onLine === false) {
      dispatch({ type: "browser-offline" });
    }
    const onOnline = () => {
      dispatch({ type: "browser-online" });
      void probe();
    };
    const onOffline = () => dispatch({ type: "browser-offline" });
    window.addEventListener("online", onOnline);
    window.addEventListener("offline", onOffline);
    return () => {
      window.removeEventListener("online", onOnline);
      window.removeEventListener("offline", onOffline);
    };
  }, [probe]);

  // Retry schedule while reconnecting/degraded; heartbeat while online.
  useEffect(() => {
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = null;
    if (shouldProbe(state)) {
      timer.current = window.setTimeout(() => void probe(), backoffDelayMs(state.attempt));
    } else if (state.status === "online" && heartbeatMs > 0) {
      timer.current = window.setTimeout(() => void probe(), heartbeatMs);
    }
    return () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    };
  }, [state, probe, heartbeatMs]);

  // "Back online" shows briefly, then clears itself.
  useEffect(() => {
    if (!state.restored) return;
    setDismissedRestored(false);
    const id = window.setTimeout(() => dispatch({ type: "restored-seen" }), 4_000);
    return () => window.clearTimeout(id);
  }, [state.restored]);

  const view = bannerView(state, dismissedRestored);
  if (!view) return null;

  const copy = {
    offline: { title: t.offlineTitle, body: t.offlineBody },
    reconnecting: {
      title: t.reconnectingTitle,
      body: fill(t.reconnectingBody, { attempt: state.attempt + 1 }),
    },
    degraded: { title: t.degradedTitle, body: t.degradedBody },
    restored: { title: t.onlineTitle, body: t.onlineBody },
  }[view];

  const lastChecked =
    state.lastCheckedAt !== null
      ? fill(t.lastChecked, {
          time: new Date(state.lastCheckedAt).toLocaleTimeString(chrome.dateLocale, {
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
          }),
        })
      : null;

  return (
    <div
      className={`connection-banner connection-banner-${view}`}
      role={view === "offline" ? "alert" : "status"}
      aria-live={view === "offline" ? "assertive" : "polite"}
      data-connection={state.status}
    >
      <span className="connection-mark" aria-hidden="true" />
      <div className="connection-copy">
        <p className="connection-title">{copy.title}</p>
        <p className="connection-body">
          {copy.body}
          {lastChecked && view !== "restored" && (
            <span className="connection-checked"> · {lastChecked}</span>
          )}
        </p>
      </div>
      <div className="connection-actions">
        {view === "restored" ? (
          <button
            type="button"
            className="connection-button"
            onClick={() => setDismissedRestored(true)}
          >
            {t.dismiss}
          </button>
        ) : (
          <button
            type="button"
            className="connection-button connection-button-primary"
            onClick={() => void probe()}
            disabled={state.status === "offline"}
            aria-busy={state.status === "reconnecting"}
          >
            {t.retryNow}
          </button>
        )}
      </div>
    </div>
  );
}

type BannerView = "offline" | "reconnecting" | "degraded" | "restored";

/** Which banner (if any) a state renders. Exported for tests. */
export function bannerView(state: ConnectionState, dismissedRestored: boolean): BannerView | null {
  if (state.status === "online") {
    return state.restored && !dismissedRestored ? "restored" : null;
  }
  return state.status;
}
