import { permanentRedirect } from "next/navigation";

/**
 * Compatibility redirect: `/docs/pod` is the accepted alias for the canonical
 * Fleet documentation URL at `/docs/fleet`. This route exists purely to keep
 * already-published links, bookmarks, and search results resolving.
 *
 * 308 rather than 307: the move is permanent, so a crawler should transfer
 * signal to `/docs/fleet` instead of indexing both URLs.
 */
export default async function PodDocsRedirect({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  permanentRedirect(`/${locale}/docs/fleet`);
}
