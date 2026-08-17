import type { Metadata } from "next";
import { locales } from "./i18n/config";

/** Canonical origin for the production site (no trailing slash). */
export const SITE_URL = "https://codewhale.net";

export const SITE_NAME = "Codewhale";

/** The one-line product identity, used as the default OG image alt text. */
export const IDENTITY_PHRASE = "Codewhale dives into the deep so you don't have to.";

/** Accessible text for the shared Open Graph card. */
export const OG_ALT = IDENTITY_PHRASE;

/** Shared OG card rendered by app/opengraph-image.tsx (1200×630 PNG). */
const OG_IMAGE = {
  url: `${SITE_URL}/opengraph-image`,
  width: 1200,
  height: 630,
  alt: OG_ALT,
};

/** Open Graph locale codes per routed locale (BCP 47 with underscore). */
const OG_LOCALE: Record<string, string> = {
  en: "en_US",
  zh: "zh_CN",
  ja: "ja_JP",
  vi: "vi_VN",
  ko: "ko_KR",
  ru: "ru_RU",
  uk: "uk_UA",
  es: "es_ES",
  fr: "fr_FR",
  de: "de_DE",
  ca: "ca_ES",
  hi: "hi_IN",
  tr: "tr_TR",
  it: "it_IT",
  pl: "pl_PL",
  ar: "ar_AR",
  "pt-BR": "pt_BR",
  id: "id_ID",
};

/**
 * buildPageMetadata — per-page SEO metadata for the localized site.
 *
 * Produces a canonical URL for the rendered locale, hreflang alternates for
 * every routed locale (derived from `locales` in lib/i18n/config.ts, plus
 * `x-default` pointing at the English page), and matching Open Graph /
 * Twitter card fields wired to the shared OG image.
 *
 * @param path        Route path WITHOUT the locale prefix, with a leading
 *                    slash: "/" for the homepage, "/install", "/docs", …
 * @param locale      Locale of the page being rendered (a routed locale).
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
 *     title: isZh ? "安装 · Codewhale" : "Install · Codewhale",
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

  // hreflang alternates derive from the canonical locale registry, so a new
  // routed locale appears on every page without touching call sites.
  const languages: Record<string, string> = {};
  for (const l of locales) {
    languages[l] = `${SITE_URL}/${l}${suffix}`;
  }
  languages["x-default"] = `${SITE_URL}/en${suffix}`;

  return {
    metadataBase: new URL(SITE_URL),
    title,
    description,
    alternates: {
      canonical,
      languages,
    },
    openGraph: {
      title,
      description,
      url: canonical,
      siteName: SITE_NAME,
      type: "website",
      locale: OG_LOCALE[locale] ?? "en_US",
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
