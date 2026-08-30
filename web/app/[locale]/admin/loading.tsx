import { LoadingRoute } from "@/components/route-state";

/**
 * Segment loading boundary: this page fetches at request time (GitHub or
 * KV), so a slow fetch streams the shared loading plate instead of a blank
 * field. Kept off the locale root on purpose — a root boundary would turn
 * the catch-all's 404 into a streamed 200.
 */
export default function Loading() {
  return (
    <div className="route-state">
      <LoadingRoute />
    </div>
  );
}
