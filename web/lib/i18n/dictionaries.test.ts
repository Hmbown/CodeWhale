import { describe, expect, it } from "vitest";
import {
  DICTIONARY_LOCALES,
  EN_CHROME,
  EN_HOME,
  fill,
  getChrome,
  getHome,
  splitToken,
} from "./dictionaries";
import { locales, partialLocales } from "./config";
import type { ChromeDict, HomeDict } from "./dictionaries/types";

/**
 * Keys whose value is a mark, a proper noun, or a formatting tag rather
 * than prose — a locale sharing English's value here is correct, not a
 * missing translation.
 */
const NON_PROSE_KEYS = new Set([
  "wordmarkSeal",
  "dateLocale",
  "githubFallback",
  "tickerLiveTag",
  "sealDecides",
  "sealWorkflow",
  "sealStart",
  "sealBoundaries",
  "sealSurfaces",
  "sealCommunity",
]);

/** Chrome keys that are real sentences/labels and must be translated. */
const CHROME_PROSE_KEYS = [
  "skipToContent",
  "navDocs",
  "navCommunity",
  "navPrimaryAria",
  "navHomeAria",
  "wordmarkTag",
  "starsAria",
  "traceLabel",
  "traceTabsAria",
  "menuOpen",
  "menuClose",
  "themeAria",
  "themeTitle",
  "footerTagline",
  "footerProduct",
  "footerProject",
  "footerGuide",
  "footerCanonicalSource",
  "footerReleasesLink",
  "switcherLabel",
  "switcherSwitchTo",
  "partialBadge",
] as const satisfies readonly (keyof ChromeDict)[];

/** Home keys that are real sentences and must be translated. */
const HOME_PROSE_KEYS = [
  "metaTitle",
  "metaDescription",
  "kicker",
  "heroTitleA",
  "heroTitleB",
  "heroIntro",
  "installEyebrow",
  "installRequirement",
  "installOtherWays",
  "shotSession",
  "screenshotAlt",
  "figcaption",
  "proofHeading",
  "proofBody",
  "decidesEyebrow",
  "decidesHeading",
  "decidesLede",
  "workflowHeading",
  "receiptAria",
  "receiptInspect",
  "receiptAct",
  "receiptReport",
  "startHeading",
  "startLede",
  "startGuideLink",
  "startVocabularyLink",
  "boundariesBody",
  "hostedGatewayLocal",
  "surfacesHeading",
  "runtimeLink",
  "installBandHeading",
  "installGuideLink",
  "communityHeading",
  "communityBody",
  "communityLinksAria",
] as const satisfies readonly (keyof HomeDict)[];

function templateTokens(value: string): string[] {
  return [...value.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
}

function flattenStrings(dict: object): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(dict)) {
    if (typeof value === "string") {
      out[key] = value;
    } else if (Array.isArray(value)) {
      value.forEach((pair, i) => {
        out[`${key}[${i}][0]`] = pair[0];
        out[`${key}[${i}][1]`] = pair[1];
      });
    }
  }
  return out;
}

