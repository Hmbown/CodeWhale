/**
 * /llms.txt — agent-readable documentation index (llmstxt.org convention).
 *
 * Lists every registered docs topic: the rendered page on codewhale.net where
 * one exists, plus the canonical repo Markdown source(s) each page is built
 * from. Generated statically at build time from lib/docs-map.ts, so it can
 * never drift from the sidebar / docs hub.
 */
import { DOC_TOPICS, REPO_DOCS_BASE, type DocTopic } from "@/lib/docs-map";
import { SITE_URL, IDENTITY_PHRASE } from "@/lib/page-meta";

export const dynamic = "force-static";

const CATEGORY_TITLES: Record<DocTopic["category"], string> = {
  "getting-started": "Getting started",
  workflows: "Workflows",
  "core-concepts": "Core concepts",
  reference: "Reference",
  extending: "Extending",
  operations: "Operations",
};

function sources(t: DocTopic): string[] {
  return Array.isArray(t.repoSource) ? t.repoSource : [t.repoSource];
}

export async function GET() {
  const lines: string[] = [
    "# CodeWhale",
    "",
    `> ${IDENTITY_PHRASE} Open-source (MIT) terminal coding agent with first-class DeepSeek and open-model support, a nested constitution, sandboxed tools, MCP, sub-agents, Fleet, and a local Runtime API.`,
    "",
    "Docs pages are rendered from the canonical Markdown files in the GitHub",
    "repository (https://github.com/Hmbown/CodeWhale); the repo files are the",
    "source of truth. Raw sources are listed next to each page.",
    "",
  ];

  const byCategory = new Map<string, DocTopic[]>();
  for (const t of DOC_TOPICS) {
    byCategory.set(t.category, [...(byCategory.get(t.category) ?? []), t]);
  }

  for (const [cat, topics] of byCategory) {
    lines.push(`## ${CATEGORY_TITLES[cat as DocTopic["category"]] ?? cat}`, "");
    for (const t of topics) {
      const url = t.hasPage
        ? `${SITE_URL}/en/docs/${t.slug}`
        : `${REPO_DOCS_BASE}/${sources(t)[0]}`;
      const srcNote = t.hasPage ? ` (source: ${sources(t).join(", ")})` : "";
      lines.push(`- [${t.label.en}](${url}): ${t.description.en}${srcNote}`);
    }
    lines.push("");
  }

  lines.push(
    "## Optional",
    "",
    `- [Full docs as one file](${SITE_URL}/llms-full.txt): concatenated Markdown of every rendered docs page`,
    `- Per-page Markdown: ${SITE_URL}/llms-full/<slug>.txt for any rendered page above (e.g. ${SITE_URL}/llms-full/guide.txt)`,
    `- [Repository](https://github.com/Hmbown/CodeWhale): full source, issues, releases`,
    `- [FAQ](${SITE_URL}/en/faq): common questions`,
    "",
  );

  return new Response(lines.join("\n"), {
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}
