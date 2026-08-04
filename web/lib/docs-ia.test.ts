/**
 * Information-architecture contracts: docs-map registration, sitemap and
 * hreflang preservation, navigation parity across breakpoints and locales,
 * and the accessibility hooks (skip link, labelled nav, aria-current).
 *
 * These are deterministic source/unit contracts in the same style as
 * public-copy.test.ts: they read the real sources and assert structure, so a
 * future IA change fails here first instead of drifting silently.
 */
import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { DOC_TOPICS, docTopicHref, getTopic } from "./docs-map";
import { docsTopicIsCurrent } from "./docs-navigation";
import { locales } from "./i18n/config";
import { getChrome, getHome } from "./i18n/dictionaries";
import {
  footerProductLinks,
  footerProjectLinks,
  navLinks as buildNavLinks,
} from "./i18n/links";

const webRoot = new URL("../", import.meta.url);
const repoRoot = new URL("../../", import.meta.url);

function webText(path: string): string {
  return readFileSync(new URL(path, webRoot), "utf8");
}

const sitemap = webText("app/sitemap.ts");
const nav = webText("components/nav.tsx");
const navLinks = webText("components/nav-links.tsx");
const mobileMenu = webText("components/mobile-menu.tsx");
const footer = webText("components/footer.tsx");
const localeLayout = webText("app/[locale]/layout.tsx");
const css = webText("app/globals.css");

describe("docs-map registration", () => {
  it("registers the guide and vocabulary topics as first-party pages", () => {
    const guide = getTopic("guide");
    const vocabulary = getTopic("vocabulary");
    expect(guide?.hasPage).toBe(true);
    expect(vocabulary?.hasPage).toBe(true);
    expect(vocabulary?.category).toBe("core-concepts");
    expect(docTopicHref(guide!, "en")).toBe("/en/docs/guide");
    expect(docTopicHref(vocabulary!, "zh")).toBe("/zh/docs/vocabulary");
    expect(docsTopicIsCurrent(vocabulary!, "en", "/en/docs/vocabulary")).toBe(true);
  });

  it("keeps every docs topic repo source on disk", () => {
    for (const topic of DOC_TOPICS) {
      const sources = Array.isArray(topic.repoSource) ? topic.repoSource : [topic.repoSource];
      for (const source of sources) {
        expect(existsSync(new URL(source, repoRoot)), `${topic.id}: ${source}`).toBe(true);
      }
    }
  });

  it("keeps topic labels and descriptions bilingual", () => {
    for (const topic of DOC_TOPICS) {
      for (const pair of [topic.label, topic.description]) {
        expect(pair.en.trim().length, `${topic.id} en`).toBeGreaterThan(0);
        expect(pair.zh.trim().length, `${topic.id} zh`).toBeGreaterThan(0);
      }
    }
  });
});

describe("sitemap and hreflang preservation", () => {
  it("indexes every first-party docs page", () => {
    for (const topic of DOC_TOPICS) {
      if (!topic.hasPage) continue;
      const path = topic.sitePath ? `/${topic.sitePath}` : `/docs/${topic.slug}`;
      expect(sitemap, path).toContain(`"${path}"`);
    }
    expect(sitemap).toContain('"/docs/guide"');
    expect(sitemap).toContain('"/docs/vocabulary"');
  });

  it("keeps per-locale alternate pairs for every indexed route", () => {
    expect(sitemap).toContain("alternates");
    // Both the routes and their hreflang alternates are generated from the
    // canonical locale registry, never hardcoded per locale — asserting the
    // literal `en:` / `zh:` pairs would forbid exactly that generalization.
    expect(sitemap).toContain("locales.map");
    expect(sitemap).toContain("locales.map((l) => [l, `${SITE_URL}/${l}${path}`])");
    expect(locales).toContain("en");
    expect(locales).toContain("zh");
  });

  it("keeps the new docs pages on the shared metadata helper", () => {
    for (const route of ["guide", "vocabulary"]) {
      const page = webText(`app/[locale]/docs/${route}/page.tsx`);
      expect(page, route).toContain('import { buildPageMetadata } from "@/lib/page-meta"');
      expect(page, route).toContain(`path: "/docs/${route}"`);
    }
  });
});

