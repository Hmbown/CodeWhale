import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { locales } from "./i18n/config";
import { contentLocalesForPath } from "./i18n/content-locales";
import { buildPageMetadata, IDENTITY_PHRASE, OG_ALT, SITE_NAME, SITE_URL } from "./page-meta";

/** hreflang alternates derive from genuine page-body translation coverage. */
function expectedLanguages(path: string): Record<string, string> {
  const suffix = path === "/" ? "" : path;
  const languages: Record<string, string> = {};
  for (const l of contentLocalesForPath(path)) {
    languages[l] = `${SITE_URL}/${l}${suffix}`;
  }
  languages["x-default"] = `${SITE_URL}/en${suffix}`;
  return languages;
}

describe("page metadata", () => {
  it.each([
    ["en", "/faq", "FAQ · Codewhale", "en_US", "en"],
    ["zh", "/faq", "常见问题 · Codewhale", "zh_CN", "zh"],
    ["en", "/feed", "Activity · Codewhale", "en_US", "en"],
    ["zh", "/feed", "动态 · Codewhale", "zh_CN", "zh"],
    ["en", "/roadmap", "Roadmap · Codewhale", "en_US", "en"],
    ["zh", "/roadmap", "路线图 · Codewhale", "zh_CN", "zh"],
    ["ru", "/faq", "FAQ · Codewhale", "en_US", "en"],
    ["uk", "/faq", "FAQ · Codewhale", "en_US", "en"],
    ["pt-BR", "/install", "Install · Codewhale", "en_US", "en"],
    ["ja", "/", "Codewhale", "ja_JP", "ja"],
  ])(
    "builds canonical, hreflang, Open Graph, and Twitter fields for %s%s",
    (locale, path, title, ogLocale, canonicalLocale) => {
      const description = `${locale} metadata contract`;
      const metadata = buildPageMetadata({ path, locale, title, description });
      const suffix = path === "/" ? "" : path;
      const canonical = `${SITE_URL}/${canonicalLocale}${suffix}`;

      expect(metadata.alternates).toEqual({
        canonical,
        languages: expectedLanguages(path),
      });
      expect(metadata.openGraph).toEqual({
        title,
        description,
        url: canonical,
        siteName: SITE_NAME,
        type: "website",
        locale: ogLocale,
        images: [
          {
            url: `${SITE_URL}/opengraph-image`,
            width: 1200,
            height: 630,
            alt: OG_ALT,
          },
        ],
      });
      expect(metadata.twitter).toEqual({
        card: "summary_large_image",
        title,
        description,
        // The card must carry its own alt text: Twitter reads
        // `twitter:image:alt` and does not inherit `og:image:alt` once a
        // `twitter:image` tag is present.
        images: [{ url: `${SITE_URL}/opengraph-image`, alt: OG_ALT }],
      });
    },
  );

  it("advertises only genuine page-body translations", () => {
    for (const [path, locale, expectedLocales, expectedCanonical] of [
      ["/", "ja", locales, `${SITE_URL}/ja`],
      [
        "/docs/guide",
        "fr",
        ["en", "zh", "fr", "de", "ca", "hi", "tr", "it", "pl", "ar"],
        `${SITE_URL}/fr/docs/guide`,
      ],
      [
        "/docs/guide",
        "ja",
        contentLocalesForPath("/docs/guide"),
        `${SITE_URL}/en/docs/guide`,
      ],
      ["/docs", "ja", ["en", "zh"], `${SITE_URL}/en/docs`],
    ] as const) {
      const metadata = buildPageMetadata({
        path,
        locale,
        title: "Metadata contract",
        description: "Truthful locale coverage",
      });
      const languages = metadata.alternates?.languages as Record<string, string>;
      expect(Object.keys(languages)).toEqual([...expectedLocales, "x-default"]);
      expect(metadata.alternates?.canonical).toBe(expectedCanonical);
    }
  });

  it("keeps the canonical brand-first identity without duplicating an OG heading", () => {
    const brand = new RegExp(`\\b${SITE_NAME}\\b`, "gi");
    const ogImage = readFileSync(new URL("../app/opengraph-image.tsx", import.meta.url), "utf8");

    expect(IDENTITY_PHRASE).toBe("Codewhale dives into the deep so you don't have to.");
    expect(OG_ALT).toBe(IDENTITY_PHRASE);
    expect(OG_ALT.match(brand)).toHaveLength(1);
    expect(ogImage).toContain("{IDENTITY_PHRASE}");
    expect(ogImage).not.toContain("{SITE_NAME}");
  });

  it("keeps the previously incomplete indexable routes on the shared helper", () => {
    for (const [route, path] of [
      ["faq", "/faq"],
      ["feed", "/feed"],
      ["roadmap", "/roadmap"],
      ["pricing", "/pricing"],
      ["legal/terms", "/legal/terms"],
      ["legal/privacy", "/legal/privacy"],
    ]) {
      const source = readFileSync(
        new URL(`../app/[locale]/${route}/page.tsx`, import.meta.url),
        "utf8",
      );
      expect(source, route).toContain('import { buildPageMetadata } from "@/lib/page-meta"');
      expect(source, route).toContain("return buildPageMetadata({");
      expect(source, route).toContain(`path: "${path}"`);
    }
  });
});
