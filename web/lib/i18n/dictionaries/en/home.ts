import type { HomeDict } from "../types";

/**
 * English reference home dictionary — the copy contract for the
 * newspaper-ocean landing page. Public-copy and public-surface tests assert
 * against these values, not against raw JSX strings.
 *
 * The `seal*` values are the paper's section seals. They are glyphs (marks,
 * not prose), so locales share them by default; the keys exist so a locale
 * that needs a different mark can set one without touching the page.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — dives into the deep so you don't have to.",
  metaDescription:
    "Codewhale dives into the deep so you don't have to — an open-source terminal coding agent. Bring your own model. Runs on your machine. Rust, MIT.",

  kicker: "Open source · Bring your own model · Runs in your terminal",
  heroTitleA: "Codewhale dives into the deep",
  heroTitleB: "so you don't have to.",
  heroIntro:
    "{brand} is an open-source coding agent for your terminal. Give it a model and a task. It reads your code, edits files, runs the checks, and stops when the job is done or it needs you. Use any model, or a different one for each role.",
  install: "Install",
  docs: "Docs",
  copy: "Copy",
  copied: "Copied ✓",

  installEyebrow: "one-line install",
  installRequirement: "needs Node 18+ — no Rust toolchain",
  installOtherWays: "other ways →",

  latestRelease: "Latest release {tag}",
  releaseUnavailable: "Release status unavailable",
  currentSource: "Source",
  sourceCandidate: "Unreleased",
  providerRoutes: "{count} providers",
  publishedRelease: "released",
  figcaptionSourceCandidate: "unreleased",

  shotSession: "Session",
  screenshotAlt:
    "Codewhale terminal session in Operate mode: the whale, the composer, and the status footer",
  figcaption: "Codewhale session · Operate mode · permissions: Ask",

  proofHeading: "A coding agent in your terminal. Any model. On your machine.",
  proofBody:
    "Use the model you already have — hosted, through a gateway, or local. Pick a mode: Plan, Work, or Operate. Pick how much it does without asking: Ask, Auto-Review, or Full Access.",

  sealDecides: "法",
  decidesEyebrow: "How it decides",
  decidesHeading: "The reasoning, in its own words",
  decidesLede:
    "Session excerpts. Each shows the project rule the model applied and what it did next.",

  sealWorkflow: "行",
  workflowHeading: "From task to verified change.",
  workflow: [
    ["Inspect", "Read the repository, its instructions, and the task."],
    ["Act", "Edit files, asking first where you told it to."],
    ["Verify", "Run the checks and read the result."],
    ["Report", "Say what changed and what passed."],
  ],
  receiptAria: "Example run summary",
  receiptInspect: "repository and instructions",
  receiptAct: "edit files under the permissions you set",
  receiptReport: "checks passed · summary saved",

  sealStart: "起",
  startHeading: "New here? Four steps.",
  startLede:
    "Install → first session, no key → connect a provider → set up a fleet.",
  startGuideLink: "Read the getting-started guide →",
  startVocabularyLink: "Look up a term →",

  sealBoundaries: "界",
  boundariesHeadingA: "Your model.",
  boundariesHeadingB: "Your boundaries.",
  boundariesBody:
    "You choose the model, the mode, and how much it may do without asking. The provider and model never change unless you change them. Preview features are marked preview.",
  hostedGatewayLocal: "Hosted, gateway, and local models",
  planActOperateDesc: "From read-only planning to autonomous operation",
  askAutoReviewDesc: "How much it does before asking you",
  tuiExecWebDesc: "Interactive or scripted",

  sealSurfaces: "面",
  surfacesHeading: "Use it where the work happens.",
  surfaces: [
    ["TUI", "Interactive terminal work"],
    ["codewhale exec", "Scripts and CI"],
    ["Web client", "Browser client, localhost only"],
    ["Runtime API + MCP", "Local integrations"],
    ["Fleet", "Several agents on one job"],
  ],
  runtimeLink: "Runtime surfaces and what is stable →",

  installBandHeading: "Start with one command.",
  binaries: "Binaries",
  chinaMirrors: "China mirrors",
  installGuideLink: "Read the install guide →",

  sealCommunity: "众",
  communityHeading: "Built in public",
  communityBody:
    "MIT license. Contributors work on the runtime, providers, platforms, docs, and tests.",
  communityLinksAria: "Community links",
  contribute: "Contribute",
};
