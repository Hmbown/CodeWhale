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
 * "Plan · Work · Operate", "Ask · Auto-Review · Full Access",
 * "TUI · exec · web · API", "Codewhale", "GitHub", "Issues",
 * `npm install -g codewhale`, `cargo test --locked`, `codewhale exec`,
 * package-manager proper nouns, mirror names, and `/codewhale-tui.webp`.
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

  /** Header account links to the Codewhale app (app.codewhale.net). */
  authSignIn: string;
  authRegister: string;
  /** aria-label for the header account link group. */
  authGroupAria: string;

  /** Wordmark seal glyph beside the masthead brand (components/seal.tsx). */
  wordmarkSeal: string;
  /** Wordmark strapline under the brand, e.g. "any model, on your machine". */
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

  /**
   * Ticker event verbs — the chrome around the repository's own record.
   * Pull-request titles, issue titles, release tags, and contributor handles
   * are CONTENT and stay verbatim in every locale; these verbs are copy and
   * must be translated.
   *
   * `tickerReleased` covers `state: "published"`, and `tickerOpened` covers
   * both a newly filed issue and an open pull request. There is deliberately
   * no draft verb: the strip reports events, and a draft pull request is one
   * its author has marked not-ready (components/ticker.tsx `EVENT_STATES`).
   */
  tickerMerged: string;
  tickerOpened: string;
  tickerClosed: string;
  tickerReleased: string;
  /**
   * Mark shown when GitHub itself reports the author as a
   * FIRST_TIME_CONTRIBUTOR — the warmest item on the strip, and never our
   * inference. Keep it short; it sits inline in a scrolling mono line.
   */
  tickerFirstContribution: string;
  /**
   * By-line template carrying a `{handle}` token, e.g. "by {handle}". The
   * handle is typeset in its own element, so a locale may place it anywhere
   * (or make it the whole value, as ja/ko do with an honorific suffix).
   */
  tickerBy: string;
  /** aria-label for the ticker's group landmark. */
  tickerAria: string;

  /** Reasoning trace title-bar label, e.g. "reasoning trace". */
  traceLabel: string;
  /** aria-label for the reasoning trace scene tablist. */
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
  /** Footer link to the pricing page, e.g. "Pricing". */
  footerPricing: string;
  /** Footer link to the terms route, e.g. "Terms". */
  footerTerms: string;
  /** Footer link to the privacy route, e.g. "Privacy". */
  footerPrivacy: string;
  /** Footer Product-column link to the release record, e.g. "Changelog". */
  footerChangelog: string;
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
  /** Screenshot alt text for /codewhale-tui.webp. */
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

/**
 * Docs "Getting started" page (`app/[locale]/docs/guide/page.tsx`).
 *
 * First of the per-page dictionaries that retire the page-body `isZh`
 * branches left after #4934. Page dictionaries are optional per locale:
 * English is the required reference, any other locale that ships the file
 * is held to exact key parity (`check-locales.mjs` OPTIONAL_FILES), and a
 * locale without it falls back to English at lookup time — matching how
 * page bodies already behave for partial locales.
 */
export interface DocsGuideDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  sessionTitle: string;
  sessionLead: string;
  nextTitle: string;
  sourceNote: string;
}

/**
 * The docs shell: the portal hero in `app/[locale]/docs/layout.tsx` that
 * wraps every docs page, plus the metadata of the hub page it frames
 * (`app/[locale]/docs/page.tsx`, whose body is the `DocsSearch` component).
 *
 * No `bodyClassName` here: the hero typesets through `portal-*` classes,
 * which never varied by locale.
 */
export interface DocsShellDict {
  metaTitle: string;
  metaDescription: string;
  portalMark: string;
  heroTitle: string;
  heroLead: string;
  installCta: string;
  sourceDocsCta: string;

