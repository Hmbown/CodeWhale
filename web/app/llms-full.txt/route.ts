/**
 * /llms-full.txt — every rendered docs page's Markdown source, concatenated.
 *
 * Companion to /llms.txt for agents that prefer one fetch. Assembled at build
 * time from the repo checkout (same assumption as scripts/derive-facts.mjs);
 * the deployed worker serves the prerendered response and never reads disk.
 */
import { DOC_TOPICS } from "@/lib/docs-map";
import { readTopicSources } from "@/lib/docs-content";
import { SITE_URL } from "@/lib/page-meta";

export const dynamic = "force-static";

export async function GET() {
  const parts: string[] = [
    "# CodeWhale — full documentation",
    "",
    `Index: ${SITE_URL}/llms.txt`,
    "Each section below is the canonical repo Markdown a rendered docs page is built from.",
    "",
  ];

  for (const topic of DOC_TOPICS) {
    if (!topic.hasPage) continue;
    for (const src of readTopicSources(topic)) {
      parts.push(
        "",
        "---",
        "",
        `<!-- page: ${SITE_URL}/en/docs/${topic.slug} | source: ${src.repoPath} -->`,
        "",
        src.markdown.trimEnd(),
      );
    }
  }

  return new Response(parts.join("\n") + "\n", {
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}
