import type { MetadataRoute } from "next";
import { contentLocalesForPath } from "@/lib/i18n/content-locales";
import { SITE_URL } from "@/lib/page-meta";

// Public, indexable routes (locale-prefixed). /admin and /api are
// intentionally excluded; see app/robots.ts.
const PATHS = ["", "/install", "/constitution", "/models", "/runtime", "/docs", "/docs/auth", "/docs/computers", "/docs/configuration", "/docs/constitution", "/docs/guide", "/docs/hooks", "/docs/mcp", "/docs/modes", "/docs/fleet", "/docs/runtime-api", "/docs/sandbox", "/docs/subagents", "/docs/tools", "/docs/troubleshooting", "/docs/trust", "/docs/vocabulary", "/docs/web", "/docs/work", "/faq", "/roadmap", "/feed", "/digest", "/changelog", "/contribute", "/community", "/pricing", "/signin", "/signup", "/legal/terms", "/legal/privacy"];

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