  // --- release truth band (docs layout; facts + CHANGELOG.md) ---
  /** Eyebrow over the band, e.g. "Release truth". */
  releaseLabel: string;
  /** "Latest release {tag} · {date}" — date already formatted per locale. */
  releasePublished: string;
  /** Shown while the source candidate is ahead of the published release. */
  releaseCandidate: string;
  /** Shown when the pages describe exactly the published release. */
  releaseMatches: string;
  /** Link to the /changelog route. */
  releaseChangelog: string;

  // --- task/topic search (components/docs-search.tsx) ---
  searchLabel: string;
  searchPlaceholder: string;
  searchClear: string;
  /** "{matched} of {total} entries match “{query}”". */
  searchMatches: string;
  /** "Nothing matches “{query}”". */
  searchNoMatches: string;
  tasksHeading: string;
  tasksLead: string;
  topicsHeading: string;
  /** Row tag for a first-party page. */
  webGuideTag: string;
  /** Row tag for a GitHub source document. */
  sourceDocTag: string;
  emptyTitle: string;
  emptyBody: string;
  emptyCta: string;
  indexNote: string;

  // --- sidebar and breadcrumb (components/docs-sidebar.tsx, docs-breadcrumb.tsx) ---
  sidebarHeading: string;
  sidebarAria: string;
  breadcrumbAria: string;
  breadcrumbHome: string;
  breadcrumbDocs: string;

  // --- contextual help band under every docs page ---
  helpTitle: string;
  helpLead: string;
  /** "Source: {name}" — the topic's repository document(s). */
  helpSource: string;
  helpTroubleshooting: string;
  helpFaq: string;
  helpDiscord: string;
  helpIssue: string;
}

/**
 * Shared surface states (`components/surface-state.tsx`,
 * `components/connection-banner.tsx`, the `loading.tsx` / `error.tsx` /
 * `not-found.tsx` boundaries). One dictionary for every empty, loading,
 * error, retry, recovery, and connection state so no page invents its own.
 */
export interface StatesDict {
  loadingLabel: string;
  emptyTitle: string;
  emptyBody: string;
  errorTitle: string;
  errorBody: string;
  retry: string;
  reload: string;
  homeLink: string;
  /** The 404 plate's primary action: the docs index, which the body names. */
  docsIndexLink: string;
  notFoundTitle: string;
  notFoundBody: string;
  /**
   * A data-bearing page whose source was not asked (build-time prerender)
   * or refused (rate limit, outage). Distinct from `empty`, which asserts
   * that nothing exists, and from `error`, which means the render threw.
   */
  unavailableTitle: string;
  unavailableBody: string;
  /** Connection banner (typed state in lib/connection-state.ts). */
  offlineTitle: string;
  offlineBody: string;
  reconnectingTitle: string;
  /** "Checking the connection (attempt {attempt})." */
  reconnectingBody: string;
  degradedTitle: string;
  degradedBody: string;
  onlineTitle: string;
  onlineBody: string;
  retryNow: string;
  dismiss: string;
  /** "Last checked {time}". */
  lastChecked: string;
}

/**
 * `app/[locale]/docs/computers/page.tsx` — cloud computers and dispatch.
 * Every claim traces to docs/DAYTONA_CLOUD_DISPATCH.md and
 * docs/CODEWHALE_AGENT.md; commands stay code-owned literals in the page.
 */
export interface DocsComputersDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  proposeTitle: string;
  proposeLead: string;
  jobsLead: string;
  remotesTitle: string;
  remotesLead: string;
  /** `[remote name, forge]` rows of the explicit-forge table. */
  remotes: [string, string][];
  enableTitle: string;
  enableLead: string;
  /** `[step, detail]` rows. */
  enableSteps: [string, string][];
  cliNote: string;
  rulesTitle: string;
  /** `[situation, outcome]` rows of the fail-closed table. */
  rules: [string, string][];
  membershipTitle: string;
  membershipLead: string;
  leftoverTitle: string;
  /** `[item, note]` rows. */
  leftover: [string, string][];
  sourceNote: string;
}

