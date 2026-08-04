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
import type { ChromeDict, HomeDict } from "./types";
import { chrome as enChrome } from "./en/chrome";
import { home as enHome } from "./en/home";
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
  "pt-BR": ptBrHome,
  id: idHome,
};

/** Locales with their own dictionary directory (English is the reference). */
export const DICTIONARY_LOCALES = Object.keys(CHROME) as readonly string[];

export function getChrome(locale: string): ChromeDict {
  return CHROME[locale] ?? enChrome;
}

export function getHome(locale: string): HomeDict {
  return HOME[locale] ?? enHome;
}

/** Reference dictionaries (parity baseline for the locale checks). */
export const EN_CHROME = enChrome;
export const EN_HOME = enHome;

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
