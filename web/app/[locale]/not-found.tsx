import { NotFoundRoute } from "@/components/route-state";

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
