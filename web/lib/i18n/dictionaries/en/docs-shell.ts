import type { DocsShellDict } from "../types";

/**
 * English reference dictionary for the docs shell.
 * Copy moved verbatim from `app/[locale]/docs/layout.tsx` and the metadata
 * of `app/[locale]/docs/page.tsx` — any wording change belongs in its own
 * commit, never mixed into a structural move.
 */
export const docsShell: DocsShellDict = {
  metaTitle: "Docs · Codewhale",
  metaDescription:
    "Codewhale documentation: install, user guide, configuration, providers, core concepts, tools, MCP, skills, sandbox, runtime API, troubleshooting.",
  portalMark: "Codewhale documentation",
  heroTitle: "Find the guidance you need.",
  heroLead:
    "Start with the guide and install pages, or go straight to vocabulary, modes, permissions, tools, providers, Pod, hooks, MCP, and the Runtime API. Each page links to its source document in the repository.",
  installCta: "Install Codewhale",
  sourceDocsCta: "Browse source docs ↗",
};
