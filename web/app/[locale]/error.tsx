"use client";

import { useEffect } from "react";
import { ErrorRoute } from "@/components/route-state";

/**
 * Route-level error boundary for every locale page. `reset` re-renders the
 * segment, which is the honest retry for a server page whose data fetch
 * failed; the shim renders the shared error plate with dictionary copy and
 * a way home. The digest (Next's server-error id) is shown so a report can
 * name it; the raw message is not, because it may carry internals.
 */
export default function Error({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <div className="route-state">
      <ErrorRoute reset={reset} digest={error.digest} />
    </div>
  );
}
