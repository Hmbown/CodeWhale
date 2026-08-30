"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";

/**
 * The one retry control for shared error states. Two modes:
 *  - `onRetry` — call the caller's recovery (an error boundary's `reset`,
 *    a re-fetch, a re-probe);
 *  - no `onRetry` — re-run the server render for this route via
 *    `router.refresh()`, which is the honest retry for a page whose data
 *    is fetched on the server.
 *
 * `aria-busy` while the retry is in flight; the label never changes, so
 * a screen reader hears one control, not a sequence of them.
 */
export function RetryAction({
  label,
  onRetry,
  variant = "primary",
}: {
  label: string;
  onRetry?: () => void | Promise<void>;
  variant?: "primary" | "secondary";
}) {
  const router = useRouter();
  const [pending, startTransition] = useTransition();
  const [busy, setBusy] = useState(false);

  const run = async () => {
    if (onRetry) {
      setBusy(true);
      try {
        await onRetry();
      } finally {
        setBusy(false);
      }
      return;
    }
    startTransition(() => router.refresh());
  };

  const inFlight = pending || busy;
  return (
    <button
      type="button"
      onClick={run}
      className={`portal-button portal-button-${variant} state-retry`}
      aria-busy={inFlight}
      disabled={inFlight}
    >
      {label}
    </button>
  );
}
