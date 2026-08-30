import { notFound } from "next/navigation";

/**
 * Catch-all inside the locale shell. Any path that matches no page raises
 * `notFound()` here, so the locale's `not-found.tsx` — nav, footer, and the
 * shared not-found state in the reader's language — answers instead of the
 * framework's bare 404. Dynamic by design (no static params): it only runs
 * for URLs no static page owns.
 */
export default function LocaleCatchAll() {
  notFound();
}
