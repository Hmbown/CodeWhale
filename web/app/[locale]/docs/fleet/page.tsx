import { permanentRedirect } from "next/navigation";

/**
 * Compatibility redirect: `/docs/fleet` was the canonical documentation URL
 * before Pod became the customer-facing name for the durable roster. The page
 * itself now lives at `/docs/pod` (`app/[locale]/docs/pod/page.tsx`) and is
 * the only entry in the sitemap and the docs map, so this route exists purely
 * to keep already-published links, bookmarks, and search results resolving.
 *
 * 308 rather than 307: the move is permanent, so a crawler should transfer
 * signal to `/docs/pod` instead of indexing both URLs.
 */
export default async function FleetDocsRedirect({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  permanentRedirect(`/${locale}/docs/pod`);
}
