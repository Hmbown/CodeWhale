/**
 * /llms-full/<slug>.txt — one rendered docs page's Markdown source(s).
 *
 * Per-page companion to the aggregate /llms-full.txt, for agents that want a
 * single topic without fetching the whole corpus. Same build-time assembly:
 * generated statically from the repo checkout; the deployed worker serves the
 * prerendered responses and never reads disk.
 */
import { notFound } from "next/navigation";
import { DOC_TOPICS } from "@/lib/docs-map";
import { readTopicSources } from "@/lib/docs-content";
import { SITE_URL } from "@/lib/page-meta";

export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return DOC_TOPICS.filter((t) => t.hasPage).map((t) => ({ slug: `${t.slug}.txt` }));
}

export async function GET(
  _req: Request,
  { params }: { params: Promise<{ slug: string }> },
) {
  const { slug } = await params;
  const topic = DOC_TOPICS.find((t) => t.hasPage && `${t.slug}.txt` === slug);
  if (!topic) notFound();

  const parts: string[] = [
    `# CodeWhale docs — ${topic.label.en}`,
    "",
    `Page: ${SITE_URL}/en/docs/${topic.slug}`,
    `Index: ${SITE_URL}/llms.txt | Full corpus: ${SITE_URL}/llms-full.txt`,
    "Each section below is the canonical repo Markdown this page is built from.",
  ];
  for (const src of readTopicSources(topic)) {
    parts.push("", "---", "", `<!-- source: ${src.repoPath} -->`, "", src.markdown.trimEnd());
  }

  return new Response(parts.join("\n") + "\n", {
    headers: { "content-type": "text/plain; charset=utf-8" },
  });
}
