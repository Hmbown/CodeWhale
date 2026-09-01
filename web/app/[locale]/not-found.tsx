import type { Metadata } from "next";
import { NotFoundRoute } from "@/components/route-state";

// A not-found boundary receives no params, so the title is locale-neutral;
// without it the 404 would carry the home page's <title>. The rest of the
// object replaces — not extends — the inherited home metadata: a 404 must
// not advertise the home page as its canonical, hreflang, or share target.
export const metadata: Metadata = {
  title: "Not found · Codewhale",
  robots: { index: false, follow: true },
  alternates: {},
  openGraph: { title: "Not found · Codewhale" },
};

/**
 * Not-found boundary inside the locale shell, so a `notFound()` thrown by
 * any locale page renders with the site's nav, footer, and dictionary copy
 * instead of the framework's bare page.
 */
export default function NotFound() {
  return (
    <div className="route-state">
      <NotFoundRoute />
    </div>
  );
}
