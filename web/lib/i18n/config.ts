/**
 * Locale configuration for codewhale.net.
 *
 * This is the single canonical website locale registry — the locale
 * switcher, Next.js route generation, middleware detection, sitemap, and
 * hreflang alternates all derive from `ALL_LOCALES`. The cross-surface
 * matrix (TUI packs, READMEs, website) lives in docs/LOCALIZATION.md.
 *
 * Status semantics:
 * - `shipped`  — full website parity with English on first-class pages.
 * - `partial`  — routed and selectable, but intentionally incomplete:
 *                chrome (nav/footer/switcher), the home page, and metadata
 *                are localized via web/lib/i18n/dictionaries/<code>/ and
 *                everything else falls back to English. The switcher marks
 *                these with a visible partial badge. No route 404s and no
 *                untranslated string ever renders a dictionary key.
 * - `planned`  — tracked in docs/LOCALIZATION.md with a target issue; not
 *                routed.
 * - `deferred` — acknowledged but not scheduled; not routed.
 *
 * When adding a locale:
 * 1. Add/flip the entry in `ALL_LOCALES` below.
 * 2. Scaffold dictionaries under web/lib/i18n/dictionaries/<code>/
 *    (chrome.ts + home.ts — see dictionaries/en/ for the reference shape).
 * 3. Run `npm run check:locales` (dictionary key parity) and `npm test`.
 * 4. Update docs/LOCALIZATION.md.
 */

/** Status of a locale relative to the website. */
export type LocaleStatus = "shipped" | "partial" | "planned" | "deferred";

/** Direction of the locale's script — used for the html `dir` attribute. */
export type LocaleDirection = "ltr" | "rtl";

export interface LocaleEntry {
  /** ISO 639-1 or IETF BCP 47 language tag used in routes. */
  code: string;
  /** Display label (native script). */
  label: string;
  /** Status relative to the website. */
  status: LocaleStatus;
  /** Text direction for the html `dir` attribute. Defaults to "ltr". */
  dir?: LocaleDirection;
  /** One-line scope note shown to maintainers (not rendered). */
  note?: string;
}

/** Text direction of a tracked locale ("ltr" for untracked codes). */
export function localeDirection(locale: string): LocaleDirection {
  return ALL_LOCALES.find((l) => l.code === locale)?.dir ?? "ltr";
}

/**
 * All locales the project tracks, ordered by priority.
 *
 * SHIPPED and PARTIAL locales are included in `locales` (the constrained
 * set used by Next.js route generation and middleware). PLANNED and
 * DEFERRED locales are listed here for the matrix but not routed.
 */
export const ALL_LOCALES: LocaleEntry[] = [
  { code: "en", label: "English", status: "shipped" },
  {
    code: "zh",
    label: "中文",
    status: "shipped",
    // Shipped on the strength of translated first-class page BODIES
    // (install, FAQ, community, contribute, models, runtime) — a chrome
    // dictionary alone never earns `shipped`. Chrome + home moved to
    // dictionaries/zh/ in #4934; the remaining page bodies are still
    // inline `{ en, zh }` content modules awaiting the same move.
    note: "#4934 — chrome + home dictionary-backed; first-class page bodies translated inline",
  },
  {
    code: "ja",
    label: "日本語",
    status: "partial",
    note: "#3091 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "vi",
    label: "Tiếng Việt",
    status: "partial",
    note: "#3091 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "ko",
    label: "한국어",
    status: "partial",
    note: "#3093 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "ru",
    label: "Русский",
    status: "partial",
    note: "#3092 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "uk",
    label: "Українська",
    status: "partial",
    note: "#4791 — shipped alongside Russian; same partial scope",
  },
  {
    code: "es",
    label: "Español",
    status: "partial",
    note: "#3093 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "pt-BR",
    label: "Português (BR)",
    status: "partial",
    note: "#3093 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "fr",
    label: "Français",
    status: "partial",
    note: "#4788 — TUI pack shipped in v0.9.2; chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "de",
    label: "Deutsch",
    status: "partial",
    note: "#4788 — TUI pack shipped in v0.9.2; chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "ca",
    label: "Català",
    status: "partial",
    note: "#4749/#4788 — TUI pack shipped in v0.9.2; chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "id",
    label: "Bahasa Indonesia",
    status: "partial",
    note: "#4789 — chrome + home localized; page bodies fall back to English",
  },
  {
    code: "hi",
    label: "हिन्दी",
    status: "partial",
    note: "#4790 — TUI pack shipped in v0.9.2; chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "tr",
    label: "Türkçe",
    status: "partial",
    note: "major-language wave — chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "it",
    label: "Italiano",
    status: "partial",
    note: "major-language wave — chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "pl",
    label: "Polski",
    status: "partial",
    note: "major-language wave — chrome + home + docs-guide localized; page bodies fall back to English",
  },
  {
    code: "ar",
    label: "العربية",
    status: "partial",
    dir: "rtl",
    note: "RTL — html dir derived from this entry; chrome + home + docs-guide localized, page bodies fall back to English",
  },
];

/**
 * Active website locales (used by Next.js route generation, middleware,
 * sitemap, and the switcher). Both `shipped` and `partial` locales route;
 * `partial` locales carry a visible partial-pack status in the switcher.
 */
export const locales = ALL_LOCALES.filter(
  (l) => l.status === "shipped" || l.status === "partial",
).map((l) => l.code) as readonly string[];

export type Locale = (typeof locales)[number];
export const defaultLocale: Locale = "en";

/** Locales whose packs are intentionally incomplete (English fallback). */
export const partialLocales = ALL_LOCALES.filter((l) => l.status === "partial").map(
  (l) => l.code,
) as readonly string[];

export function isPartialLocale(x: string): boolean {
  return partialLocales.includes(x);
}

/** Set to "1" once the Gitee mirror at gitee.com/Hmbown/... exists. */
export const GITEE_ENABLED = process.env.NEXT_PUBLIC_GITEE_ENABLED === "1";

export function isValidLocale(x: string): x is Locale {
  return (locales as readonly string[]).includes(x);
}

/** Check if a locale code is tracked (shipped, partial, planned, or deferred). */
export function isTrackedLocale(x: string): boolean {
  return ALL_LOCALES.some((l) => l.code === x);
}