/**
 * `app/[locale]/docs/auth/page.tsx` — account, sign-in, and key storage.
 * Claims trace to docs/CONFIGURATION.md, docs/CODEWHALE_AGENT.md, and
 * docs/PROVIDERS.md.
 */
export interface DocsAuthDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  /** `[credential, role]` rows. */
  credentials: [string, string][];
  providerTitle: string;
  providerLead: string;
  accountTitle: string;
  accountLead: string;
  /** `[command, effect]` rows. */
  accountCommands: [string, string][];
  storageTitle: string;
  storageLead: string;
  vaultTitle: string;
  vaultLead: string;
  portableTitle: string;
  portableLead: string;
  appTitle: string;
  appLead: string;
  appSignIn: string;
  appRegister: string;
  sourceNote: string;
}

/**
 * `app/[locale]/docs/trust/page.tsx` — security and trust reference.
 * Claims trace to docs/SANDBOX.md, docs/AUTHORIZATION_ORDER.md,
 * docs/TELEMETRY.md, and docs/public-surface-facts.json.
 */
export interface DocsTrustDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  boundaryTitle: string;
  /** `[boundary, statement]` rows. */
  boundaries: [string, string][];
  approvalTitle: string;
  approvalLead: string;
  sandboxTitle: string;
  sandboxLead: string;
  /** `[platform, behaviour]` rows. */
  sandboxes: [string, string][];
  sandboxNote: string;
  telemetryTitle: string;
  telemetryLead: string;
  /** `[fact, detail]` rows. */
  telemetry: [string, string][];
  auditTitle: string;
  auditLead: string;
  reportTitle: string;
  reportLead: string;
  reportCta: string;
  sourceNote: string;
}

/** `app/[locale]/changelog/page.tsx` — version-aware release record. */
export interface ChangelogDict {
  metaTitle: string;
  metaDescription: string;
  kicker: string;
  title: string;
  lead: string;
  publishedLabel: string;
  /** "{tag} · published {date}" */
  publishedValue: string;
  candidateLabel: string;
  /** "{version} · unreleased" */
  candidateValue: string;
  /** Shown when the candidate equals the published release. */
  candidateMatches: string;
  releasesLink: string;
  unreleasedHeading: string;
  unreleasedNote: string;
  compareLink: string;
  releasePageLink: string;
  /** "{shown} of {total} entries" */
  moreEntries: string;
  fullNotes: string;
  /** "Full notes for {version}" — per-release deep link into CHANGELOG.md. */
  releaseNotesLink: string;
  emptyTitle: string;
  emptyBody: string;
}

/**
 * `app/[locale]/docs/hooks/page.tsx`.
 *
 * `configIntro` carries three `{token}` placeholders for the literal
 * `[[hooks.hooks]]`, `/hooks` and `[hooks].enabled` spans the page typesets
 * as inline `<code>`. They stay out of the prose because they are config
 * syntax rather than copy, and the token-parity half of `check-locales.mjs`
 * then guards the sentence for free.
 */
export interface DocsHooksDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  configIntro: string;
  /** Lifecycle events as `[name, detail]`, in the order the page lists them. */
  events: [string, string][];
  projectTitle: string;
  projectLead: string;
  sourceNote: string;
}

/** `app/[locale]/docs/troubleshooting/page.tsx`. */
export interface DocsTroubleshootingDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  /** Triage entries as `[name, detail]`, in the order the page lists them. */
  incidents: [string, string][];
  dockerTitle: string;
  dockerLead: string;
  dockerToolboxNote: string;
  sourceNote: string;
}

/** `app/[locale]/docs/configuration/page.tsx`. */
export interface DocsConfigurationDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  auditLead: string;
  overlayTitle: string;
  overlayLead: string;
  overlayLimits: string;
  credentialsTitle: string;
  credentialsLead: string;
  legacyTitle: string;
  legacyLead: string;
  sourceNote: string;
}

