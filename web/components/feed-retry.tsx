"use client";

import { useRouter } from "next/navigation";
import { RetryAction } from "@/components/retry-action";

/**
 * The feed page's retry. `/feed` is ISR (600s), so a bare server re-render
 * would serve the same cached `skipped`/`unavailable` record the visitor is
 * looking at; POST to the retry endpoint first to bust the cached route,
 * then refresh so the server render happens against fresh data.
 */
export function FeedRetry({ label }: { label: string }) {
  const router = useRouter();
  return (
    <RetryAction
      label={label}
      onRetry={async () => {
        try {
          await fetch("/api/github/feed/retry", { method: "POST", cache: "no-store" });
        } finally {
          router.refresh();
        }
      }}
    />
  );
}
