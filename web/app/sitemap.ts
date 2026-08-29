import type { MetadataRoute } from "next";
import { contentLocalesForPath } from "@/lib/i18n/content-locales";
import { SITE_URL } from "@/lib/page-meta";

// Public, indexable routes (locale-prefixed). /admin and /api are
// intentionally excluded; see app/robots.ts.
const PATHS = ["", "/install", "/constitution", "/models", "/runtime", "/docs", "/docs/configuration", "/docs/constitution", "/docs/fleet", "/docs/guide", "/docs/hooks", "/docs/mcp", "/docs/modes", "/docs/runtime-api", "/docs/sandbox", "/docs/subagents", "/docs/tools", "/docs/troubleshooting", "/docs/vocabulary", "/docs/web", "/docs/work", "/faq", "/roadmap", "/feed", "/digest", "/contribute", "/community", "/pricing", "/legal/terms", "/legal/privacy"];

export default function sitemap(): MetadataRoute.Sitemap {
  return PATHS.flatMap((path) =>
    contentLocalesForPath(path || "/").map((locale) => ({
      url: `${SITE_URL}/${locale}${path}`,
      alternates: {
        languages: Object.fromEntries(
          contentLocalesForPath(path || "/").map((l) => [
            l,
            `${SITE_URL}/${l}${path}`,
          ]),
        ),
      },
    })),
  );
}
