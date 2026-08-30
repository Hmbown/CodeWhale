import { LoadingRoute } from "@/components/route-state";

/**
 * Segment loading boundary for the one request-time segment. /admin is
 * `force-dynamic` and reads KV on every request, so a slow read streams the
 * shared loading plate instead of a blank field.
 *
 * Deliberately the only loading.tsx under the locale root: a boundary on a
 * statically generated or ISR segment (docs, feed, digest, roadmap) makes the
 * served HTML carry the fallback with the real body in a hidden slot that
 * only a script swaps in — every no-JS reader and crawler would see
 * "Loading…" in place of the article. A root boundary would also turn the
 * catch-all's 404 into a streamed 200.
 */
export default function Loading() {
  return (
    <div className="route-state">
      <LoadingRoute />
    </div>
  );
}
