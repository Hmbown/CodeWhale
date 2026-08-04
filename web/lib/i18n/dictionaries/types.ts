/**
 * Dictionary shapes for the website localization layer (#3091, #4934).
 *
 * `ChromeDict` covers shared chrome: the newspaper masthead, nav, mobile
 * menu, theme toggle, live ticker, footer, and the locale switcher with its
 * visible partial-pack badge. `HomeDict` covers the landing page
 * (`app/[locale]/page.tsx`). Templates use `{name}` tokens interpolated
 * with `fill()` from dictionaries/index.ts — never concatenate translated
 * sentences around variables in JSX.
 *
 * English (`dictionaries/en/`) is the reference shape; every routed locale —
 * including Chinese — must define exactly the same keys. Parity is enforced
 * by `web/scripts/check-locales.mjs` and `web/lib/i18n/dictionaries.test.ts`.
 * A locale without a dictionary falls back to the English one at lookup
 * time, so an untranslated string renders English copy — never a key.
 *
 * Code-owned strings stay out of these dictionaries per docs/VOICE.md:
 * "Plan · Act · Operate", "Ask · Auto-Review · Full Access",
 * "TUI · exec · web · API", "Codewhale", "GitHub", "Issues",
 * `npm install -g codewhale`, `cargo test --locked`, `codewhale exec`,
 * package-manager proper nouns, mirror names, and `/codewhale-tui.png`.
 */

export interface ChromeDict {
  // --- primary nav labels (components/nav.tsx via lib/i18n/links.ts) ---
  navDocs: string;
  navStart: string;
  navInstall: string;
  navFaq: string;
  navCommunity: string;
  navContribute: string;

  /**
   * Bilingual secondary nav labels — the small companion label the
   * newspaper masthead sets beside each primary link.
   *
   * The English edition uses the Han seal pair (文档 / 指引 / …) as an
   * editorial device; every other locale supplies its OWN pairing (native
   * primary, short English secondary). Never hardcode Han characters at a
   * call site — a locale that wants no second label still needs a value
   * here, because empty strings are rejected by dictionaries.test.ts.
   */
  navDocsSecondary: string;
  navStartSecondary: string;
  navInstallSecondary: string;
  navFaqSecondary: string;
  navCommunitySecondary: string;
  navContributeSecondary: string;

  /**
   * Skip-to-content link rendered before the nav in app/[locale]/layout.tsx.
   * It sits on EVERY page of EVERY locale, so it belongs to shared chrome —
   * leaving it hardcoded is what kept an EN/ZH branch alive in the layout.
   */
  skipToContent: string;

  /** aria-label for the primary <nav> landmark (components/nav-links.tsx). */
  navPrimaryAria: string;
  /** aria-label for the wordmark link back to the locale home. */
  navHomeAria: string;

  /** Mobile-menu and masthead call to action, e.g. "Install →". */
  installCta: string;

  /** Wordmark seal glyph beside the masthead brand (components/seal.tsx). */
  wordmarkSeal: string;
  /** Wordmark strapline under the brand, e.g. "any model, local-first". */
  wordmarkTag: string;

  /** Masthead issue line, e.g. "Issue {date}". */
  issueLabel: string;
  /**
   * BCP 47 tag used for the masthead weekday via `toLocaleDateString` — not
   * rendered copy, but per-locale, so it belongs beside it. Without this the
   * masthead date renders in English for every non-Chinese locale.
   */
  dateLocale: string;

  /** aria-label on the star-count link, e.g. "GitHub stars". */
  starsAria: string;
  /** Star-badge label when the live count is unavailable. */
  githubFallback: string;

  /** Live-ticker seal label (components/ticker.tsx). */
  tickerLiveLabel: string;
  /** Live-ticker mono tag beside the seal label, e.g. "LIVE". */
  tickerLiveTag: string;

  /** TerminalPlayer title-bar label, e.g. "reasoning trace". */
  traceLabel: string;
  /** aria-label for the TerminalPlayer scene tablist. */
  traceTabsAria: string;

  /** Mobile-menu toggle labels. */
  menuOpen: string;
  menuClose: string;

  /** Docs theme toggle: the three cycle states. */
  themeAuto: string;
  themeLight: string;
  themeDark: string;
  /** Theme toggle aria-label, e.g. "Docs theme: {mode} (click to cycle)". */
  themeAria: string;
  /** Theme toggle title attribute. */
  themeTitle: string;

