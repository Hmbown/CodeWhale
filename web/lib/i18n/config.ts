/**
 * Locale configuration for codewhale.net.
 *
 * SHIPPED locales have full website routes and dictionaries.
 * PLANNED locales are tracked in docs/LOCALIZATION.md with target issues.
 * DEFERRED locales are acknowledged but not yet scheduled.
 *
 * When adding a locale:
 * 1. Add the code to the `locales` array below.
 * 2. Add a label in web/components/locale-switcher.tsx → LOCALE_LABELS.
 * 3. Scaffold dictionaries under web/lib/i18n/dictionaries/<code>/.
 * 4. Update docs/LOCALIZATION.md.
 */

/** Status of a locale relative to the website. */
export type LocaleStatus = "shipped" | "partial" | "planned" | "deferred";

export interface LocaleEntry {
  /** ISO 639-1 or IETF BCP 47 language tag used in routes. */
  code: string;
  /** Display label (native script). */
  label: string;
  /** Status relative to the website. */
  status: LocaleStatus;
}

/**
 * All locales the project tracks, ordered by priority.
 *
 * SHIPPED locales are included in `locales` (the constrained set used
 * by Next.js route generation). PLANNED and DEFERRED locales are listed
 * here for the matrix but not yet included in route generation.
 */
export const ALL_LOCALES: LocaleEntry[] = [
  { code: "en", label: "English", status: "shipped" },
  { code: "zh", label: "中文", status: "shipped" },
  { code: "ja", label: "日本語", status: "planned" },
  { code: "vi", label: "Tiếng Việt", status: "planned" },
  { code: "ko", label: "한국어", status: "planned" },
  { code: "ru", label: "Русский", status: "planned" },
  { code: "es", label: "Español", status: "deferred" },
  { code: "pt-BR", label: "Português (BR)", status: "deferred" },
  { code: "ar", label: "العربية", status: "deferred" },
];

/** Active website locales (used by Next.js route generation). */
export const locales = ALL_LOCALES.filter((l) => l.status === "shipped").map((l) => l.code) as readonly string[];

export type Locale = (typeof locales)[number];
export const defaultLocale: Locale = "en";

/** Set to "1" once the Gitee mirror at gitee.com/Hmbown/... exists. */
export const GITEE_ENABLED = process.env.NEXT_PUBLIC_GITEE_ENABLED === "1";

export function isValidLocale(x: string): x is Locale {
  return (locales as readonly string[]).includes(x);
}

/** Check if a locale code is tracked (shipped, planned, or deferred). */
export function isTrackedLocale(x: string): boolean {
  return ALL_LOCALES.some((l) => l.code === x);
}
