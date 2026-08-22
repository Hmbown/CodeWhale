/**
 * Deterministic locale detection for the website middleware (#3091).
 *
 * Resolution order (first match wins, no ambient state):
 * 1. The NEXT_LOCALE cookie (a previous explicit choice).
 * 2. Accept-Language, in the header's stated preference order — descending
 *    `q` weight, original order breaking ties, and `q=0` ("not acceptable")
 *    dropped outright. Each tag matches:
 *    a. exact full tag against the routed set (pt-BR → pt-BR);
 *    b. its primary subtag against the routed set (ru-RU → ru, zh-Hant → zh);
 *    c. a declared base→variant mapping for bases we only serve as a
 *       regional variant (pt → pt-BR).
 * 3. The default locale (en).
 *
 * The mapping table is deliberately tiny and explicit — no guessing that
 * e.g. es-419 should route anywhere other than the shipped `es`.
 */
import { defaultLocale, locales } from "./config";

const ROUTED = locales as readonly string[];

/** Base subtags that route to a specific regional variant. */
const BASE_TO_VARIANT: Record<string, string> = {
  pt: "pt-BR",
};

/** Match one language tag (any case, optional region/script) to a routed locale. */
export function matchLocaleTag(tag: string): string | null {
  const t = tag.trim().toLowerCase();
  if (!t || t === "*") return null;

  // Exact full-tag match (case-insensitive; routed codes are lowercase).
  const exact = ROUTED.find((l) => l.toLowerCase() === t);
  if (exact) return exact;

  const base = t.split("-")[0];
  if (ROUTED.includes(base)) return base;

  const variant = BASE_TO_VARIANT[base];
  if (variant && ROUTED.includes(variant)) return variant;

  return null;
}

/**
 * Accept-Language tags in the client's stated preference order.
 *
 * The header carries weights, and its list order is not required to be the
 * preference order: `en;q=0.2, ja;q=0.9` asks for Japanese, and `q=0` means
 * "not acceptable" (RFC 9110 §12.4.2), not "least preferred". Reading the
 * list positionally handed the first of those a reader English and the
 * second one a language they had explicitly refused.
 *
 * Weights sort descending; original order breaks ties, so an unweighted
 * `ru,uk` still resolves to Russian. A malformed weight is ignored rather
 * than guessed at, leaving the tag at the default weight of 1.
 */
export function acceptLanguageTags(header: string): string[] {
  const entries: { tag: string; q: number; order: number }[] = [];

  header.split(",").forEach((part, order) => {
    const [rawTag, ...params] = part.split(";");
    const tag = rawTag.trim();
    if (!tag) return;

    let q = 1;
    for (const param of params) {
      const eq = param.indexOf("=");
      if (eq === -1) continue;
      if (param.slice(0, eq).trim().toLowerCase() !== "q") continue;
      const parsed = Number.parseFloat(param.slice(eq + 1).trim());
      if (Number.isFinite(parsed)) q = Math.min(Math.max(parsed, 0), 1);
    }

    if (q === 0) return;
    entries.push({ tag, q, order });
  });

  entries.sort((a, b) => b.q - a.q || a.order - b.order);
  return entries.map((entry) => entry.tag);
}

/** Resolve the locale for a request from its cookie and Accept-Language header. */
export function detectLocaleFromHeaders(
  cookie: string | undefined,
  acceptLanguage: string | null,
): string {
  if (cookie) {
    const match = matchLocaleTag(cookie);
    if (match) return match;
  }

  if (acceptLanguage) {
    for (const tag of acceptLanguageTags(acceptLanguage)) {
      const match = matchLocaleTag(tag);
      if (match) return match;
    }
  }

  return defaultLocale;
}
