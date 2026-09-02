import type { DocsGuideDict } from "../types";

/**
 * English reference dictionary for the docs "Getting started" page.
 * Copy moved verbatim from `app/[locale]/docs/guide/page.tsx` — any wording
 * change belongs in its own commit, never mixed into a structural move.
 */
export const docsGuide: DocsGuideDict = {
  metaTitle: "Getting started · Codewhale Docs",
  metaDescription:
    "The full path from install to your ideal fleet: install, a first keyless session, provider connection, and fleet setup.",
  bodyClassName: "text-ink-soft leading-relaxed",
  overviewTitle: "Getting started",
  overviewLead:
    "Four steps from one install command to a fleet set up for your work.",
  sessionTitle: "Watch a real session",
  sessionLead:
    "A recording of a real session will go here. There is no recording yet, so nothing is shown.",
  nextTitle: "Where next",
  sourceNote:
    "Source documents: docs/GUIDE.md, docs/KEYBINDINGS.md · Step copy lives in web/lib/content/getting-started.ts; update docs-map.ts when changing.",
};
