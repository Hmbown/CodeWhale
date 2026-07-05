import type { MetadataRoute } from "next";
import { locales } from "@/lib/i18n/config";
import { SITE_URL } from "@/lib/page-meta";
import { DOC_TOPICS } from "@/lib/docs-map";

// Public, indexable routes (locale-prefixed). /admin and /api are
// intentionally excluded; see app/robots.ts. Docs sub-pages are derived from
// the docs-map registry so newly rendered topics are indexed automatically.
const PATHS = [
  "",
  "/install",
  "/constitution",
  "/models",
  "/runtime",
  "/docs",
  ...DOC_TOPICS.filter((t) => t.hasPage).map((t) => `/docs/${t.slug}`),
  "/faq",
  "/roadmap",
  "/feed",
  "/digest",
  "/contribute",
  "/community",
];

export default function sitemap(): MetadataRoute.Sitemap {
  const lastModified = new Date();
  return PATHS.flatMap((path) =>
    locales.map((locale) => ({
      url: `${SITE_URL}/${locale}${path}`,
      lastModified,
      alternates: {
        languages: {
          en: `${SITE_URL}/en${path}`,
          zh: `${SITE_URL}/zh${path}`,
        },
      },
    })),
  );
}
