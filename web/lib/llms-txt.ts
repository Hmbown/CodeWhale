import { DOC_TOPICS, REPO_DOCS_BASE, docTopicHref } from "./docs-map";
import { PUBLIC_MEMBERSHIP_COPY } from "./content/membership";
import { DISCORD_URL, REPO_URL } from "./i18n/links";
import { IDENTITY_PHRASE, SITE_NAME, SITE_URL } from "./page-meta";

/**
 * First-party routes that are in the sitemap but are not a docs-map topic.
 * English canonicals only — this file is the machine-readable index.
 */
const EXTRA_PAGES: readonly { path: string; title: string; description: string }[] = [
  {
    path: "/docs",
    title: "Documentation",
    description:
      "Documentation hub: install, guide, configuration, providers, modes, tools, MCP, sandbox, Fleet, and the Runtime API.",
  },
  {
    path: "/faq",
    title: "FAQ",
    description:
      "Frequently asked questions: install, config, providers, models, modes, security, and privacy.",
  },
  {
    path: "/runtime",
    title: "Runtime & Integrations",
    description:
      "Local Runtime API, HTTP/SSE, baseline ACP stdio adapter, MCP servers, and the Phase 0 VS Code companion.",
  },
  {
    path: "/constitution",
    title: "Three layers of law",
    description:
      "Nested constitution: bundled base law, standing law (/constitution), and the repo's .codewhale/constitution.json.",
  },
  {
    path: "/roadmap",
    title: "Roadmap",
    description: "Shipped, underway, considered, and ruled-out work.",
  },
  {
    path: "/feed",
    title: "Activity",
    description: "Recent repository issues, pull requests, and releases.",
  },
  {
    path: "/digest",
    title: "Community digest",
    description: "The weekly project record, reviewed by a maintainer.",
  },
  {
    path: "/community",
    title: "Community",
    description: "Release contributors, helpers, and ways to take part.",
  },
  {
    path: "/contribute",
    title: "Contribute",
    description: "The pull-request workflow: scoped issue, fork, test the change, explain the result.",
  },
  {
    path: "/pricing",
    title: "Pricing",
    description: PUBLIC_MEMBERSHIP_COPY.metadata.description.en,
  },
  {
    path: "/legal/terms",
    title: "Terms of service",
    description: "Terms that govern your use of Codewhale, a Shannon Labs product.",
  },
  {
    path: "/legal/privacy",
    title: "Privacy policy",
    description: "How Shannon Labs handles information when you use Codewhale.",
  },
];

function topicSitePath(topic: (typeof DOC_TOPICS)[number]): string | null {
  if (topic.sitePath) return `/${topic.sitePath}`;
  if (topic.hasPage) return `/docs/${topic.slug}`;
  return null;
}

/** Plain-text /llms.txt body generated from the docs registry and sitemap extras. */
export function buildLlmsTxt(): string {
  const covered = new Set<string>();
  const lines: string[] = [
    `# ${SITE_NAME}`,
    "",
    `> ${IDENTITY_PHRASE}`,
    "",
    `${SITE_NAME} is a terminal-native coding agent for hosted and local models.`,
    `Official site: ${SITE_URL}`,
    `Source: ${REPO_URL}`,
    "",
    "## Documentation",
    "",
  ];

  for (const topic of DOC_TOPICS) {
    const path = topicSitePath(topic);
    if (!path) continue;
    covered.add(path);
    const href = `${SITE_URL}${docTopicHref(topic, "en")}`;
    lines.push(`- [${topic.label.en}](${href}): ${topic.description.en}`);
  }

  lines.push("", "## Pages", "");
  for (const page of EXTRA_PAGES) {
    if (covered.has(page.path)) continue;
    lines.push(`- [${page.title}](${SITE_URL}/en${page.path}): ${page.description}`);
  }

  lines.push("", "## Source documents", "");
  for (const topic of DOC_TOPICS) {
    if (topic.hasPage) continue;
    const source = Array.isArray(topic.repoSource) ? topic.repoSource[0] : topic.repoSource;
    lines.push(`- [${topic.label.en}](${REPO_DOCS_BASE}/${source}): ${topic.description.en}`);
  }

  lines.push("", "## Optional", "", `- [GitHub](${REPO_URL})`, `- [Discord](${DISCORD_URL})`, "");
  return lines.join("\n");
}
