import type { MetadataRoute } from "next";
import { locales } from "@/lib/i18n/config";
import { SITE_URL } from "@/lib/page-meta";

// Public, indexable routes (locale-prefixed). /admin and /api are
// intentionally excluded; see app/robots.ts.
const PATHS = ["", "/install", "/constitution", "/models", "/runtime", "/docs", "/docs/constitution", "/docs/modes", "/docs/tools", "/faq", "/roadmap", "/feed", "/digest", "/contribute", "/community"];

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