describe("website dictionaries", () => {
  it("cover every routed locale except the English reference", () => {
    expect([...DICTIONARY_LOCALES].sort()).toEqual(
      ["zh", "es", "id", "ja", "ko", "pt-BR", "ru", "uk", "vi"].sort(),
    );
    // Chinese is dictionary-backed like every other locale — no inline
    // en/zh special case survives in the page/component sources (#4934).
    expect(DICTIONARY_LOCALES).toContain("zh");
    // Every routed locale either has its own dictionary or *is* English.
    for (const locale of locales) {
      expect(
        locale === "en" || DICTIONARY_LOCALES.includes(locale),
        `${locale} has no dictionary`,
      ).toBe(true);
    }
    // Every partial locale is dictionary-backed, so the partial badge marks
    // untranslated page bodies — never untranslated chrome.
    for (const locale of partialLocales) {
      expect(DICTIONARY_LOCALES, `${locale} partial pack`).toContain(locale);
    }
  });

  it("holds every dictionary to exact key parity with the English reference", () => {
    const enChromeKeys = Object.keys(EN_CHROME).sort();
    const enHomeKeys = Object.keys(EN_HOME).sort();
    for (const locale of DICTIONARY_LOCALES) {
      expect(Object.keys(getChrome(locale)).sort(), `${locale} chrome keys`).toEqual(
        enChromeKeys,
      );
      expect(Object.keys(getHome(locale)).sort(), `${locale} home keys`).toEqual(
        enHomeKeys,
      );
    }
  });

  it("preserves {token} template placeholders through translation", () => {
    const enChromeTokens = flattenStrings(EN_CHROME);
    const enHomeTokens = flattenStrings(EN_HOME);
    for (const locale of DICTIONARY_LOCALES) {
      const chrome = flattenStrings(getChrome(locale));
      const home = flattenStrings(getHome(locale));
      for (const key of Object.keys(enChromeTokens)) {
        expect(templateTokens(chrome[key]), `${locale} chrome ${key}`).toEqual(
          templateTokens(enChromeTokens[key]),
        );
      }
      for (const key of Object.keys(enHomeTokens)) {
        expect(templateTokens(home[key]), `${locale} home ${key}`).toEqual(
          templateTokens(enHomeTokens[key]),
        );
      }
    }
  });

  it("keeps workflow and surface lists structurally aligned", () => {
    for (const locale of DICTIONARY_LOCALES) {
      const home = getHome(locale);
      expect(home.workflow, `${locale} workflow`).toHaveLength(4);
      expect(home.surfaces, `${locale} surfaces`).toHaveLength(5);
      for (const pair of [...home.workflow, ...home.surfaces]) {
        expect(pair[0].length, `${locale} empty title`).toBeGreaterThan(0);
        expect(pair[1].length, `${locale} empty description`).toBeGreaterThan(0);
      }
    }
  });

  it("falls back to the English dictionary for unrouted locales — no missing markers", () => {
    for (const key of Object.keys(EN_CHROME) as (keyof ChromeDict)[]) {
      expect(getChrome("fr")[key]).toBe(EN_CHROME[key]);
      expect(getChrome("en")[key]).toBe(EN_CHROME[key]);
    }
    for (const key of Object.keys(EN_HOME) as (keyof HomeDict)[]) {
      expect(getHome("de")[key]).toEqual(EN_HOME[key]);
    }
  });

  it("has no empty strings anywhere", () => {
    for (const locale of ["en", ...DICTIONARY_LOCALES]) {
      for (const [key, value] of Object.entries(flattenStrings(getChrome(locale)))) {
        expect(value.trim().length, `${locale} chrome ${key}`).toBeGreaterThan(0);
      }
      for (const [key, value] of Object.entries(flattenStrings(getHome(locale)))) {
        expect(value.trim().length, `${locale} home ${key}`).toBeGreaterThan(0);
      }
    }
  });

  it("keeps the Cyrillic packs script-pure (no cross-leakage, no mixed copy)", () => {
    const cyrillic = /[Ѐ-ӿ]/;
    for (const [key, value] of Object.entries(flattenStrings(getChrome("uk")))) {
      expect(value, `uk chrome ${key}`).not.toMatch(/[ыэъЫЭЪ]/);
      void cyrillic;
    }
    for (const [key, value] of Object.entries(flattenStrings(getHome("uk")))) {
      expect(value, `uk home ${key}`).not.toMatch(/[ыэъЫЭЪ]/);
    }
    for (const [key, value] of Object.entries(flattenStrings(getChrome("ru")))) {
      expect(value, `ru chrome ${key}`).not.toMatch(/[іІїЇєЄґҐ]/);
    }
    for (const [key, value] of Object.entries(flattenStrings(getHome("ru")))) {
      expect(value, `ru home ${key}`).not.toMatch(/[іІїЇєЄґҐ]/);
    }
    // Prose values are actually translated, not English pass-through.
    expect(getHome("ru").heroIntro).toMatch(cyrillic);
    expect(getHome("uk").heroIntro).toMatch(cyrillic);
    expect(getChrome("ru").navDocs).not.toBe(EN_CHROME.navDocs);
    expect(getChrome("uk").navDocs).not.toBe(EN_CHROME.navDocs);
    expect(getChrome("ru").navDocs).not.toBe(getChrome("uk").navDocs);
  });

  it("keeps the Chinese pack in Han script for prose (no English pass-through)", () => {
    const han = /[一-鿿]/;
    const chrome = getChrome("zh");
    const home = getHome("zh");
    for (const key of CHROME_PROSE_KEYS) {
      expect(chrome[key], `zh chrome ${key}`).toMatch(han);
    }
    for (const key of HOME_PROSE_KEYS) {
      expect(home[key], `zh home ${key}`).toMatch(han);
    }
    // Chinese resolves to its OWN dictionary, not the English reference.
    expect(chrome.navDocs).not.toBe(EN_CHROME.navDocs);
    expect(home.heroTitleA).not.toBe(EN_HOME.heroTitleA);
  });

  it("leaves no unmarked English prose in any non-English dictionary", () => {
    for (const locale of DICTIONARY_LOCALES) {
      const chrome = getChrome(locale);
      const home = getHome(locale);
      for (const key of CHROME_PROSE_KEYS) {
        expect(chrome[key], `${locale} chrome ${key} is English pass-through`).not.toBe(
          EN_CHROME[key],
        );
      }
      for (const key of HOME_PROSE_KEYS) {
        expect(home[key], `${locale} home ${key} is English pass-through`).not.toBe(
          EN_HOME[key],
        );
      }
    }
  });

  it("keeps marks, tags, and proper nouns out of the translated-prose rule", () => {
    // Documents the deliberate exceptions so a future audit does not read a
    // shared value here as a missing translation.
    for (const key of NON_PROSE_KEYS) {
      const inChrome = key in EN_CHROME;
      const inHome = key in EN_HOME;
      expect(inChrome || inHome, `${key} is not a real dictionary key`).toBe(true);
      expect(CHROME_PROSE_KEYS as readonly string[]).not.toContain(key);
      expect(HOME_PROSE_KEYS as readonly string[]).not.toContain(key);
    }
  });

  it("carries the {brand} token through every hero lede for splitToken()", () => {
    for (const locale of ["en", ...DICTIONARY_LOCALES]) {
      const lede = getHome(locale).heroIntro;
      expect(lede, `${locale} heroIntro`).toContain("{brand}");
      const parts = splitToken(lede, "brand");
      expect(parts.length, `${locale} heroIntro brand split`).toBe(2);
      expect(parts.join("").includes("{brand}")).toBe(false);
    }
  });

  it("interpolates templates with fill() and leaves unknown tokens visible", () => {
    expect(fill("Latest release {tag}", { tag: "v0.9.2" })).toBe("Latest release v0.9.2");
    expect(fill("{count} provider routes", { count: 30 })).toBe("30 provider routes");
    expect(fill("v{version} {state}", { version: "0.9.2" })).toBe("v0.9.2 {state}");
  });
});
