/**
 * Locale-aware chrome link sets (#4934).
 *
 * Nav and footer used to carry hardcoded `/en/...` and `/zh/...` arrays plus
 * a thin dictionary branch for everything else, so foreign locales silently
 * lost the Start and FAQ links. There is now one generator per link set:
 * labels come from `ChromeDict`, hrefs come from the locale, and every
 * routed locale gets the identical route shape. Locale-swap parity is a
 * unit test (`docs-ia.test.ts`) instead of a string scrape over the TSX.
 */
import type { ChromeDict } from "./dictionaries/types";

export const REPO_URL = "https://github.com/Hmbown/CodeWhale";
export const REPO_ISSUES_URL = `${REPO_URL}/issues`;
export const REPO_RELEASES_URL = `${REPO_URL}/releases`;
export const REPO_LICENSE_URL = `${REPO_URL}/blob/main/LICENSE`;

/** A chrome link. `secondary` is the small bilingual companion label. */
export interface ChromeLink {
  href: string;
  label: string;
  secondary?: string;
}

/** The six primary nav links, identical in shape for every routed locale. */
export function navLinks(locale: string, chrome: ChromeDict): ChromeLink[] {
  return [
    { href: `/${locale}/docs`, label: chrome.navDocs, secondary: chrome.navDocsSecondary },
    {
      href: `/${locale}/docs/guide`,
      label: chrome.navStart,
      secondary: chrome.navStartSecondary,
    },
    {
      href: `/${locale}/install`,
      label: chrome.navInstall,
      secondary: chrome.navInstallSecondary,
    },
    { href: `/${locale}/faq`, label: chrome.navFaq, secondary: chrome.navFaqSecondary },
    {
      href: `/${locale}/community`,
      label: chrome.navCommunity,
      secondary: chrome.navCommunitySecondary,
    },
    {
      href: `/${locale}/contribute`,
      label: chrome.navContribute,
      secondary: chrome.navContributeSecondary,
    },
  ];
}

/** Footer "Product" column — the in-site discovery links. */
export function footerProductLinks(locale: string, chrome: ChromeDict): ChromeLink[] {
  return [
    { href: `/${locale}/docs`, label: chrome.footerDocs },
    { href: `/${locale}/docs/guide`, label: chrome.footerGuide },
    { href: `/${locale}/install`, label: chrome.footerInstall },
    { href: `/${locale}/models`, label: chrome.footerModels },
    { href: `/${locale}/runtime`, label: chrome.footerRuntime },
    { href: `/${locale}/faq`, label: chrome.footerFaq },
  ];
}

/** Footer "Project" column — GitHub plus the pinned legal link. */
export function footerProjectLinks(locale: string, chrome: ChromeDict): ChromeLink[] {
  return [
    { href: REPO_URL, label: "GitHub" },
    { href: REPO_ISSUES_URL, label: chrome.footerIssues },
    { href: `/${locale}/contribute`, label: chrome.footerContribute },
    { href: REPO_LICENSE_URL, label: chrome.footerLicense },
  ];
}
