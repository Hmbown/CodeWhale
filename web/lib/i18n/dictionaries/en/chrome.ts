import type { ChromeDict } from "../types";

/**
 * English reference chrome dictionary. Every other locale must match these
 * keys exactly (`npm run check:locales`, `dictionaries.test.ts`).
 *
 * The Han secondary labels and the 深 wordmark seal are the English
 * edition's own editorial device — the newspaper masthead sets them beside
 * the Latin labels on purpose. Other locales pair their native label with a
 * short English one instead; nothing hardcodes Han at a call site.
 */
export const chrome: ChromeDict = {
  navDocs: "Docs",
  navStart: "Start",
  navInstall: "Install",
  navFaq: "FAQ",
  navCommunity: "Community",
  navContribute: "Contribute",

  navDocsSecondary: "文档",
  navStartSecondary: "指引",
  navInstallSecondary: "安装",
  navFaqSecondary: "问答",
  navCommunitySecondary: "社区",
  navContributeSecondary: "贡献",

  skipToContent: "Skip to main content",


  navPrimaryAria: "Primary",
  navHomeAria: "Codewhale home",

  installCta: "Install →",

  authSignIn: "Sign in",
  authRegister: "Register",
  authGroupAria: "Account",

  wordmarkSeal: "深",
  wordmarkTag: "any model, on your machine",

  issueLabel: "Issue {date}",
  dateLocale: "en-US",

  starsAria: "GitHub stars",
  githubFallback: "GitHub",

  tickerLiveLabel: "实 时",
  tickerLiveTag: "LIVE",
  tickerMerged: "merged",
  tickerOpened: "opened",
  tickerClosed: "closed",
  tickerReleased: "released",
  tickerFirstContribution: "first contribution",
  tickerBy: "by {handle}",
  tickerAria: "Recent repository activity",

  traceLabel: "reasoning trace",
  traceTabsAria: "Session excerpts",

  menuOpen: "Open menu",
  menuClose: "Close menu",

  themeAuto: "auto",
  themeLight: "light",
  themeDark: "dark",
  themeAria: "Docs theme: {mode} (click to cycle)",
  themeTitle: "Docs theme · auto / light / dark",

  footerTagline:
    "Codewhale dives into the deep so you don't have to — docs, source, and community for the open-source runtime.",
  footerProduct: "Product",
  footerProject: "Project",
  footerDocs: "Docs",
  footerGuide: "Getting started",
  footerInstall: "Install",
  footerModels: "Models",
  footerRuntime: "Runtime",
  footerFaq: "FAQ",
  footerIssues: "Issues",
  footerContribute: "Contribute",
  footerLicense: "MIT license",
  footerPricing: "Pricing",
  footerTerms: "Terms",
  footerPrivacy: "Privacy",
  footerChangelog: "Changelog",
  footerCanonicalSource: "Canonical source: ",
  footerReleases: " · Releases: ",
  footerReleasesLink: "GitHub Releases",
  footerSecurity: "Security",

  switcherLabel: "Language",
  switcherSwitchTo: "Switch to {label}",
  partialBadge: "(partial)",
};