  // --- footer ---
  footerTagline: string;
  footerProduct: string;
  footerProject: string;
  footerDocs: string;
  footerGuide: string;
  footerInstall: string;
  footerModels: string;
  footerRuntime: string;
  footerFaq: string;
  footerIssues: string;
  footerContribute: string;
  footerLicense: string;
  /** Prefix before the canonical-source link, e.g. "Canonical source: ". */
  footerCanonicalSource: string;
  /** Separator + label before the releases link, e.g. " · Releases: ". */
  footerReleases: string;
  /** Link text for the GitHub releases page. */
  footerReleasesLink: string;
  /** Link text for the security-contact mailto. */
  footerSecurity: string;

  /** aria-label for the locale switcher control. */
  switcherLabel: string;
  /** Two-locale toggle aria-label, e.g. "Switch to {label}". */
  switcherSwitchTo: string;
  /**
   * Visible badge marking a partial locale pack in the switcher, e.g.
   * "(partial)" — honest scope signal, per the localization quality
   * contract. Keep it short.
   */
  partialBadge: string;
}

export interface HomeDict {
  /**
   * `<title>` and meta description for the locale home route, consumed by
   * `generateMetadata` in app/[locale]/layout.tsx. These were the last
   * inline EN/ZH pair on the required slice; per-locale metadata is the
   * whole point of routing a locale, so it lives in the dictionary.
   */
  metaTitle: string;
  metaDescription: string;

  /** Hero pill, e.g. "Open source · Any model · Runs in your terminal". */
  kicker: string;
  heroTitleA: string;
  heroTitleB: string;
  /**
   * Hero lede. Carries a `{brand}` token so the brand can be typeset in its
   * own span wherever the sentence needs it — the page splits on the token
   * instead of concatenating fragments around it.
   */
  heroIntro: string;
  install: string;
  docs: string;
  copy: string;
  copied: string;

  /** Eyebrow above the one-line install block, e.g. "one-line install". */
  installEyebrow: string;
  /** Install prerequisite line, e.g. "needs Node 18+ — no Rust toolchain". */
  installRequirement: string;
  /** Link to the other install methods, e.g. "other ways →". */
  installOtherWays: string;

  /** "Latest release {tag}" */
  latestRelease: string;
  releaseUnavailable: string;
  /** "Current source" / "Source candidate" — prepended to `v{version}:`. */
  currentSource: string;
  sourceCandidate: string;
  /** "{count} provider routes" */
  providerRoutes: string;
  /** "published release" / "source candidate" — the source-state label. */
  publishedRelease: string;
  figcaptionSourceCandidate: string;

  /** Screenshot toolbar label, e.g. "Current session". */
  shotSession: string;
  /** Screenshot alt text for /codewhale-tui.png. */
  screenshotAlt: string;
  /** Screenshot figcaption. */
  figcaption: string;

  proofHeading: string;
  proofBody: string;

  /** Section seal glyph for the "see how it decides" band. */
  sealDecides: string;
  decidesEyebrow: string;
  decidesHeading: string;
  decidesLede: string;

  /** Section seal glyph for the workflow band. */
  sealWorkflow: string;
  workflowHeading: string;
  /** Four [title, description] steps. */
  workflow: [string, string][];
  receiptAria: string;
  /**
   * Right-hand column of the example receipt. The verbs (inspect / act /
   * verify / report), `$ codewhale exec …`, and `cargo test --locked` stay
   * code-owned literals in the JSX per docs/VOICE.md.
   */
  receiptInspect: string;
  receiptAct: string;
  receiptReport: string;

  /** Section seal glyph for the getting-started band. */
  sealStart: string;
  startHeading: string;
  startLede: string;
  startGuideLink: string;
  startVocabularyLink: string;

  /** Section seal glyph for the boundaries band. */
  sealBoundaries: string;
  boundariesHeadingA: string;
  boundariesHeadingB: string;
  boundariesBody: string;
  hostedGatewayLocal: string;
  planActOperateDesc: string;
  askAutoReviewDesc: string;
  tuiExecWebDesc: string;

  /** Section seal glyph for the surfaces band. */
  sealSurfaces: string;
  surfacesHeading: string;
  /** Five [name, description] surfaces. */
  surfaces: [string, string][];
  runtimeLink: string;

  installBandHeading: string;
  binaries: string;
  chinaMirrors: string;
  installGuideLink: string;

  /** Section seal glyph for the community band. */
  sealCommunity: string;
  communityHeading: string;
  communityBody: string;
  communityLinksAria: string;
  contribute: string;
}
