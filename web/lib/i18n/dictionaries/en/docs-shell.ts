import type { DocsShellDict } from "../types";

/**
 * English reference dictionary for the docs shell: the portal hero in
 * `app/[locale]/docs/layout.tsx`, the hub metadata, the task/topic search,
 * the sidebar and breadcrumb chrome, the release-truth band, and the
 * contextual help band that closes every docs page.
 */
export const docsShell: DocsShellDict = {
  metaTitle: "Docs · Codewhale",
  metaDescription:
    "Codewhale documentation: install, user guide, configuration, providers, account and keys, cloud computers, security and trust, core concepts, tools, MCP, skills, sandbox, runtime API, troubleshooting.",
  portalMark: "Codewhale documentation",
  heroTitle: "Find the guidance you need.",
  heroLead:
    "Start from a task or a topic. Every page states which repository document it is drawn from and which release it describes.",
  installCta: "Install Codewhale",
  sourceDocsCta: "Browse source docs ↗",

  releaseLabel: "Release truth",
  releasePublished: "Latest release {tag} · {date}",
  releaseCandidate:
    "These pages describe the {version} source candidate, which is not published yet.",
  releaseMatches: "These pages describe {tag}, the published release.",
  releaseChangelog: "Changelog →",

  searchLabel: "Search the documentation",
  searchPlaceholder: "Search by task or topic… (press / to focus)",
  searchClear: "Clear",
  searchMatches: "{matched} of {total} entries match “{query}”",
  searchNoMatches: "Nothing matches “{query}”",
  tasksHeading: "By task",
  tasksLead: "Start from what you are trying to do.",
  topicsHeading: "By topic",
  webGuideTag: "Web guide",
  sourceDocTag: "Source doc",
  emptyTitle: "No matching entry",
  emptyBody:
    "Try a different word — searches match English and Chinese — or browse the complete docs directory on GitHub.",
  emptyCta: "GitHub docs directory ↗",
  indexNote:
    "Web guides stay on codewhale.net. Source docs open the complete reference in the GitHub repository. Tasks come from docs-tasks.ts and topics from docs-map.ts; both registries live in the repository.",

  sidebarHeading: "Documentation",
  sidebarAria: "Documentation index",
  breadcrumbAria: "Breadcrumb",
  breadcrumbHome: "Home",
  breadcrumbDocs: "Docs",

  helpTitle: "Need more than this page?",
  helpLead:
    "Every guide is drawn from a repository document. If it is wrong or missing something, the fastest fix is to say so where the maintainer will see it.",
  helpSource: "Source: {name}",
  helpTroubleshooting: "Troubleshooting",
  helpFaq: "FAQ",
  helpDiscord: "Ask on Discord ↗",
  helpIssue: "Report a docs problem ↗",
};
