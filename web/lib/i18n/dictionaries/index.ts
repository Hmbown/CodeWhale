/**
 * Dictionary loader for the website localization layer (#3091, #4934).
 *
 * Lookup is deterministic: a routed locale with a dictionary gets its own
 * copy; every other locale gets the English reference dictionary. There is
 * no per-key fallback chain — each shipped/partial dictionary is held to
 * exact key parity with English by `web/scripts/check-locales.mjs` and
 * `dictionaries.test.ts`, so a missing key is a build-time failure, never
 * a runtime "missing marker".
 *
 * English resolves through the `?? enChrome` / `?? enHome` fallback rather
 * than a map entry, which keeps `DICTIONARY_LOCALES` equal to the set of
 * non-reference locale directories that `check-locales.mjs` walks.
 */
import type {
  ChromeDict,
  DocsConfigurationDict,
  DocsConstitutionDict,
  DocsFleetDict,
  DocsGuideDict,
  DocsHooksDict,
  DocsMcpDict,
  DocsRuntimeApiDict,
  DocsSandboxDict,
  DocsShellDict,
  DocsModesDict,
  DocsSubagentsDict,
  DocsTroubleshootingDict,
  DocsWebDict,
  HomeDict,
} from "./types";
import { chrome as enChrome } from "./en/chrome";
import { home as enHome } from "./en/home";
import { docsGuide as enDocsGuide } from "./en/docs-guide";
import { docsGuide as zhDocsGuide } from "./zh/docs-guide";
import { docsShell as enDocsShell } from "./en/docs-shell";
import { docsShell as zhDocsShell } from "./zh/docs-shell";
import { docsHooks as enDocsHooks } from "./en/docs-hooks";
import { docsHooks as zhDocsHooks } from "./zh/docs-hooks";
import { docsTroubleshooting as enDocsTroubleshooting } from "./en/docs-troubleshooting";
import { docsTroubleshooting as zhDocsTroubleshooting } from "./zh/docs-troubleshooting";
import { docsConfiguration as enDocsConfiguration } from "./en/docs-configuration";
import { docsConfiguration as zhDocsConfiguration } from "./zh/docs-configuration";
import { docsConstitution as enDocsConstitution } from "./en/docs-constitution";
import { docsConstitution as zhDocsConstitution } from "./zh/docs-constitution";
import { docsFleet as enDocsFleet } from "./en/docs-fleet";
import { docsFleet as zhDocsFleet } from "./zh/docs-fleet";
import { docsMcp as enDocsMcp } from "./en/docs-mcp";
import { docsMcp as zhDocsMcp } from "./zh/docs-mcp";
import { docsModes as enDocsModes } from "./en/docs-modes";
import { docsModes as zhDocsModes } from "./zh/docs-modes";
import { docsRuntimeApi as enDocsRuntimeApi } from "./en/docs-runtime-api";
import { docsRuntimeApi as zhDocsRuntimeApi } from "./zh/docs-runtime-api";
import { docsSandbox as enDocsSandbox } from "./en/docs-sandbox";
import { docsSandbox as zhDocsSandbox } from "./zh/docs-sandbox";
import { docsSubagents as enDocsSubagents } from "./en/docs-subagents";
import { docsSubagents as zhDocsSubagents } from "./zh/docs-subagents";
import { docsWeb as enDocsWeb } from "./en/docs-web";
import { docsWeb as zhDocsWeb } from "./zh/docs-web";
import { chrome as zhChrome } from "./zh/chrome";
import { home as zhHome } from "./zh/home";
import { chrome as jaChrome } from "./ja/chrome";
import { home as jaHome } from "./ja/home";
import { chrome as viChrome } from "./vi/chrome";
import { home as viHome } from "./vi/home";
import { chrome as koChrome } from "./ko/chrome";
import { home as koHome } from "./ko/home";
import { chrome as ruChrome } from "./ru/chrome";
import { home as ruHome } from "./ru/home";
import { chrome as ukChrome } from "./uk/chrome";
import { home as ukHome } from "./uk/home";
import { chrome as esChrome } from "./es/chrome";
import { home as esHome } from "./es/home";
import { chrome as frChrome } from "./fr/chrome";
import { home as frHome } from "./fr/home";
import { docsGuide as frDocsGuide } from "./fr/docs-guide";
import { chrome as deChrome } from "./de/chrome";
import { home as deHome } from "./de/home";
import { docsGuide as deDocsGuide } from "./de/docs-guide";
import { chrome as caChrome } from "./ca/chrome";
import { home as caHome } from "./ca/home";
import { docsGuide as caDocsGuide } from "./ca/docs-guide";
import { chrome as hiChrome } from "./hi/chrome";
import { home as hiHome } from "./hi/home";
import { docsGuide as hiDocsGuide } from "./hi/docs-guide";
import { chrome as trChrome } from "./tr/chrome";
import { home as trHome } from "./tr/home";
import { docsGuide as trDocsGuide } from "./tr/docs-guide";
import { chrome as itChrome } from "./it/chrome";
import { home as itHome } from "./it/home";
import { docsGuide as itDocsGuide } from "./it/docs-guide";
import { chrome as plChrome } from "./pl/chrome";
import { home as plHome } from "./pl/home";
import { docsGuide as plDocsGuide } from "./pl/docs-guide";
import { chrome as arChrome } from "./ar/chrome";
import { home as arHome } from "./ar/home";
import { docsGuide as arDocsGuide } from "./ar/docs-guide";
import { chrome as ptBrChrome } from "./pt-BR/chrome";
import { home as ptBrHome } from "./pt-BR/home";
import { chrome as idChrome } from "./id/chrome";
import { home as idHome } from "./id/home";

