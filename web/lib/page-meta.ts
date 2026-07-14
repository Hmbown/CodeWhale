import type { Metadata } from "next";

/** Canonical origin for the production site (no trailing slash). */
export const SITE_URL = "https://codewhale.net";

export const SITE_NAME = "Codewhale";

/** The one-line product identity, used as the default OG image alt text. */
export const IDENTITY_PHRASE =
  "Documentation and source for the open-source Codewhale terminal coding runtime.";

/** Shared OG card rendered by app/opengraph-image.tsx (1200×630 PNG). */
const OG_IMAGE = {
  url: `${SITE_URL}/opengraph-image`,
  width: 1200,
  height: 630,
  alt: `${SITE_NAME} — ${IDENTITY_PHRASE}`,
};

/**
 * buildPageMetadata — per-page SEO metadata for the bilingual (en/zh) site.
 *
 * Produces a canonical URL for the rendered locale, hreflang alternates for
 * both locales (plus `x-default` pointing at the English page), and matching
 * Open Graph / Twitter card fields wired to the shared OG image.
 *
 * @param path        Route path WITHOUT the locale prefix, with a leading
 *                    slash: "/" for the homepage, "/install", "/docs", …
 * @param locale      Locale of the page being rendered: "en" | "zh".
 * @param title       Localized page <title> (full string; no template is applied).
 * @param description Localized meta description, same locale as `title`.
 *
 * Usage in a page or layout:
 * ```ts
 * export async function generateMetadata({ params }) {
 *   const { locale } = await params;
 *   const isZh = locale === "zh";
 *   return buildPageMetadata({
 *     path: "/install",
 *     locale,
 *     title: isZh ? "安装 · CodeWhale" : "Install · CodeWhale",
 *     description: isZh ? "…" : "…",
 *   });
 * }
 * ```
 */
export function buildPageMetadata({
  path,
  locale,
  title,
  description,
}: {
  path: string;
  locale: string;
  title: string;
  description: string;
}): Metadata {
  // "/" → "" so the homepage canonical is /en, not /en/.
  const suffix = path === "/" ? "" : path.replace(/\/+$/, "");
  const canonical = `${SITE_URL}/${locale}${suffix}`;

  return {
    metadataBase: new URL(SITE_URL),
    title,
    description,
    alternates: {
      canonical,
      languages: {
        en: `${SITE_URL}/en${suffix}`,
        zh: `${SITE_URL}/zh${suffix}`,
        "x-default": `${SITE_URL}/en${suffix}`,
      },
    },
    openGraph: {
      title,
      description,
      url: canonical,
      siteName: SITE_NAME,
      type: "website",
      locale: locale === "zh" ? "zh_CN" : "en_US",
      images: [OG_IMAGE],
    },
    twitter: {
      card: "summary_large_image",
      title,
      description,
      images: [OG_IMAGE.url],
    },
  };
}
