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
    "{brand} is an open-source coding agent for your terminal. Give it a model and a task — it reads your code, edits files, runs its own checks, and stops when the job's done or it needs you. Bring any model, or mix them: pin a different model to each role.",
  install: "Install",
  docs: "Docs",
  copy: "Copy",
  copied: "Copied ✓",

  installEyebrow: "one-line install",
  installRequirement: "needs Node 18+ — no Rust toolchain",
  installOtherWays: "other ways →",

  latestRelease: "Latest release {tag}",
  releaseUnavailable: "Release status unavailable",
  currentSource: "Current source",
  sourceCandidate: "Source candidate",
  providerRoutes: "{count} provider routes",
  publishedRelease: "published release",
  figcaptionSourceCandidate: "source candidate",

  shotSession: "Current session",
  screenshotAlt:
    "Current Codewhale terminal session showing Operate mode, the whale, composer, and footer",
  figcaption: "Current Codewhale session · Operate mode · Ask permission posture",

  proofHeading: "An underwater terminal shell. Any model. On your machine.",
  proofBody:
    "Bring the model you already use — hosted, gateway, or local. Plan / Work / Operate and explicit permission postures keep the dive under your control.",

  sealDecides: "法",
  decidesEyebrow: "See how it decides",
  decidesHeading: "Law you can watch in the trace",
  decidesLede:
    "Real session excerpts — ranked project law visible in the model's reasoning, not just a landing-page claim.",

  sealWorkflow: "行",
  workflowHeading: "From task to verified change.",
  workflow: [
    ["Inspect", "Read the repository, its instructions, and the task."],
    ["Act", "Edit files through explicit approval boundaries."],
    ["Verify", "Run checks and inspect the result."],
    ["Report", "Leave a concise, durable receipt."],
  ],
  receiptAria: "Example work receipt",
  receiptInspect: "repository and instructions",
  receiptAct: "edit through the selected permission posture",
  receiptReport: "checks passed · receipt saved",

  sealStart: "起",
  startHeading: "New to Codewhale? Four steps end to end.",
  startLede:
    "Install → keyless first session → connect a provider → set up your fleet. Nouns defined on the vocabulary page.",
  startGuideLink: "Read the getting-started guide →",
  startVocabularyLink: "See the product vocabulary →",

  sealBoundaries: "界",
  boundariesHeadingA: "Your model.",
  boundariesHeadingB: "Your boundaries.",
  boundariesBody:
    "Choose the model, working mode, and permission posture explicitly. Unknown cost stays unknown, and preview surfaces stay labeled as such.",
  hostedGatewayLocal: "Hosted, gateway, and local models",
  planActOperateDesc: "Read-only planning through autonomous operation",
  askAutoReviewDesc: "Choose the permission posture for the work",
  tuiExecWebDesc: "Interactive and headless runtime surfaces",

  sealSurfaces: "面",
  surfacesHeading: "Use the runtime where the work happens.",
  surfaces: [
    ["TUI", "Interactive terminal work"],
    ["codewhale exec", "Scripts and CI"],
    ["Web client", "Loopback-only browser client"],
    ["Runtime API + MCP", "Local integrations"],
    ["Fleet", "Durable multi-agent work"],
  ],
  runtimeLink: "See runtime surfaces and stability notes →",

  installBandHeading: "Start with one command.",
  binaries: "Binaries",
  chinaMirrors: "China mirrors",
  installGuideLink: "Read the install guide →",

  sealCommunity: "众",
  communityHeading: "Built in public",
  communityBody:
    "MIT-licensed and shaped by contributors across runtimes, providers, platforms, documentation, and tests.",
  communityLinksAria: "Community links",
  contribute: "Contribute",
};