const CHROME: Record<string, ChromeDict> = {
  zh: zhChrome,
  ja: jaChrome,
  vi: viChrome,
  ko: koChrome,
  ru: ruChrome,
  uk: ukChrome,
  es: esChrome,
  fr: frChrome,
  de: deChrome,
  ca: caChrome,
  hi: hiChrome,
  tr: trChrome,
  it: itChrome,
  pl: plChrome,
  ar: arChrome,
  "pt-BR": ptBrChrome,
  id: idChrome,
};

const HOME: Record<string, HomeDict> = {
  zh: zhHome,
  ja: jaHome,
  vi: viHome,
  ko: koHome,
  ru: ruHome,
  uk: ukHome,
  es: esHome,
  fr: frHome,
  de: deHome,
  ca: caHome,
  hi: hiHome,
  tr: trHome,
  it: itHome,
  pl: plHome,
  ar: arHome,
  "pt-BR": ptBrHome,
  id: idHome,
};

/** Locales with their own dictionary directory (English is the reference). */
export const DICTIONARY_LOCALES = Object.keys(CHROME) as readonly string[];

/**
 * Per-page dictionaries (#5337). Unlike chrome/home, a page dictionary is
 * optional per locale: English is the required reference, any locale that
 * ships the file is held to exact key parity, and everyone else falls back
 * to English here — the same behavior page bodies already had for partial
 * locales, now expressed through one lookup instead of an `isZh` ternary.
 */
const DOCS_GUIDE: Record<string, DocsGuideDict> = {
  zh: zhDocsGuide,
  fr: frDocsGuide,
  de: deDocsGuide,
  ca: caDocsGuide,
  hi: hiDocsGuide,
  tr: trDocsGuide,
  it: itDocsGuide,
  pl: plDocsGuide,
  ar: arDocsGuide,
};

const DOCS_SHELL: Record<string, DocsShellDict> = {
  zh: zhDocsShell,
};

const DOCS_HOOKS: Record<string, DocsHooksDict> = {
  zh: zhDocsHooks,
};

const DOCS_TROUBLESHOOTING: Record<string, DocsTroubleshootingDict> = {
  zh: zhDocsTroubleshooting,
};

const DOCS_CONFIGURATION: Record<string, DocsConfigurationDict> = {
  zh: zhDocsConfiguration,
};

const DOCS_CONSTITUTION: Record<string, DocsConstitutionDict> = {
  zh: zhDocsConstitution,
};

const DOCS_FLEET: Record<string, DocsFleetDict> = {
  zh: zhDocsFleet,
};

const DOCS_MCP: Record<string, DocsMcpDict> = {
  zh: zhDocsMcp,
};

const DOCS_MODES: Record<string, DocsModesDict> = {
  zh: zhDocsModes,
};

const DOCS_RUNTIME_API: Record<string, DocsRuntimeApiDict> = {
  zh: zhDocsRuntimeApi,
};

const DOCS_SANDBOX: Record<string, DocsSandboxDict> = {
  zh: zhDocsSandbox,
};

const DOCS_SUBAGENTS: Record<string, DocsSubagentsDict> = {
  zh: zhDocsSubagents,
};

const DOCS_WEB: Record<string, DocsWebDict> = {
  zh: zhDocsWeb,
};

export function getChrome(locale: string): ChromeDict {
  return CHROME[locale] ?? enChrome;
}