describe("navigation parity and accessibility", () => {
  it("keeps desktop and mobile navigation on one shared link set", () => {
    // Both surfaces consume the same `links` prop from nav.tsx — assert the
    // wiring rather than duplicating the arrays.
    expect(nav).toContain("<NavLinks links={links} primaryAria={chrome.navPrimaryAria} />");
    expect(nav).toContain("links={links}");
    expect(mobileMenu).toContain("links.map");
    expect(navLinks).toContain("links.map");
    // One generator feeds both surfaces — no per-locale hardcoded arrays.
    expect(nav).toContain("navLinks(locale, chrome)");
    expect(nav).not.toMatch(/const (EN|ZH)_LINKS/);
  });

  it("keeps nav link paths in exact locale-swap parity for every routed locale", () => {
    // The hardcoded /en/ and /zh/ arrays are gone; assert the generated set
    // directly, across every routed locale rather than only two of them.
    const reference = buildNavLinks("en", getChrome("en")).map((l) =>
      l.href.replace(/^\/en\//, ""),
    );
    expect(reference.length).toBeGreaterThanOrEqual(4);
    expect(reference).toContain("docs/guide");
    expect(reference).toContain("faq");
    for (const locale of locales) {
      const links = buildNavLinks(locale, getChrome(locale));
      expect(
        links.map((l) => l.href.replace(new RegExp(`^/${locale}/`), "")),
        `${locale} nav routes`,
      ).toEqual(reference);
      for (const link of links) {
        expect(link.href.startsWith(`/${locale}/`), `${locale} ${link.href}`).toBe(true);
        expect(link.label.trim().length, `${locale} empty nav label`).toBeGreaterThan(0);
      }
    }
  });

  it("keeps footer link paths in exact locale-swap parity for every routed locale", () => {
    const reference = footerProductLinks("en", getChrome("en")).map((l) =>
      l.href.replace(/^\/en\//, ""),
    );
    expect(reference).toContain("docs/guide");
    expect(reference).toContain("faq");
    for (const locale of locales) {
      const product = footerProductLinks(locale, getChrome(locale));
      expect(
        product.map((l) => l.href.replace(new RegExp(`^/${locale}/`), "")),
        `${locale} footer product routes`,
      ).toEqual(reference);
      const project = footerProjectLinks(locale, getChrome(locale));
      expect(project.map((l) => l.href), `${locale} footer project routes`).toEqual([
        "https://github.com/Hmbown/CodeWhale",
        "https://github.com/Hmbown/CodeWhale/issues",
        `/${locale}/contribute`,
        "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE",
      ]);
    }
  });

  it("labels the primary nav and marks the current page accessibly", () => {
    expect(navLinks).toContain("aria-label={primaryAria}");
    expect(getChrome("en").navPrimaryAria).toBe("Primary");
    expect(getChrome("zh").navPrimaryAria).toBe("主导航");
    expect(navLinks).toContain('aria-current={isActive ? "page" : undefined}');
    expect(mobileMenu).toContain('aria-current={isActive ? "page" : undefined}');
    expect(mobileMenu).toContain('aria-expanded={open}');
    expect(mobileMenu).toContain('aria-controls="mobile-menu"');
    expect(mobileMenu).toContain('role="dialog"');
  });

  it("ships a keyboard-reachable skip link to the main landmark", () => {
    expect(localeLayout).toContain('href="#main-content"');
    expect(localeLayout).toContain('className="skip-link"');
    expect(localeLayout).toContain('<main id="main-content">');
    expect(css).toContain(".skip-link:focus-visible");
  });

  it("keeps responsive breakpoints for the getting-started steps", () => {
    // 4-up grid by default, 2-up at the tablet breakpoint, 1-up on phones —
    // the same responsive ladder as the existing workflow steps.
    expect(css).toMatch(/\.gs-steps\s*\{[^}]*repeat\(4, minmax\(0, 1fr\)\)/);
    expect(css).toMatch(
      /@media \(max-width: 760px\)[\s\S]*?\.gs-steps\s*\{[^}]*repeat\(2, minmax\(0, 1fr\)\)/,
    );
    expect(css).toMatch(
      /@media \(max-width: 520px\)[\s\S]*?\.gs-steps\s*\{\s*grid-template-columns: 1fr/,
    );
  });

  it("keeps the footer discovery links alongside the pinned legal links", () => {
    // The link sets moved to lib/i18n/links.ts, so assert the rendered
    // contract for en AND zh rather than scraping literals out of the TSX.
    for (const locale of ["en", "zh"]) {
      const product = footerProductLinks(locale, getChrome(locale)).map((l) => l.href);
      expect(product, `${locale} footer product`).toContain(`/${locale}/docs/guide`);
      expect(product, `${locale} footer product`).toContain(`/${locale}/faq`);
    }
    const license = footerProjectLinks("en", getChrome("en")).at(-1);
    expect(license).toEqual({
      label: "MIT license",
      href: "https://github.com/Hmbown/CodeWhale/blob/main/LICENSE",
    });
    expect(footer).toContain("footerProductLinks(locale, chrome)");
    expect(footer).toContain("footerProjectLinks(locale, chrome)");
  });
});

describe("homepage integration", () => {
  const homepage = webText("app/[locale]/page.tsx");

  it("renders the shared getting-started path on the homepage", () => {
    expect(homepage).toContain('import { GettingStartedSteps } from "@/components/getting-started-steps"');
    expect(homepage).toContain("<GettingStartedSteps locale={locale} />");
    expect(homepage).toContain("product-start");
    expect(homepage).toContain("/docs/guide");
    expect(homepage).toContain("/docs/vocabulary");
  });

  it("keeps the previously pinned homepage facts intact", () => {
    // Guard against the new band accidentally displacing the public-copy
    // gate's required surface (the full contract lives in public-copy.test.ts).
    expect(homepage).toContain("facts.latestPublishedRelease");
    // "Source candidate" is now the EN dictionary value the page renders.
    expect(homepage).toContain("d.sourceCandidate");
    expect(getHome("en").sourceCandidate).toBe("Source candidate");
    expect(homepage).toContain('src="/codewhale-tui.png"');
    for (const label of ["Plan", "Act", "Operate", "Ask", "Auto-Review", "Full Access"]) {
      expect(homepage).toContain(label);
    }
  });
});
