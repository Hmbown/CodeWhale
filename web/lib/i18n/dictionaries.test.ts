import { describe, expect, it } from "vitest";
import {
  DICTIONARY_LOCALES,
  EN_CHROME,
  EN_DOCS_GUIDE,
  EN_DOCS_CONSTITUTION,
  EN_DOCS_HOOKS,
  EN_DOCS_MCP,
  EN_DOCS_RUNTIME_API,
  EN_DOCS_SANDBOX,
  EN_DOCS_SUBAGENTS,
  EN_DOCS_WEB,
  EN_DOCS_SHELL,
  EN_DOCS_TROUBLESHOOTING,
  EN_HOME,
  fill,
  getChrome,
  getDocsGuide,
  getDocsConstitution,
  getDocsHooks,
  getDocsMcp,
  getDocsRuntimeApi,
  getDocsSandbox,
  getDocsSubagents,
  getDocsWeb,
  getDocsShell,
  getDocsTroubleshooting,
  getHome,
  pickText,
  splitToken,
  splitTokens,
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
  // Ticker chrome. The repository's own record (titles, handles, tags) stays
  // verbatim, but the verbs the strip prints around it are copy.
  "tickerMerged",
  "tickerOpened",
  "tickerClosed",
  "tickerReleased",
  "tickerFirstContribution",
  "tickerBy",
  "tickerAria",
] as const satisfies readonly (keyof ChromeDict)[];

/**
 * Locale/key pairs whose English-identical value is a native loanword in
 * that locale, not a missing translation — asserted below so the equality
 * is deliberate and visible. German "Community" matches the TUI pack
 * (crates/tui/locales/de.json: "Community & Mitwirken").
 */