export function getHome(locale: string): HomeDict {
  return HOME[locale] ?? enHome;
}

export function getDocsGuide(locale: string): DocsGuideDict {
  return DOCS_GUIDE[locale] ?? enDocsGuide;
}

export function getDocsShell(locale: string): DocsShellDict {
  return DOCS_SHELL[locale] ?? enDocsShell;
}

export function getDocsHooks(locale: string): DocsHooksDict {
  return DOCS_HOOKS[locale] ?? enDocsHooks;
}

export function getDocsTroubleshooting(locale: string): DocsTroubleshootingDict {
  return DOCS_TROUBLESHOOTING[locale] ?? enDocsTroubleshooting;
}

export function getDocsConfiguration(locale: string): DocsConfigurationDict {
  return DOCS_CONFIGURATION[locale] ?? enDocsConfiguration;
}

export function getDocsConstitution(locale: string): DocsConstitutionDict {
  return DOCS_CONSTITUTION[locale] ?? enDocsConstitution;
}

export function getDocsFleet(locale: string): DocsFleetDict {
  return DOCS_FLEET[locale] ?? enDocsFleet;
}

export function getDocsMcp(locale: string): DocsMcpDict {
  return DOCS_MCP[locale] ?? enDocsMcp;
}

export function getDocsModes(locale: string): DocsModesDict {
  return DOCS_MODES[locale] ?? enDocsModes;
}

export function getDocsRuntimeApi(locale: string): DocsRuntimeApiDict {
  return DOCS_RUNTIME_API[locale] ?? enDocsRuntimeApi;
}

export function getDocsSandbox(locale: string): DocsSandboxDict {
  return DOCS_SANDBOX[locale] ?? enDocsSandbox;
}

export function getDocsSubagents(locale: string): DocsSubagentsDict {
  return DOCS_SUBAGENTS[locale] ?? enDocsSubagents;
}

export function getDocsWeb(locale: string): DocsWebDict {
  return DOCS_WEB[locale] ?? enDocsWeb;
}

/**
 * Select one side of a legacy `{ en, zh }` content pair by locale. This is
 * the transitional bridge for `web/lib/content/` modules that still carry
 * two-language pairs (#5337 Phase 3 dissolves them into dictionaries): it
 * moves the branch out of page TSX and into the i18n layer, so call sites
 * stay locale-agnostic.
 */
export function pickText(pair: { en: string; zh: string }, locale: string): string {
  return locale === "zh" ? pair.zh : pair.en;
}

/** Reference dictionaries (parity baseline for the locale checks). */
export const EN_CHROME = enChrome;
export const EN_HOME = enHome;
export const EN_DOCS_GUIDE = enDocsGuide;
export const EN_DOCS_SHELL = enDocsShell;
export const EN_DOCS_HOOKS = enDocsHooks;
export const EN_DOCS_TROUBLESHOOTING = enDocsTroubleshooting;
export const EN_DOCS_CONFIGURATION = enDocsConfiguration;
export const EN_DOCS_CONSTITUTION = enDocsConstitution;
export const EN_DOCS_FLEET = enDocsFleet;
export const EN_DOCS_MCP = enDocsMcp;
export const EN_DOCS_MODES = enDocsModes;
export const EN_DOCS_RUNTIME_API = enDocsRuntimeApi;
export const EN_DOCS_SANDBOX = enDocsSandbox;
export const EN_DOCS_SUBAGENTS = enDocsSubagents;
export const EN_DOCS_WEB = enDocsWeb;

/** Interpolate `{name}` tokens in a dictionary template. Unknown tokens are
 * left intact so a template/variable drift is visible in review, not silent. */
export function fill(template: string, vars: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}

/**
 * Split a template on a single `{token}` so a call site can typeset the
 * substituted value as its own element without concatenating translated
 * fragments around it. Returns the literal parts in template order — the
 * caller interleaves its node between them, so a locale that puts the token
 * in a different position still renders correctly.
 */
export function splitToken(template: string, token: string): string[] {
  return template.split(`{${token}}`);
}

/**
 * Split a template on every `{token}` it carries, for a sentence with more
 * than one substituted node. Returns literal text and token names
 * interleaved in template order, so a locale that reorders the tokens still
 * renders correctly and no translated fragment is concatenated by the
 * call site.
 */
export function splitTokens(template: string): Array<{ text: string } | { token: string }> {
  return template
    .split(/\{(\w+)\}/g)
    .map((part, i) => (i % 2 === 1 ? { token: part } : { text: part }))
    .filter((part) => "token" in part || part.text !== "");
}
