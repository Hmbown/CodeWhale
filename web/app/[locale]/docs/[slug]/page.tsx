import Link from "next/link";
import { notFound } from "next/navigation";
import { CUSTOM_DOC_PAGES, DOC_TOPICS, REPO_DOCS_BASE, getTopic } from "@/lib/docs-map";
import { markdownRenderedTopics, readTopicSources } from "@/lib/docs-content";
import { renderDocMarkdown, Slugger, type DocHeading } from "@/lib/markdown";
import { buildPageMetadata } from "@/lib/page-meta";

/**
 * Dynamic docs renderer — /[locale]/docs/[slug].
 *
 * Renders the repo's canonical Markdown docs (registered in lib/docs-map.ts)
 * as first-class pages. Markdown is read from the repo checkout at build time
 * (same assumption as scripts/derive-facts.mjs); the route is fully static so
 * the deployed worker never touches the filesystem.
 *
 * Slugs with hand-authored pages (CUSTOM_DOC_PAGES) are excluded here — their
 * static app/[locale]/docs/<slug>/ routes take precedence.
 */

const REPO_EDIT_BASE = "https://github.com/Hmbown/CodeWhale/edit/main";

export const dynamic = "force-static";
export const dynamicParams = false;

export function generateStaticParams() {
  return markdownRenderedTopics(CUSTOM_DOC_PAGES).map((t) => ({ slug: t.slug }));
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ locale: string; slug: string }>;
}) {
  const { locale, slug } = await params;
  const topic = getTopic(slug) ?? DOC_TOPICS.find((t) => t.slug === slug);
  const isZh = locale === "zh";
  if (!topic) return {};
  return buildPageMetadata({
    path: `/docs/${topic.slug}`,
    locale,
    title: isZh ? `${topic.label.zh} · CodeWhale 文档` : `${topic.label.en} · CodeWhale Docs`,
    description: isZh ? topic.description.zh : topic.description.en,
  });
}

/* ------------------------------------------------------------------ */

function fileStem(repoPath: string): string {
  const base = repoPath.split("/").pop() ?? repoPath;
  return base.replace(/\.md$/i, "");
}

function OnThisPage({
  sections,
  isZh,
}: {
  sections: { repoPath: string; headings: DocHeading[] }[];
  isZh: boolean;
}) {
  const entries = sections.flatMap((s) => s.headings.filter((h) => h.depth <= 2));
  if (entries.length < 2) return null;
  return (
    <nav
      aria-label={isZh ? "本页目录" : "On this page"}
      className="hairline-t hairline-b hairline-l hairline-r px-4 py-3"
    >
      <div className="eyebrow mb-2">{isZh ? "本页目录" : "On this page"}</div>
      <ul className="columns-1 sm:columns-2 gap-x-8 space-y-1">
        {entries.map((h) => (
          <li key={h.id} className={h.depth > 1 ? "pl-4" : ""}>
            <a
              href={`#${h.id}`}
              className="text-sm text-ink-soft hover:text-indigo transition-colors"
            >
              {h.text}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  );
}

/* ------------------------------------------------------------------ */

export default async function DocPage({
  params,
}: {
  params: Promise<{ locale: string; slug: string }>;
}) {
  const { locale, slug } = await params;
  const isZh = locale === "zh";

  const topic = getTopic(slug);
  if (!topic || !topic.hasPage || (CUSTOM_DOC_PAGES as readonly string[]).includes(slug)) {
    notFound();
  }

  const sources = readTopicSources(topic);
  const slugger = new Slugger();
  const rendered = sources.map((src) => {
    const { html, headings } = renderDocMarkdown(src.markdown, {
      sourceRepoPath: src.repoPath,
      locale,
      slugger,
    });
    return { repoPath: src.repoPath, html, headings };
  });

  return (
    <section className="space-y-8">
      {/* Title */}
      <header>
        <h2 className="font-display text-3xl mb-1">
          {isZh ? topic.label.zh : topic.label.en}{" "}
          <span className="font-cjk text-indigo text-2xl ml-2">
            {isZh ? topic.label.en : topic.label.zh}
          </span>
        </h2>
        <p className={`text-ink-soft mt-3 ${isZh ? "leading-[1.9] tracking-wide" : "leading-relaxed"}`}>
          {isZh ? topic.description.zh : topic.description.en}
        </p>
        {isZh && topic.zhIntro && (
          <p className="mt-4 text-ink-soft leading-[1.9] tracking-wide">{topic.zhIntro}</p>
        )}
        {isZh && (
          <p className="mt-3 text-xs text-ink-mute leading-relaxed">
            本页正文直接渲染自仓库中的英文文档（真实来源），暂未翻译。
          </p>
        )}
      </header>

      {/* On this page */}
      <OnThisPage sections={rendered} isZh={isZh} />

      {/* Rendered sources */}
      {rendered.map((src) => (
        <section key={src.repoPath} id={`src-${fileStem(src.repoPath).toLowerCase()}`} className="scroll-mt-32">
          <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 hairline-b pb-2 mb-4">
            <span className="font-mono text-[0.68rem] uppercase tracking-widest text-ink-mute">
              {src.repoPath}
            </span>
            <Link
              href={`${REPO_EDIT_BASE}/${src.repoPath}`}
              className="font-mono text-[0.68rem] uppercase tracking-widest text-indigo hover:underline"
              target="_blank"
              rel="noopener noreferrer"
            >
              {isZh ? "在 GitHub 上编辑此页 ↗" : "Edit this page on GitHub ↗"}
            </Link>
          </div>
          <div
            className="doc-prose"
            // Trusted content: rendered at build time from this repo's own docs.
            dangerouslySetInnerHTML={{ __html: src.html }}
          />
        </section>
      ))}

      {/* Source footer — same convention as the hand-authored docs pages */}
      <section className="hairline-t pt-8">
        <p className="text-sm text-ink-mute">
          {isZh ? "来源文档：" : "Source: "}
          {sources.map((s, i) => (
            <span key={s.repoPath}>
              {i > 0 && " · "}
              <Link
                href={`${REPO_DOCS_BASE}/${s.repoPath}`}
                className="body-link"
                target="_blank"
                rel="noopener noreferrer"
              >
                {s.repoPath}
              </Link>
            </span>
          ))}
          {isZh ? " · 更新时请同步修改 docs-map.ts。" : " · Update docs-map.ts when changing."}
        </p>
      </section>
    </section>
  );
}