const CHROME_LOANWORDS: Record<string, readonly string[]> = {
  de: ["navCommunity"],
};

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
      [
        "zh", "es", "id", "ja", "ko", "pt-BR", "ru", "uk", "vi",
        "fr", "de", "ca", "hi", "tr", "it", "pl", "ar",
      ].sort(),
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

  it("holds every shipped page dictionary to key parity and English fallback (#5337)", () => {
    const enKeys = Object.keys(EN_DOCS_GUIDE).sort();
    for (const locale of [...DICTIONARY_LOCALES, "fr", "und"]) {
      // Page dictionaries are optional per locale: whatever getDocsGuide
      // resolves — the locale's own file or the English fallback — must
      // carry the exact reference shape, so a page never sees a missing key.
      expect(Object.keys(getDocsGuide(locale)).sort(), `${locale} docs-guide keys`).toEqual(
        enKeys,
      );
    }
    // zh ships a real translation, not an English pass-through.
    expect(getDocsGuide("zh").overviewTitle).not.toBe(EN_DOCS_GUIDE.overviewTitle);
    // The wave-2 locales ship docs-guide too, translated rather than passed through.
    for (const locale of ["fr", "de", "ca", "hi", "tr", "it", "pl", "ar"]) {
      expect(getDocsGuide(locale).overviewTitle, `${locale} docs-guide`).not.toBe(
        EN_DOCS_GUIDE.overviewTitle,
      );
    }
    // A locale without the file falls back to the English reference object.
    expect(getDocsGuide("ja")).toBe(EN_DOCS_GUIDE);
  });

  it("holds the docs shell dictionary to the same contract (#5337)", () => {
    const enKeys = Object.keys(EN_DOCS_SHELL).sort();
    for (const locale of [...DICTIONARY_LOCALES, "fr", "und"]) {
      expect(Object.keys(getDocsShell(locale)).sort(), `${locale} docs-shell keys`).toEqual(
        enKeys,
      );
    }
    // zh ships a real translation, not an English pass-through.
    expect(getDocsShell("zh").heroTitle).not.toBe(EN_DOCS_SHELL.heroTitle);
    // Every other locale renders the English shell today, exactly as the
    // `isZh` ternaries in docs/layout.tsx did before the move.
    for (const locale of ["ja", "fr", "ar", "und"]) {
      expect(getDocsShell(locale), `${locale} docs-shell`).toBe(EN_DOCS_SHELL);
    }
  });

  it("holds the docs page-body dictionaries to the same contract (#5337)", () => {
    for (const [label, get, reference] of [
      ["docs-hooks", getDocsHooks, EN_DOCS_HOOKS],
      ["docs-troubleshooting", getDocsTroubleshooting, EN_DOCS_TROUBLESHOOTING],
      ["docs-constitution", getDocsConstitution, EN_DOCS_CONSTITUTION],
      ["docs-runtime-api", getDocsRuntimeApi, EN_DOCS_RUNTIME_API],
      ["docs-sandbox", getDocsSandbox, EN_DOCS_SANDBOX],
      ["docs-subagents", getDocsSubagents, EN_DOCS_SUBAGENTS],
      ["docs-mcp", getDocsMcp, EN_DOCS_MCP],
      ["docs-web", getDocsWeb, EN_DOCS_WEB],
    ] as const) {
      const enKeys = Object.keys(reference).sort();
      for (const locale of [...DICTIONARY_LOCALES, "fr", "und"]) {
        expect(Object.keys(get(locale)).sort(), `${locale} ${label} keys`).toEqual(enKeys);
      }
      // zh ships a real translation, not an English pass-through. The probe is
      // `metaTitle` rather than `overviewTitle` because docs/mcp's heading is
      // the code-owned literal `MCP` and stays in the page.
      expect(get("zh").metaTitle, `zh ${label}`).not.toBe(reference.metaTitle);
      // Every other locale renders English today, exactly as the `isZh`
      // ternaries in the page did before the move.
      for (const locale of ["ja", "fr", "ar", "und"]) {
        expect(get(locale), `${locale} ${label}`).toBe(reference);
      }
    }
  });

  it("keeps the docs page lists structurally aligned", () => {
    for (const locale of [...DICTIONARY_LOCALES, "und"]) {
      expect(getDocsHooks(locale).events, `${locale} hook events`).toHaveLength(4);
      expect(
        getDocsTroubleshooting(locale).incidents,
        `${locale} triage entries`,
      ).toHaveLength(5);
      expect(
        getDocsConstitution(locale).principles.map(([key]) => key),
        `${locale} constitution principles`,
      ).toEqual(["userGlobal", "repoLocal", "runtime"]);
      expect(
        getDocsRuntimeApi(locale).entries.map(([key]) => key),
        `${locale} runtime entries`,
      ).toEqual(["http", "mobile", "stdio", "web", "doctor", "acp", "exec"]);
      // The platform rows are keyed by their own translated name rather than a
      // code-owned key, so only the count is comparable across locales.
      expect(getDocsSandbox(locale).platforms, `${locale} sandbox platforms`).toHaveLength(4);
      // Role names are identifiers the page owns, so the keys are comparable
      // across locales rather than only the count.
      expect(
        getDocsSubagents(locale).roles.map(([key]) => key),
        `${locale} subagent roles`,
      ).toEqual([
        "worker",
        "scout",
        "planner",
        "reviewer",
        "builder",
        "verifier",
        "consultant",
        "custom",
      ]);
    }
  });

  it("carries every code-span token through the hooks intro for splitTokens()", () => {
    for (const locale of [...DICTIONARY_LOCALES, "und"]) {
      const parts = splitTokens(getDocsHooks(locale).configIntro);
      const tokens = parts.flatMap((part) => ("token" in part ? [part.token] : []));
      expect(tokens, `${locale} configIntro tokens`).toEqual([
        "hooksTable",
        "hooksCommand",
        "enabledKey",
      ]);
    }
  });

  it("carries every code-span token through the constitution and runtime-api copy", () => {
    const tokensOf = (template: string) =>
      splitTokens(template).flatMap((part) => ("token" in part ? [part.token] : []));
    for (const locale of [...DICTIONARY_LOCALES, "und"]) {
      const constitution = getDocsConstitution(locale);
      expect(tokensOf(constitution.overviewLead), `${locale} overviewLead`).toEqual([
        "constitutionCommand",
        "homeConfig",
        "repoConfig",
      ]);
      // Exactly one link slot, so the translated label is never concatenated
      // onto a fragment the call site owns.
      expect(tokensOf(constitution.authorityNote), `${locale} authorityNote`).toEqual([
        "configDocs",
      ]);
      expect(tokensOf(getDocsRuntimeApi(locale).securityLead), `${locale} securityLead`).toEqual([
        "authToken",
        "runtimeTokenEnv",
        "legacyTokenEnv",
        "insecureFlag",
        "mobileFlag",
      ]);
    }
  });

  it("carries every code-span token through the subagents and mcp copy", () => {
    const tokensOf = (template: string) =>
      splitTokens(template).flatMap((part) => ("token" in part ? [part.token] : []));
    for (const locale of [...DICTIONARY_LOCALES, "und"]) {
      const subagents = getDocsSubagents(locale);
      expect(tokensOf(subagents.forkLead), `${locale} forkLead`).toEqual([
        "agentTool",
        "forkContext",
      ]);
      expect(tokensOf(subagents.worktreeLead), `${locale} worktreeLead`).toEqual([
        "worktreeFlag",
        "branchPattern",
        "worktreeDir",
        "writeAuthority",
        "writeRoots",
        "exactFiles",
        "coordinationContracts",
      ]);
      const mcp = getDocsMcp(locale);
      expect(tokensOf(mcp.overviewConfig), `${locale} mcp overviewConfig`).toEqual([
        "configPath",
        "legacyConfigPath",
        "configPathOption",
        "configEnvVar",
        "serversKey",
      ]);
      expect(tokensOf(mcp.setupLead), `${locale} mcp setupLead`).toEqual([
        "initCommand",
        "mcpCommand",
      ]);
      expect(tokensOf(mcp.toolsLead), `${locale} mcp toolsLead`).toEqual([
        "toolNamePattern",
        "gitServer",
        "statusTool",
        "gitStatusTool",
      ]);
      expect(tokensOf(mcp.serverLead), `${locale} mcp serverLead`).toEqual([
        "serveMcp",
        "mcpServerCommand",
        "addSelfCommand",
        "serveHttp",
      ]);
    }
  });

  it("carries every code-span token through the sandbox and web copy", () => {
    const tokensOf = (template: string) =>
      splitTokens(template).flatMap((part) => ("token" in part ? [part.token] : []));
    for (const locale of [...DICTIONARY_LOCALES, "und"]) {
      // Two of these repeat, and the order is the sentence's, so this is a
      // stricter check than check-locales.mjs, which compares token sets.
      expect(tokensOf(getDocsSandbox(locale).policiesLead), `${locale} policiesLead`).toEqual([
        "sandboxMode",
        "readOnly",
        "workspaceWrite",
        "dangerFullAccess",
        "externalSandbox",
        "dangerFullAccess",
        "externalSandbox",
      ]);
      const web = getDocsWeb(locale);
      expect(tokensOf(web.overviewLead), `${locale} web overviewLead`).toEqual([
        "webCommand",
        "loopbackHost",
        "defaultUrl",
        "portExample",
      ]);
      expect(tokensOf(web.localLead), `${locale} localLead`).toEqual([
        "webCommand",
        "portFlag",
        "hostFlag",
        "mobileCommand",
        "httpFlag",
      ]);
    }
  });

  it("splitTokens interleaves literal text and token names in template order", () => {
    expect(splitTokens("a {one} b {two}")).toEqual([
      { text: "a " },
      { token: "one" },
      { text: " b " },
      { token: "two" },
    ]);
    // A template with no token is one literal run, and an empty run between
    // adjacent tokens is dropped rather than rendered as an empty node.
    expect(splitTokens("plain")).toEqual([{ text: "plain" }]);
    expect(splitTokens("{one}{two}")).toEqual([{ token: "one" }, { token: "two" }]);
  });

  it("pickText selects the locale side of legacy { en, zh } pairs", () => {
    const pair = { en: "English", zh: "中文" };
    expect(pickText(pair, "zh")).toBe("中文");
    expect(pickText(pair, "en")).toBe("English");
    expect(pickText(pair, "ja"), "non-zh locales read the English side").toBe("English");
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
      expect(getChrome("xx")[key]).toBe(EN_CHROME[key]);
      expect(getChrome("en")[key]).toBe(EN_CHROME[key]);
    }
    for (const key of Object.keys(EN_HOME) as (keyof HomeDict)[]) {
      expect(getHome("xx")[key]).toEqual(EN_HOME[key]);
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
      const loanwords = new Set(CHROME_LOANWORDS[locale] ?? []);
      for (const key of CHROME_PROSE_KEYS) {
        if (loanwords.has(key)) {
          // A documented loanword: the shared value IS the native word.
          expect(chrome[key], `${locale} chrome ${key} loanword`).toBe(EN_CHROME[key]);
          continue;
        }
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

  it("carries the {handle} token through every ticker by-line", () => {
    // components/ticker.tsx splits on the token so the handle is typeset in
    // its own element. A locale that drops it would print a by-line with no
    // contributor in it — the opposite of the point.
    for (const locale of ["en", ...DICTIONARY_LOCALES]) {
      const byLine = getChrome(locale).tickerBy;
      expect(byLine, `${locale} tickerBy`).toContain("{handle}");
      const parts = splitToken(byLine, "handle");
      expect(parts.length, `${locale} tickerBy split`).toBe(2);
    }
  });

  it("interpolates templates with fill() and leaves unknown tokens visible", () => {
    expect(fill("Latest release {tag}", { tag: "v0.9.2" })).toBe("Latest release v0.9.2");
    expect(fill("{count} provider routes", { count: 30 })).toBe("30 provider routes");
    expect(fill("v{version} {state}", { version: "0.9.2" })).toBe("v0.9.2 {state}");
  });
});