/**
 * `app/[locale]/docs/constitution/page.tsx`.
 *
 * `overviewLead` carries three `{token}` placeholders and `authorityNote` one
 * more. Per `docs/VOICE.md` — keep commands, key names and paths as code-owned
 * placeholders — the literals themselves live in the page, along with the
 * en/zh badge pair on each principle row, which is a fixed bilingual glyph
 * rather than copy.
 */
export interface DocsConstitutionDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewTitle: string;
  /** Opposite-language echo the heading prints beside the title. */
  overviewTitleAside: string;
  overviewLead: string;
  /** Three `[key, detail]` rows; the key selects the page's badge pair. */
  principles: [string, string][];
  authorityNote: string;
  /** Link text inside `authorityNote`'s `{configDocs}` slot. */
  configDocsLabel: string;
  sourceNote: string;
}

/** `app/[locale]/docs/fleet/page.tsx` (the `DocsFleetDict` name is internal compatibility). */
export interface DocsFleetDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  runTitle: string;
  runLead: string;
  statusLead: string;
  profilesTitle: string;
  profilesLead: string;
  workflowTitle: string;
  workflowLead: string;
  workflowLimits: string;
  sourceNote: string;
}

/** `app/[locale]/docs/mcp/page.tsx`. */
export interface DocsMcpDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewLead: string;
  overviewConfig: string;
  setupTitle: string;
  setupLead: string;
  setupReload: string;
  authTitle: string;
  authLead: string;
  toolsTitle: string;
  toolsLead: string;
  toolsTrust: string;
  serverTitle: string;
  serverLead: string;
  sourceNote: string;
}

/** `app/[locale]/docs/modes/page.tsx`. */
export interface DocsModesDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  modes: [string, string][];
  switchingTitle: string;
  switchingLead: string;
  switchingCommandLead: string;
  permissionsTitle: string;
  permissionsLead: string;
  postures: [string, string][];
  sourceNote: string;
}

/**
 * `app/[locale]/docs/runtime-api/page.tsx`.
 *
 * Only `securityLead` is tokenized. The command names elsewhere on the page
 * are set as prose rather than `<code>`, so they stay inside the sentences
 * exactly as the `isZh` ternaries had them.
 */
export interface DocsRuntimeApiDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  /** Seven `[key, detail]` rows; the key selects the page's command literal. */
  entries: [string, string][];
  stdioTitle: string;
  stdioLead: string;
  interruptNote: string;
  securityTitle: string;
  securityLead: string;
  sourceNote: string;
}

export interface DocsSandboxDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  /** Four `[name, detail]` rows; the name is also the row's key. */
  platforms: [string, string][];
  policiesTitle: string;
  policiesLead: string;
  diagnosticsTitle: string;
  diagnosticsLead: string;
  diagnosticsLimits: string;
  sourceNote: string;
}

/** `app/[locale]/docs/subagents/page.tsx`. */
export interface DocsSubagentsDict {
  metaTitle: string;
  metaDescription: string;
  /** Body-copy typography for this locale (CJK needs looser leading). */
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  overviewFleetNote: string;
  /** Eight `[key, detail]` rows; the key names the page's role literal. */
  roles: [string, string][];
  forkTitle: string;
  forkLead: string;
  worktreeTitle: string;
  worktreeLead: string;
  capacityTitle: string;
  capacityLead: string;
  sourceNote: string;
}

export interface DocsWebDict {
  metaTitle: string;
  metaDescription: string;
  bodyClassName: string;
  overviewTitle: string;
  overviewLead: string;
  overviewBody: string;
  authTitle: string;
  authLead: string;
  localTitle: string;
  localLead: string;
  troubleshootingTitle: string;
  troubleshootingLead: string;
  sourceNote: string;
}
